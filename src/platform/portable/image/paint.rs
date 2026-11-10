//! Turning decoded pixels into a window's framebuffer — the portable
//! counterpart of GDI's `StretchDIBits` + `FillRect`.
//!
//! The picture is drawn at its zoomed size with its top-left corner at the
//! pan origin, clipped by the framebuffer, over the window background.
//! Scaling quality matches what the Windows viewer gets from HALFTONE:
//! exact pixels at 100%, bilinear when enlarging, and box-filtered halvings
//! (a mip pyramid) before bilinear when shrinking, so a large photo fitted
//! into a small window does not shimmer.

use crate::platform::raster::Decoded;

/// The window background, `0x00RRGGBB` as softbuffer wants it.
pub(super) const BACKGROUND: u32 = 0x00FF_FFFF;

/// Successive 2× box-filtered reductions of one image, built on demand.
#[derive(Default)]
pub(super) struct Pyramid {
    levels: Vec<Decoded>,
}

impl Pyramid {
    /// Forget the reductions; called when the picture changes.
    pub(super) fn clear(&mut self) {
        self.levels.clear();
    }

    /// The level to sample for drawing `image` at `scale`: the smallest one
    /// that is still at least as large as the destination.
    fn level<'a>(&'a mut self, image: &'a Decoded, scale: f64) -> &'a Decoded {
        let mut wanted = 0;
        let mut level_scale = 1.0;
        while scale <= level_scale / 2.0 && wanted < 16 {
            level_scale /= 2.0;
            wanted += 1;
        }
        while self.levels.len() < wanted {
            let previous = self.levels.last().unwrap_or(image);
            if previous.width < 2 && previous.height < 2 {
                break;
            }
            let Some(next) = halve(previous) else { break };
            self.levels.push(next);
        }
        match wanted.min(self.levels.len()) {
            0 => image,
            n => &self.levels[n - 1],
        }
    }
}

/// One 2× box-filtered reduction, or `None` if memory is not there (the
/// caller then samples the larger level, which is slower, not wrong).
fn halve(image: &Decoded) -> Option<Decoded> {
    let width = image.width.div_ceil(2);
    let height = image.height.div_ceil(2);
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(width as usize * height as usize * 4)
        .ok()?;
    let at = |x: u32, y: u32| {
        let x = x.min(image.width - 1) as usize;
        let y = y.min(image.height - 1) as usize;
        let i = (y * image.width as usize + x) * 4;
        &image.pixels[i..i + 4]
    };
    for y in 0..height {
        for x in 0..width {
            let quad = [
                at(2 * x, 2 * y),
                at(2 * x + 1, 2 * y),
                at(2 * x, 2 * y + 1),
                at(2 * x + 1, 2 * y + 1),
            ];
            for c in 0..4 {
                let sum: u32 = quad.iter().map(|p| u32::from(p[c])).sum();
                pixels.push(((sum + 2) / 4) as u8);
            }
        }
    }
    Some(Decoded {
        width,
        height,
        pixels,
    })
}

/// The size an image is drawn at, in client pixels — the same rounding the
/// Windows viewer uses, so both clamp panning against the same rectangle.
pub(super) fn drawn_size(image: &Decoded, scale: f64) -> (i32, i32) {
    let side = |length: u32| -> i32 {
        (f64::from(length) * scale)
            .round()
            .clamp(1.0, f64::from(i32::MAX)) as i32
    };
    (side(image.width), side(image.height))
}

