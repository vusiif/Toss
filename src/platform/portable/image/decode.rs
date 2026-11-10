//! Decoding an image with the bundled codecs — no window involved.
//!
//! The same contract as the Windows (WIC) decoder: premultiplied BGRA, the
//! shared budget checked from the header *before* the pixel buffer exists,
//! and the same exit codes for the same causes (4 missing, 5 refused,
//! 6 not decodable as the format it claimed to be).

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use image_codecs::{ColorType, ImageDecoder, ImageError, ImageReader, Limits};

use crate::core::error::TossError;
use crate::platform::raster::{
    Decoded, MAX_DIMENSION, MAX_PIXEL_BYTES, corrupt, pixel_buffer, pixel_layout,
};

/// Decode `path` into premultiplied BGRA, or say why it could not be done.
pub(crate) fn decode(path: &Path) -> Result<Decoded, TossError> {
    let file = File::open(path).map_err(|err| TossError::from_io(path, err))?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|err| TossError::from_io(path, err))?;
    if reader.format().is_none() {
        return Err(corrupt(path, "not a supported image format"));
    }
    let mut decoder = reader.into_decoder().map_err(|err| refused(path, err))?;

    // The codec's own working memory is capped as well as our output: a
    // header that lies about its size must not make the decoder allocate.
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_PIXEL_BYTES * 2);
    decoder
        .set_limits(limits)
        .map_err(|err| refused(path, err))?;

    let (width, height) = decoder.dimensions();
    let (_, length) = pixel_layout(path, width, height)?;
    let color = decoder.color_type();

    let native_length = usize::try_from(decoder.total_bytes())
        .map_err(|_| TossError::other("image exceeds the decoded byte limit"))?;
    let mut native = pixel_buffer(native_length)?;
    decoder
        .read_image(&mut native)
        .map_err(|err| refused(path, err))?;

    let mut pixels = pixel_buffer(length)?;
    to_premultiplied_bgra(color, &native, &mut pixels)
        .ok_or_else(|| corrupt(path, &format!("unsupported pixel layout {color:?}")))?;

    Ok(Decoded {
        width,
        height,
        pixels,
    })
}

/// Convert one decoded buffer into premultiplied BGRA, or `None` for a
/// layout these formats never produce.
fn to_premultiplied_bgra(color: ColorType, native: &[u8], out: &mut [u8]) -> Option<()> {
    let pixels = out.as_chunks_mut::<4>().0;
    let write = |pixel: &mut [u8; 4], r: u8, g: u8, b: u8, a: u8| {
        let premultiply = |c: u8| ((u16::from(c) * u16::from(a) + 127) / 255) as u8;
        *pixel = [premultiply(b), premultiply(g), premultiply(r), a];
    };
    let wide = |chunk: &[u8], i: usize| u16::from_ne_bytes([chunk[2 * i], chunk[2 * i + 1]]) >> 8;
    match color {
        ColorType::L8 => {
            for (pixel, &l) in pixels.iter_mut().zip(native) {
                write(pixel, l, l, l, 255);
            }
        }
        ColorType::La8 => {
            for (pixel, la) in pixels.iter_mut().zip(native.as_chunks::<2>().0) {
                write(pixel, la[0], la[0], la[0], la[1]);
            }
        }
        ColorType::Rgb8 => {
            for (pixel, rgb) in pixels.iter_mut().zip(native.as_chunks::<3>().0) {
                write(pixel, rgb[0], rgb[1], rgb[2], 255);
            }
        }
        ColorType::Rgba8 => {
            for (pixel, rgba) in pixels.iter_mut().zip(native.as_chunks::<4>().0) {
                write(pixel, rgba[0], rgba[1], rgba[2], rgba[3]);
            }
        }
        ColorType::L16 => {
            for (pixel, l) in pixels.iter_mut().zip(native.as_chunks::<2>().0) {
                let l = wide(l, 0) as u8;
                write(pixel, l, l, l, 255);
            }
        }
        ColorType::La16 => {
            for (pixel, la) in pixels.iter_mut().zip(native.as_chunks::<4>().0) {
                let (l, a) = (wide(la, 0) as u8, wide(la, 1) as u8);
                write(pixel, l, l, l, a);
            }
        }
        ColorType::Rgb16 => {
            for (pixel, rgb) in pixels.iter_mut().zip(native.as_chunks::<6>().0) {
                let c = |i| wide(rgb, i) as u8;
                write(pixel, c(0), c(1), c(2), 255);
            }
        }
        ColorType::Rgba16 => {
            for (pixel, rgba) in pixels.iter_mut().zip(native.as_chunks::<8>().0) {
                let c = |i| wide(rgba, i) as u8;
                write(pixel, c(0), c(1), c(2), c(3));
            }
        }
        _ => return None,
    }
    Some(())
}

