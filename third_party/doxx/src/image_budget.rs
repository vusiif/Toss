//! Image decoding resource budget and admission policy for DOCX viewer.
//!
//! Provides pre-decode dimension and allocation validation, cumulative decoded-byte
//! accounting per viewer session, and graceful degradation placeholders.

use image::DynamicImage;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// Policy controlling image dimension and memory limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageBudgetPolicy {
    /// Maximum allowed image width in pixels.
    pub max_width: u32,
    /// Maximum allowed image height in pixels.
    pub max_height: u32,
    /// Maximum allowed total pixel count (width * height).
    pub max_pixels: u64,
    /// Maximum allowed decoded memory bytes for a single image.
    pub max_bytes_per_image: u64,
    /// Maximum cumulative decoded memory bytes admitted across a document/session.
    pub max_cumulative_bytes: u64,
}

impl Default for ImageBudgetPolicy {
    fn default() -> Self {
        Self {
            max_width: 8_192,
            max_height: 8_192,
            max_pixels: 16_777_216,
            max_bytes_per_image: 64 * 1024 * 1024,   // 64 MiB
            max_cumulative_bytes: 128 * 1024 * 1024, // 128 MiB
        }
    }
}

impl ImageBudgetPolicy {
    /// Construct a custom budget policy.
    pub fn new(
        max_width: u32,
        max_height: u32,
        max_pixels: u64,
        max_bytes_per_image: u64,
        max_cumulative_bytes: u64,
    ) -> Self {
        Self {
            max_width,
            max_height,
            max_pixels,
            max_bytes_per_image,
            max_cumulative_bytes,
        }
    }
}

/// Detailed reasons why an image was rejected during admission or decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRejectionReason {
    /// Image width or height exceeds maximum allowed dimensions.
    ExceedsDimensions {
        width: u32,
        height: u32,
        max_width: u32,
        max_height: u32,
    },
    /// Total pixel count exceeds maximum allowed limit.
    ExceedsPixels { pixels: u64, max_pixels: u64 },
    /// Decoded image byte size exceeds per-image limit.
    ExceedsBytes { bytes: u64, max_bytes: u64 },
    /// Decoded image byte size exceeds remaining cumulative session budget.
    ExceedsCumulativeBudget {
        required_bytes: u64,
        remaining_bytes: u64,
        max_cumulative_bytes: u64,
    },
    /// Image dimensions would cause arithmetic overflow.
    IntegerOverflow,
    /// All cache entries are pinned by the current screen.
    CacheCapacity { max_images: usize },
    /// Image format is unsupported or could not be detected.
    UnsupportedFormat(String),
    /// Image data is corrupt, truncated, or failed decoding.
    CorruptImage(String),
    /// IO error reading image file.
    IoError(String),
}

impl fmt::Display for ImageRejectionReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExceedsDimensions {
                width,
                height,
                max_width,
                max_height,
            } => write!(
                f,
                "dimensions {}x{} exceed limit {}x{}",
                width, height, max_width, max_height
            ),
            Self::ExceedsPixels { pixels, max_pixels } => {
                write!(f, "pixel count {} exceeds limit {}", pixels, max_pixels)
            }
            Self::ExceedsBytes { bytes, max_bytes } => {
                write!(f, "decoded size {} B exceeds limit {} B", bytes, max_bytes)
            }
            Self::ExceedsCumulativeBudget {
                required_bytes,
                remaining_bytes,
                max_cumulative_bytes,
            } => write!(
                f,
                "cumulative budget exceeded (needs {} B, {} B remaining of {} B)",
                required_bytes, remaining_bytes, max_cumulative_bytes
            ),
            Self::CacheCapacity { max_images } => write!(
                f,
                "image cache capacity exceeded ({max_images} images on this screen)"
            ),
            Self::IntegerOverflow => write!(f, "dimension arithmetic overflow"),
            Self::UnsupportedFormat(fmt) => write!(f, "unsupported format: {}", fmt),
            Self::CorruptImage(msg) => write!(f, "corrupt image data: {}", msg),
            Self::IoError(msg) => write!(f, "IO error: {}", msg),
        }
    }
}

impl ImageRejectionReason {
    /// Return a concise, accurate reason string for user-facing placeholders.
    pub fn to_reason_string(&self) -> String {
        self.to_string()
    }
}

/// Tracks decoded image memory budgets across a single document viewer session.
#[derive(Debug, Clone)]
pub struct ImageBudgetTracker {
    policy: ImageBudgetPolicy,
    cumulative_bytes_used: u64,
    retained_images: HashMap<usize, u64>,
}

impl Default for ImageBudgetTracker {
    fn default() -> Self {
        Self::new(ImageBudgetPolicy::default())
    }
}

impl ImageBudgetTracker {
    /// Create a new budget tracker with the given policy.
    pub fn new(policy: ImageBudgetPolicy) -> Self {
        Self {
            policy,
            cumulative_bytes_used: 0,
            retained_images: HashMap::new(),
        }
    }

    /// Current cumulative decoded bytes admitted and retained in the session.
    pub fn cumulative_bytes_used(&self) -> u64 {
        self.cumulative_bytes_used
    }

