//! Decoding an image with WIC — no window involved.
//!
//! Split from the viewer on purpose (`IMAGE_VIEWER.md` §8): this half runs on
//! a headless CI machine, which is the only way "does WIC actually work"
//! becomes a question a pipeline can answer rather than one a person has to
//! be asked.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, GENERIC_READ,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::core::{Error, PCWSTR};

use crate::core::error::TossError;

/// An image in memory: 32 bits per pixel, top-down, no row padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Decoded {
    pub(super) width: u32,
    pub(super) height: u32,
    /// `width * height * 4` bytes, ordered B, G, R, A.
    pub(super) pixels: Vec<u8>,
}

/// Decode `path` into 32-bit PBGRA, or say why it could not be done.
pub(super) fn decode(path: &Path) -> Result<Decoded, TossError> {
    // Every WIC object is a COM object, so the apartment comes first, and it
    // is released however this function ends.
    let _apartment = Apartment::new()?;
    let wide = wide_path(path)?;

    // SAFETY:
    // - `wide` is NUL-terminated, outlives the block and is only read; WIC
    //   copies the name rather than keeping it;
    // - every interface below arrives inside a `Result`, is borrowed only
    //   until the block ends, and is released by windows-rs when it drops —
    //   the converter holds its own reference to the frame, so the frame and
    //   factory may go out of scope while the converter is still in use;
    // - `pixels` is plain storage whose length is computed from dimensions
    //   WIC itself reported, checked for overflow before allocation, and
    //   passed to `CopyPixels` as the buffer it is.
    unsafe {
        // The two object *factories* are about this machine working, not
        // about this file: a failure there is a broken Windows, not a broken
        // picture, so it stays a generic failure (§24's 1) rather than
        // being reported as a corrupt input the user could fix.
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(|err| super::failed("decode the image", err))?;

        // Everything from here down is about *this file*, and `refused` is
        // the one place that decides which number it answers with.
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|err| refused(path, err))?;
        let frame = decoder.GetFrame(0).map_err(|err| refused(path, err))?;

        let converter = factory
            .CreateFormatConverter()
            .map_err(|err| super::failed("decode the image", err))?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(|err| refused(path, err))?;

        let (mut width, mut height) = (0_u32, 0_u32);
        converter
            .GetSize(&mut width, &mut height)
            .map_err(|err| refused(path, err))?;

        // A zero-sized image is not an error WIC reports often, but it is
        // one it is allowed to — and it is still a statement about the file
        // rather than about the system (§24: 6, not 1).
        if width == 0 || height == 0 {
            return Err(corrupt(path, "the image reports no pixels"));
        }

        // Checked rather than `width * 4`: this is a number a file chose,
        // and §23 does not allow it to panic a debug build (wrap silently in
        // a release one either).
        let stride = width
            .checked_mul(4)
            .ok_or_else(|| corrupt(path, "the image is too wide to lay out"))?;
        let length = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| corrupt(path, "the image is too large to hold in memory"))?;

        let mut pixels = vec![0_u8; length];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .map_err(|err| refused(path, err))?;

        Ok(Decoded {
            width,
            height,
            pixels,
        })
    }
}

/// A COM apartment held for as long as the decode runs.
///
/// Tied to a value rather than to a code path so that an early `?` cannot
/// leave it initialised — the same rule `Reader` and `Writer` follow over in
/// the archive backend (§31).
struct Apartment;

impl Apartment {
    fn new() -> Result<Self, TossError> {
        // SAFETY: a null reserved pointer with apartment-threaded is the
        // documented pair of arguments, and every call this returns `Ok`
        // from is matched by the `CoUninitialize` in `Drop`.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .map_err(platform)?;

        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: exactly the initialisation `Apartment::new` performed, on
        // this same thread, which is where COM requires it to be balanced.
        unsafe { CoUninitialize() };
    }
}

/// The path as WIC wants it: a NUL-terminated UTF-16 buffer.
///
/// A Windows path already *is* UTF-16, so this widens rather than converts —
/// a name with no UTF-8 form still reaches the decoder (§22). An interior NUL
/// cannot be represented in a C string at all and is refused rather than
/// truncated at the first one (§16, §23).
fn wide_path(path: &Path) -> Result<Vec<u16>, TossError> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();

    if wide.contains(&0) {
        // The path came from the command line, and one with an interior NUL
        // is not a path Windows can name at all — so this is the argument's
        // fault, not the file's (§24: 2 rather than 6).
        return Err(TossError::invalid_arguments(format!(
            "path cannot be opened: {}",
            path.display()
        )));
    }
    wide.push(0);

    Ok(wide)
}

/// A `windows-rs` failure about *this machine*, in words a person can act
/// on (§23). Not for failures about the file being decoded — those go
/// through [`refused`], which knows which of §24's numbers each one earns.
fn platform(err: Error) -> TossError {
    super::failed("decode the image", err)
}