/// A codec said no. The one place that decides which of §24's numbers a
/// failed decode answers with, mirroring the WIC backend's `refused`.
fn refused(path: &Path, err: ImageError) -> TossError {
    match err {
        // The file is already open, so a read that fails now is about its
        // contents (a truncated stream is `UnexpectedEof`), not about
        // availability — except the two cases that keep their own numbers.
        ImageError::IoError(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            TossError::from_io(path, err)
        }
        ImageError::IoError(err) => corrupt(path, &err.to_string()),
        ImageError::Limits(_) => TossError::other(format!(
            "{}: image exceeds the native viewer limit (32768 pixels per side, 64 Mi pixels, 256 MiB decoded)",
            path.display()
        )),
        other => corrupt(path, &other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::decode;
    use crate::core::error::TossError;

    /// The colour every lossless sample was written with, in RGB.
    const TOP_LEFT: [u8; 3] = [200, 60, 10];

    /// `(name, width, height)` — the same corpus the WIC decoder is held to.
    const SAMPLES: [(&str, u32, u32); 6] = [
        ("odd.png", 7, 5),
        ("odd.bmp", 9, 4),
        ("small.gif", 5, 3),
        ("block.jpg", 8, 8),
        ("unicode 中文 😊.png", 3, 3),
        ("panel.png", 320, 200),
    ];

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
            assert_eq!(image.pixels.len(), (width * height * 4) as usize, "{name}");
            assert!(image.pixels.iter().any(|byte| *byte != 0), "{name}");
        }
    }

    #[test]
    fn lossless_samples_keep_the_colour_they_were_written_with() {
        let expected = [TOP_LEFT[2], TOP_LEFT[1], TOP_LEFT[0], 255];
        for name in LOSSLESS {
            let image = decode(&sample(name)).unwrap_or_else(|err| panic!("{name}: {err}"));
            assert_eq!(&image.pixels[..4], &expected, "{name}: wrong channel order");
        }
    }

    #[test]
    fn a_broken_png_is_a_corrupt_input() {
        let err = decode(&sample("truncated.png")).expect_err("a broken PNG is not an image");
        assert!(matches!(err, TossError::CorruptInput { .. }), "{err:?}");
        assert!(err.to_string().contains("truncated.png"), "{err}");
    }

    #[test]
    fn a_missing_file_is_not_found() {
        let err = decode(&sample("missing.png")).expect_err("absent");
        assert!(matches!(err, TossError::InputNotFound(_)), "{err:?}");
    }

    #[test]
    fn a_file_that_is_not_an_image_is_refused_without_panic() {
        let err = decode(&Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
            .expect_err("a manifest is not a picture");
        assert!(matches!(err, TossError::CorruptInput { .. }), "{err:?}");
    }

    #[test]
    fn an_oversized_header_is_refused_before_the_pixels_are_allocated() {
        let path =
            std::env::temp_dir().join(format!("toss-portable-budget-{}.bmp", std::process::id()));
        let mut bytes = std::fs::read(sample("odd.bmp")).unwrap();
        bytes[18..22].copy_from_slice(&40_000_i32.to_le_bytes());
        bytes[22..26].copy_from_slice(&1_i32.to_le_bytes());
        bytes.resize(54 + 120_000, 0);
        bytes[2..6].copy_from_slice(&120_054_u32.to_le_bytes());
        bytes[34..38].copy_from_slice(&120_000_u32.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        let result = decode(&path);
        std::fs::remove_file(&path).unwrap();
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("native viewer limit")
        );
    }

    #[test]
    fn transparency_is_premultiplied() {
        let mut out = [0u8; 4];
        super::to_premultiplied_bgra(
            image_codecs::ColorType::Rgba8,
            &[200, 100, 50, 128],
            &mut out,
        )
        .unwrap();
        assert_eq!(out, [25, 50, 100, 128]);
    }
}
