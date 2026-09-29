//! Decoding an image with WIC — no window involved.
//!
//! Split from the viewer on purpose (`IMAGE_VIEWER.md` §8): this half runs on
//! a headless CI machine, which is the only way "does WIC actually work"
//! becomes a question a pipeline can answer rather than one a person has to
//! be asked.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::GENERIC_READ;
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
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(platform)?;

        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(platform)?;
        let frame = decoder.GetFrame(0).map_err(platform)?;

        let converter = factory.CreateFormatConverter().map_err(platform)?;
        converter
            .Initialize(
                &frame,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(platform)?;

        let (mut width, mut height) = (0_u32, 0_u32);
        converter
            .GetSize(&mut width, &mut height)
            .map_err(platform)?;

        // A zero-sized image is not an error WIC reports often, but it is
        // one it is allowed to, and multiplying it through would produce an
        // empty buffer that `CopyPixels` would then be asked to fill.
        if width == 0 || height == 0 {
            return Err(TossError::other("image reports no pixels"));
        }

        // Checked rather than `width * 4`: this is a number a file chose,
        // and §23 does not allow it to panic a debug build (wrap silently in
        // a release one either).
        let stride = width
            .checked_mul(4)
            .ok_or_else(|| TossError::other("image is too wide to lay out"))?;
        let length = (stride as usize)
            .checked_mul(height as usize)
            .ok_or_else(|| TossError::other("image is too large to hold in memory"))?;

        let mut pixels = vec![0_u8; length];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .map_err(platform)?;

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
        return Err(TossError::other(format!(
            "path cannot be opened: {}",
            path.display()
        )));
    }
    wide.push(0);

    Ok(wide)
}

/// A `windows-rs` failure, in words a person can act on (§23).
fn platform(err: Error) -> TossError {
    super::failed("decode the image", err)
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
    const SAMPLES: [(&str, u32, u32); 5] = [
        ("odd.png", 7, 5),
        ("odd.bmp", 9, 4),
        ("small.gif", 5, 3),
        ("block.jpg", 8, 8),
        ("unicode 中文 😊.png", 3, 3),
    ];

    /// Formats that cannot lose a pixel, so the first one can be compared
    /// exactly rather than merely being present.
    const LOSSLESS: [&str; 3] = ["odd.png", "odd.bmp", "small.gif"];

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
    fn a_broken_png_is_reported_rather_than_returning_empty() {
        // Not "unsupported": the format was recognised and then refused, and
        // §24 keeps those two apart (exit 6 against exit 3).
        //
        // The fixture stops inside its header on purpose. A file cut in the
        // middle of its pixel data is a different case — WIC returns the rows
        // it managed to read and zeroes the rest — and asserting on that
        // would be asserting how the OS happens to behave today.
        let err = decode(&sample("truncated.png")).expect_err("a broken PNG is not an image");

        assert!(
            matches!(err, TossError::Other(_)),
            "expected a decode failure, got {err:?}"
        );
    }

    #[test]
    fn a_file_that_is_not_an_image_at_all_is_refused_without_panic() {
        let err = decode(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect_err("a manifest is not a picture");

        assert!(
            matches!(err, TossError::Other(_)),
            "expected a decode failure, got {err:?}"
        );
    }
}