/// A decoder said no. This is the one place that decides which of §24's
/// numbers a failed decode answers with.
///
/// Toss only reaches a decoder with a file it has already classified as an
/// image — by extension or by magic (§6) — so a decoder that refuses it is
/// reporting on the *contents*. That is §24's "corrupt or incomplete",
/// **exit 6**, not exit 3: the format was recognised, the bytes were not
/// usable as that format. WIC's own text (`Unknown image format`) reads
/// like "unsupported", but it is answering for a file whose extension said
/// `png`, and the difference between "no decoder for this" and "this is no
/// longer a png" is exactly what the two codes exist to keep apart.
///
/// Two answers are about the file's *availability* rather than its contents
/// and keep their own numbers: a file that vanished is 4, one that is shut
/// against us is 5. A script waiting for a lock to clear has no use for
/// being told the picture is broken.
fn refused(path: &Path, err: Error) -> TossError {
    let code = err.code().0;

    if code == from_win32(ERROR_ACCESS_DENIED.0) {
        return TossError::PermissionDenied(path.to_path_buf());
    }
    if code == from_win32(ERROR_FILE_NOT_FOUND.0) || code == from_win32(ERROR_PATH_NOT_FOUND.0) {
        return TossError::InputNotFound(path.to_path_buf());
    }

    corrupt(path, &err.to_string())
}

/// The contents of `path` will not decode (§24: exit 6), in the words the
/// reason was found in.
fn corrupt(path: &Path, context: &str) -> TossError {
    TossError::CorruptInput {
        path: path.to_path_buf(),
        context: context.to_owned(),
    }
}

/// A Win32 error code in its HRESULT form: `HRESULT_FROM_WIN32(n)` puts the
/// code in the low word under facility 5. Computed rather than imported
/// because `windows-rs` 0.62 does not export the conversion helper.
const fn from_win32(code: u32) -> i32 {
    (0x8007_0000_u32 | code) as i32
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::decode;
    use crate::core::error::TossError;

    /// The colour every lossless sample was written with, in RGB.
    const TOP_LEFT: [u8; 3] = [200, 60, 10];

    /// `(name, width, height)`. The sizes are deliberately not all multiples
    /// of four, because a decoder that gets the row stride wrong still
    /// succeeds on 8x8 and only fails on 7x5.
    const SAMPLES: [(&str, u32, u32); 6] = [
        ("odd.png", 7, 5),
        ("odd.bmp", 9, 4),
        ("small.gif", 5, 3),
        ("block.jpg", 8, 8),
        ("unicode 中文 😊.png", 3, 3),
        ("panel.png", 320, 200),
    ];

    /// Formats that cannot lose a pixel, so the first one can be compared
    /// exactly rather than merely being present.
    const LOSSLESS: [&str; 4] = ["odd.png", "odd.bmp", "small.gif", "panel.png"];

    fn sample(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("corpus")
            .join("image")
            .join(name)
    }

    #[test]
    fn every_sample_decodes_to_the_size_it_advertises() {
        for (name, width, height) in SAMPLES {
            let image = decode(&sample(name)).unwrap_or_else(|err| panic!("{name}: {err}"));

            assert_eq!((image.width, image.height), (width, height), "{name}");
            assert_eq!(
                image.pixels.len(),
                (width * height * 4) as usize,
                "{name} is not 32 bits per pixel with no row padding"
            );
            assert!(
                image.pixels.iter().any(|byte| *byte != 0),
                "{name} decoded to nothing but zeroes"
            );
        }
    }

    #[test]
    fn lossless_samples_keep_the_colour_they_were_written_with() {
        // PBGRA, so the RGB triple arrives back to front.
        let expected = [TOP_LEFT[2], TOP_LEFT[1], TOP_LEFT[0], 255];

        for name in LOSSLESS {
            let image = decode(&sample(name)).unwrap_or_else(|err| panic!("{name}: {err}"));

            assert_eq!(
                &image.pixels[..4],
                &expected,
                "{name} decoded the wrong pixel, which means the channel order is wrong"
            );
        }
    }

    #[test]
    fn a_name_without_an_ascii_form_reaches_the_decoder() {
        // §22: the file name goes to WIC as UTF-16, so a name with no UTF-8
        // form is an ordinary case rather than an edge that fails.
        let image = decode(&sample("unicode 中文 😊.png")).expect("the sample decodes");

        assert_eq!((image.width, image.height), (3, 3));
    }

    #[test]
    fn a_broken_png_is_reported_as_a_corrupt_input_not_a_failure() {
        // §24 keeps exit 6 and exit 1 apart, and this is the case that
        // earns the 6: the extension said `png`, the format was recognised,
        // and the bytes inside are not one. Reporting it as a generic
        // failure would tell a script nothing — a script cannot act on
        // "something went wrong" the way it can on "this file is broken".
        //
        // The fixture stops inside its header on purpose. A file cut in the
        // middle of its pixel data is a different case — WIC returns the rows
        // it managed to read and zeroes the rest — and asserting on that
        // would be asserting how the OS happens to behave today.
        let err = decode(&sample("truncated.png")).expect_err("a broken PNG is not an image");

        assert!(
            matches!(err, TossError::CorruptInput { .. }),
            "expected a corrupt input, got {err:?}"
        );
        assert_eq!(
            err.exit_code(),
            crate::core::exit_code::ExitCode::CorruptInput,
            "the number §24 tables for this is 6, got {err}"
        );
        assert!(
            err.to_string().contains("truncated.png"),
            "the error should name the file it is about: {err}"
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_at_all_is_refused_without_panic() {
        // Reached only by calling the decoder directly — `toss view` is
        // refused by the classifier long before this (§6), so what is
        // asserted here is that a wrong input never *panics* (§23) and
        // never masquerades as a system fault.
        let err = decode(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect_err("a manifest is not a picture");

        assert!(
            matches!(err, TossError::CorruptInput { .. }),
            "expected a decode failure, got {err:?}"
        );
    }
}
