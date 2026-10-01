//! The facts Toss reports about an input no handler claims.
//!
//! This is what keeps `toss <anything>` useful (§7). Instead of a bare
//! refusal, an unhandled file comes back with its name, size, type and
//! timestamp, gathered from metadata alone — no large dependency is needed
//! to answer "what is this thing?".
//!
//! The fields follow §7: filename, path, size, extension, detected type and
//! modified time. The detected type is supplied by the caller, because
//! classification belongs to `detection/` and must not be duplicated here.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What is known about one path, gathered without touching file contents.
#[derive(Debug)]
pub struct Facts {
    filename: String,
    path: PathBuf,
    size: Option<u64>,
    is_dir: bool,
    extension: Option<String>,
    modified: Option<SystemTime>,
}

impl Facts {
    /// Render the report as it appears on stdout (§25).
    ///
    /// `detected` is the type classification produced elsewhere — this module
    /// only knows how to describe a path, not how to identify one.
    pub fn render(&self, detected: &str) -> String {
        let mut text = String::new();

        push(&mut text, "Filename", &self.filename);
        push(&mut text, "Path", &self.path.display().to_string());
        push(&mut text, "Size", &self.size_line());
        push(
            &mut text,
            "Extension",
            self.extension.as_deref().unwrap_or("(none)"),
        );
        push(&mut text, "Detected type", detected);
        push(&mut text, "Modified time", &format_time(self.modified));

        text
    }

    fn size_line(&self) -> String {
        if self.is_dir {
            // The filesystem's own size for a directory entry says nothing
            // useful, so report what it is rather than invent a number.
            return "(directory)".to_owned();
        }

        match self.size {
            Some(bytes) => human_size(bytes),
            None => "unknown".to_owned(),
        }
    }
}

/// Describe `path` using metadata only. Never fails: a path whose metadata
/// cannot be read still reports the facts that come from the name itself.
pub fn gather(path: &Path) -> Facts {
    let metadata = std::fs::metadata(path).ok();

    Facts {
        filename: path
            .file_name()
            .map(|name| Path::new(name).display().to_string())
            .unwrap_or_else(|| path.display().to_string()),
        path: path.to_path_buf(),
        size: metadata.as_ref().map(|meta| meta.len()),
        is_dir: metadata.as_ref().is_some_and(|meta| meta.is_dir()),
        extension: path
            .extension()
            .map(|ext| ext.to_string_lossy().into_owned()),
        modified: metadata.as_ref().and_then(|meta| meta.modified().ok()),
    }
}

fn push(text: &mut String, label: &str, value: &str) {
    text.push_str(&format!("{label:<13}: {value}\n"));
}

/// Render an exact byte count alongside a readable unit.
///
/// A byte count as one display line: `747 bytes`, or `9175040 bytes
/// (8.7 MiB)` once there is a unit worth using.
///
/// Shared with the summary the image viewer prints beside its window, so a
/// size looks the same whether it comes from the info fallback or from
/// looking at a picture (§25: one product, one voice).
///
/// The decimal is computed with integer arithmetic on purpose. Formatting a
/// float here would link Rust's entire float-to-decimal machinery into the
/// binary for one display line — measured at 22,528 bytes, and §2 ranks
/// binary size above feature count.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];

    if bytes < 1024 {
        return format!("{bytes} bytes");
    }

    let mut unit = 0;
    let mut probe = bytes;
    while probe >= 1024 && unit < UNITS.len() - 1 {
        probe /= 1024;
        unit += 1;
    }

    // The remainder is always smaller than the divisor, so multiplying it by
    // ten cannot overflow.
    let divisor = 1024u64.pow(unit as u32);
    let whole = bytes / divisor;
    let tenths = (bytes % divisor) * 10 / divisor;

    format!("{bytes} bytes ({whole}.{tenths} {})", UNITS[unit])
}

fn format_time(time: Option<SystemTime>) -> String {
    let Some(time) = time else {
        return "unknown".to_owned();
    };

    // No timezone database and no dependency for one, so the report states
    // UTC plainly rather than implying a local time it did not compute.
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_secs() as i64,
        Err(before_epoch) => -(before_epoch.duration().as_secs() as i64),
    };

    format_utc(seconds)
}

/// Format Unix seconds as `YYYY-MM-DD HH:MM:SS UTC`.
///
/// The civil-date conversion is Howard Hinnant's algorithm: integer
/// arithmetic only, correct either side of the epoch, and far too small to
/// justify a date library for a single report line.
fn format_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);

    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }

    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{format_utc, gather, human_size};

    fn manifest(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn every_field_of_the_info_fallback_is_present() {
        let report = gather(&manifest("Cargo.toml")).render("unknown");

        for label in [
            "Filename",
            "Path",
            "Size",
            "Extension",
            "Detected type",
            "Modified time",
        ] {
            assert!(
                report.contains(label),
                "report is missing {label}:\n{report}"
            );
        }
        assert!(
            report.contains("Cargo.toml"),
            "filename not shown:\n{report}"
        );
        assert!(report.contains("toml"), "extension not shown:\n{report}");
    }

    #[test]
    fn a_directory_does_not_pretend_to_have_a_file_size() {
        let report = gather(&manifest("src")).render("directory");

        assert!(
            report.contains("(directory)"),
            "size line is wrong:\n{report}"
        );
        assert!(
            !report.contains("bytes"),
            "a directory should not report bytes:\n{report}"
        );
    }

    #[test]
    fn a_path_with_no_extension_says_so_rather_than_silently_omitting_it() {
        let report = gather(&manifest("Cargo.lock")).render("unknown");

        // Cargo.lock has an extension; use a bare name to prove the absence case.
        let bare = gather(Path::new("no-extension-here")).render("unknown");
        assert!(
            bare.contains("(none)"),
            "missing extension not shown:\n{bare}"
        );

        assert!(
            report.contains("lock"),
            "extension should be shown:\n{report}"
        );
    }

    #[test]
    fn a_file_whose_metadata_cannot_be_read_still_reports_its_name() {
        let report = gather(&manifest("no-such-file-for-info")).render("unknown");

        assert!(report.contains("no-such-file-for-info"), "{report}");
        assert!(
            report.contains("unknown"),
            "size/time should degrade, not panic:\n{report}"
        );
    }

    #[test]
    fn timestamps_format_as_readable_utc_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(format_utc(86_400), "1970-01-02 00:00:00 UTC");
        // A widely used reference point: 1000000000 is 2001-09-09 01:46:40 UTC.
        assert_eq!(format_utc(1_000_000_000), "2001-09-09 01:46:40 UTC");
    }

    #[test]
    fn timestamps_before_the_epoch_are_still_readable() {
        assert_eq!(format_utc(-1), "1969-12-31 23:59:59 UTC");
        assert_eq!(format_utc(-86_400), "1969-12-31 00:00:00 UTC");
    }

    #[test]
    fn sizes_stay_exact_in_bytes_and_gain_a_readable_unit_above_one_kibibyte() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1023), "1023 bytes");
        assert_eq!(human_size(1024), "1024 bytes (1.0 KiB)");
        assert_eq!(human_size(1536), "1536 bytes (1.5 KiB)");
        assert_eq!(human_size(1_048_576), "1048576 bytes (1.0 MiB)");
    }

    #[test]
    fn the_largest_possible_size_is_still_reported_without_overflowing() {
        // The tenths calculation multiplies a remainder by ten; this is the
        // input that would break it if that were done carelessly.
        let report = human_size(u64::MAX);

        assert!(report.contains("TiB"), "{report}");
        assert!(report.contains("18446744073709551615"), "{report}");
    }
}