    /// Remaining cumulative decoded bytes available in the session.
    pub fn remaining_cumulative_bytes(&self) -> u64 {
        self.policy
            .max_cumulative_bytes
            .saturating_sub(self.cumulative_bytes_used)
    }

    /// The active budget policy.
    pub fn policy(&self) -> &ImageBudgetPolicy {
        &self.policy
    }

    /// Number of admitted images retained.
    pub fn admitted_count(&self) -> usize {
        self.retained_images.len()
    }

    /// Inspect header, validate limits before full allocation, and decode if admitted.
    ///
    /// If validation or decoding fails, the cumulative budget is NOT charged.
    fn checked_decoder(
        &self,
        path: &Path,
    ) -> Result<(impl image::ImageDecoder + 'static, u64), ImageRejectionReason> {
        let file =
            std::fs::File::open(path).map_err(|e| ImageRejectionReason::IoError(e.to_string()))?;
        let buf_reader = std::io::BufReader::new(file);

        let mut reader = image::ImageReader::new(buf_reader)
            .with_guessed_format()
            .map_err(|e| ImageRejectionReason::IoError(e.to_string()))?;

        // Configure decoder limits before constructing decoder
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(self.policy.max_bytes_per_image);
        reader.limits(limits);

        // Header inspection & decoder construction
        let decoder = reader.into_decoder().map_err(|err| match err {
            image::ImageError::Limits(_) => ImageRejectionReason::ExceedsBytes {
                bytes: self.policy.max_bytes_per_image.saturating_add(1),
                max_bytes: self.policy.max_bytes_per_image,
            },
            image::ImageError::Unsupported(u) => {
                ImageRejectionReason::UnsupportedFormat(u.to_string())
            }
            image::ImageError::IoError(e) => ImageRejectionReason::IoError(e.to_string()),
            other => ImageRejectionReason::CorruptImage(other.to_string()),
        })?;

        use image::ImageDecoder;
        let (width, height) = decoder.dimensions();

        // 1. Check dimensions
        if width > self.policy.max_width || height > self.policy.max_height {
            return Err(ImageRejectionReason::ExceedsDimensions {
                width,
                height,
                max_width: self.policy.max_width,
                max_height: self.policy.max_height,
            });
        }

        // 2. Check total pixels with checked arithmetic
        let pixels = (width as u64)
            .checked_mul(height as u64)
            .ok_or(ImageRejectionReason::IntegerOverflow)?;

        if pixels > self.policy.max_pixels {
            return Err(ImageRejectionReason::ExceedsPixels {
                pixels,
                max_pixels: self.policy.max_pixels,
            });
        }

        // 3. Check decoded byte size with checked arithmetic
        let bytes_per_pixel = decoder.color_type().bytes_per_pixel() as u64;
        let decoded_bytes = pixels
            .checked_mul(bytes_per_pixel)
            .ok_or(ImageRejectionReason::IntegerOverflow)?;

        let total_bytes = decoded_bytes.max(decoder.total_bytes());
        if total_bytes > self.policy.max_bytes_per_image {
            return Err(ImageRejectionReason::ExceedsBytes {
                bytes: total_bytes,
                max_bytes: self.policy.max_bytes_per_image,
            });
        }

        Ok((decoder, total_bytes))
    }

    /// Inspect an image without decoding or retaining it. A native viewer keeps
    /// one image at a time, so compare against the full cache capacity here.
    pub fn validate_image(&self, path: &Path) -> Result<(), ImageRejectionReason> {
        let (_, bytes) = self.checked_decoder(path)?;
        if bytes > self.policy.max_cumulative_bytes {
            return Err(ImageRejectionReason::ExceedsCumulativeBudget {
                required_bytes: bytes,
                remaining_bytes: self.policy.max_cumulative_bytes,
                max_cumulative_bytes: self.policy.max_cumulative_bytes,
            });
        }
        Ok(())
    }

    /// Release a decoded image after its owning protocol has been dropped.
    pub fn release_element(&mut self, element_index: usize) {
        if let Some(bytes) = self.retained_images.remove(&element_index) {
            self.cumulative_bytes_used = self.cumulative_bytes_used.saturating_sub(bytes);
        }
    }

    /// Inspect header, check retained-byte capacity, then decode and charge.
    pub fn load_and_decode_element(
        &mut self,
        element_index: usize,
        path: &Path,
    ) -> Result<DynamicImage, ImageRejectionReason> {
        let (decoder, total_bytes) = self.checked_decoder(path)?;
        // 4. Check cumulative budget
        let remaining = self.remaining_cumulative_bytes();
        if total_bytes > remaining {
            return Err(ImageRejectionReason::ExceedsCumulativeBudget {
                required_bytes: total_bytes,
                remaining_bytes: remaining,
                max_cumulative_bytes: self.policy.max_cumulative_bytes,
            });
        }

        // 5. Full decode after pre-checks pass
        let dyn_img = DynamicImage::from_decoder(decoder)
            .map_err(|e| ImageRejectionReason::CorruptImage(e.to_string()))?;

        // 6. Charge cumulative budget upon confirmed successful decode
        self.cumulative_bytes_used = self.cumulative_bytes_used.saturating_add(total_bytes);
        self.retained_images.insert(element_index, total_bytes);

        Ok(dyn_img)
    }
}