/// Fill `frame` (`size.0 × size.1`, row-major) with the picture drawn at
/// `scale` with its top-left at `origin`, over [`BACKGROUND`].
pub(super) fn paint(
    frame: &mut [u32],
    size: (u32, u32),
    image: &Decoded,
    pyramid: &mut Pyramid,
    scale: f64,
    origin: (i32, i32),
) {
    let (width, height) = (size.0 as i64, size.1 as i64);
    frame.fill(BACKGROUND);
    if width == 0 || height == 0 || image.width == 0 || image.height == 0 {
        return;
    }
    let drawn = drawn_size(image, scale);
    let left = i64::from(origin.0).clamp(0, width);
    let right = (i64::from(origin.0) + i64::from(drawn.0)).clamp(0, width);
    let top = i64::from(origin.1).clamp(0, height);
    let bottom = (i64::from(origin.1) + i64::from(drawn.1)).clamp(0, height);
    if left >= right || top >= bottom {
        return;
    }

    // Exact pixels at 1:1 — what a user checking detail at 100% expects.
    if drawn == (image.width as i32, image.height as i32) {
        for y in top..bottom {
            let sy = (y - i64::from(origin.1)) as usize;
            let row = &image.pixels[sy * image.width as usize * 4..];
            for x in left..right {
                let sx = (x - i64::from(origin.0)) as usize;
                frame[(y * width + x) as usize] = over_background(&row[sx * 4..sx * 4 + 4]);
            }
        }
        return;
    }

    let source = pyramid.level(image, scale);
    let (sw, sh) = (source.width, source.height);
    // Where destination pixel centres land in the sampled level.
    let ratio_x = f64::from(sw) / f64::from(drawn.0);
    let ratio_y = f64::from(sh) / f64::from(drawn.1);
    let taps = |d: i64, o: i32, ratio: f64, limit: u32| -> (usize, usize, u32) {
        let s = ((d - i64::from(o)) as f64 + 0.5) * ratio - 0.5;
        let s = s.clamp(0.0, f64::from(limit - 1));
        let s0 = s.floor() as u32;
        let s1 = (s0 + 1).min(limit - 1);
        let weight = ((s - f64::from(s0)) * 256.0).round() as u32;
        (s0 as usize, s1 as usize, weight)
    };
    let columns: Vec<_> = (left..right)
        .map(|x| taps(x, origin.0, ratio_x, sw))
        .collect();
    let stride = sw as usize * 4;
    for y in top..bottom {
        let (y0, y1, wy) = taps(y, origin.1, ratio_y, sh);
        let row0 = &source.pixels[y0 * stride..(y0 + 1) * stride];
        let row1 = &source.pixels[y1 * stride..(y1 + 1) * stride];
        let out = &mut frame[(y * width + left) as usize..(y * width + right) as usize];
        for (pixel, &(x0, x1, wx)) in out.iter_mut().zip(&columns) {
            let mut blended = [0u8; 4];
            for (c, value) in blended.iter_mut().enumerate() {
                let p = |row: &[u8], x: usize| u32::from(row[x * 4 + c]);
                let top = p(row0, x0) * (256 - wx) + p(row0, x1) * wx;
                let bottom = p(row1, x0) * (256 - wx) + p(row1, x1) * wx;
                *value = ((top * (256 - wy) + bottom * wy + 32768) >> 16) as u8;
            }
            *pixel = over_background(&blended);
        }
    }
}

/// One premultiplied BGRA pixel composited over the white background.
fn over_background(bgra: &[u8]) -> u32 {
    let clear = 255 - u32::from(bgra[3]);
    let channel = |c: u8| (u32::from(c) + clear).min(255);
    (channel(bgra[2]) << 16) | (channel(bgra[1]) << 8) | channel(bgra[0])
}

#[cfg(test)]
mod tests {
    use super::{BACKGROUND, Pyramid, drawn_size, paint};
    use crate::platform::raster::Decoded;

    fn solid(width: u32, height: u32, bgra: [u8; 4]) -> Decoded {
        Decoded {
            width,
            height,
            pixels: bgra.repeat((width * height) as usize),
        }
    }

    #[test]
    fn actual_size_copies_exact_pixels_and_leaves_the_rest_background() {
        let image = solid(2, 2, [10, 20, 30, 255]);
        let mut frame = vec![0; 16];
        paint(
            &mut frame,
            (4, 4),
            &image,
            &mut Pyramid::default(),
            1.0,
            (1, 1),
        );
        assert_eq!(frame[5], 0x001E_140A);
        assert_eq!(frame[0], BACKGROUND);
        assert_eq!(frame[15], BACKGROUND);
    }

    #[test]
    fn transparent_pixels_show_the_background() {
        let image = solid(1, 1, [0, 0, 0, 0]);
        let mut frame = vec![0; 1];
        paint(
            &mut frame,
            (1, 1),
            &image,
            &mut Pyramid::default(),
            1.0,
            (0, 0),
        );
        assert_eq!(frame[0], BACKGROUND);
    }

    #[test]
    fn scaling_fills_exactly_the_drawn_rectangle() {
        let image = solid(100, 50, [0, 0, 255, 255]);
        for scale in [0.1, 0.33, 0.5, 1.7, 4.0] {
            let (w, h) = drawn_size(&image, scale);
            let size = (w as u32 + 4, h as u32 + 4);
            let mut frame = vec![0; (size.0 * size.1) as usize];
            paint(
                &mut frame,
                size,
                &image,
                &mut Pyramid::default(),
                scale,
                (2, 2),
            );
            let red = frame.iter().filter(|&&p| p == 0x00FF_0000).count();
            assert_eq!(red, (w * h) as usize, "scale {scale}");
        }
    }

    #[test]
    fn a_picture_panned_off_screen_draws_nothing_and_does_not_panic() {
        let image = solid(10, 10, [1, 2, 3, 255]);
        let mut frame = vec![0; 100];
        paint(
            &mut frame,
            (10, 10),
            &image,
            &mut Pyramid::default(),
            2.0,
            (-500, 500),
        );
        assert!(frame.iter().all(|&p| p == BACKGROUND));
    }
}
