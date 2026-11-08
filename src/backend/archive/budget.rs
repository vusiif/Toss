//! Streaming extraction limits. Declared sizes are hints; output is charged
//! again immediately before each disk write.
use super::types::ArchiveError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtractionLimits {
    pub total_bytes: u64,
    pub file_bytes: u64,
    pub entries: u64,
}

impl Default for ExtractionLimits {
    fn default() -> Self {
        Self {
            total_bytes: 64 * 1024 * 1024 * 1024,
            file_bytes: 32 * 1024 * 1024 * 1024,
            entries: 100_000,
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ArchiveSummary {
    pub entries: u64,
    pub regular_files: u64,
    pub declared_bytes: u64,
    pub unknown_sizes: u64,
}

impl ArchiveSummary {
    pub fn add(
        &mut self,
        regular: bool,
        size: Option<u64>,
        limits: ExtractionLimits,
    ) -> Result<(), ArchiveError> {
        self.entries = charge(self.entries, 1, limits.entries, "member count")?;
        if regular {
            self.regular_files += 1; // entries bounds this increment.
            if let Some(size) = size {
                charge(0, size, limits.file_bytes, "declared file bytes")?;
                self.declared_bytes = charge(
                    self.declared_bytes,
                    size,
                    limits.total_bytes,
                    "declared total bytes",
                )?;
            } else {
                self.unknown_sizes += 1;
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct ExtractionBudget {
    limits: ExtractionLimits,
    entries: u64,
    total: u64,
    file: u64,
}

impl ExtractionBudget {
    pub fn new(limits: ExtractionLimits) -> Self {
        Self {
            limits,
            entries: 0,
            total: 0,
            file: 0,
        }
    }
    pub fn member(&mut self) -> Result<(), ArchiveError> {
        self.entries = charge(self.entries, 1, self.limits.entries, "member count")?;
        self.file = 0;
        Ok(())
    }
    pub fn output(&mut self, count: u64) -> Result<(), ArchiveError> {
        let file = charge(
            self.file,
            count,
            self.limits.file_bytes,
            "actual file bytes",
        )?;
        let total = charge(
            self.total,
            count,
            self.limits.total_bytes,
            "actual total bytes",
        )?;
        self.file = file;
        self.total = total;
        Ok(())
    }
}

fn charge(used: u64, count: u64, limit: u64, resource: &str) -> Result<u64, ArchiveError> {
    used.checked_add(count)
        .filter(|&next| next <= limit)
        .ok_or_else(|| ArchiveError::BudgetExceeded(format!("{resource} exceeds limit {limit}")))
}

/// Trial policy: logical bytes plus worst-case cluster slack and an explicit
/// reserve and one allocation unit per entry as a provisional metadata
/// allowance. No empirical safety multiplier is asserted. Implicit parent
/// directories, decoder scratch and concurrent writers are not fully predicted.
pub fn space_required(summary: ArchiveSummary, allocation_unit: u64, reserve: u64) -> Option<u64> {
    if summary.unknown_sizes != 0 || allocation_unit == 0 {
        return None;
    }
    summary
        .regular_files
        .checked_mul(allocation_unit - 1)?
        .checked_add(summary.declared_bytes)?
        .checked_add(summary.entries.checked_mul(allocation_unit)?)?
        .checked_add(reserve)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_output_does_not_trust_declared_sizes_and_failure_does_not_release_prior_usage() {
        let limits = ExtractionLimits {
            total_bytes: 6,
            file_bytes: 4,
            entries: 3,
        };
        let mut budget = ExtractionBudget::new(limits);
        budget.member().unwrap();
        budget.output(4).unwrap();
        assert!(budget.output(1).is_err());
        budget.member().unwrap();
        assert!(budget.output(3).is_err());
        budget.output(2).unwrap();
        budget.member().unwrap();
        assert!(budget.output(1).is_err());
        assert!(budget.member().is_err());
    }
    #[test]
    fn summary_counts_empty_and_skipped_entries_and_unknowns() {
        let limits = ExtractionLimits {
            total_bytes: 9,
            file_bytes: 8,
            entries: 3,
        };
        let mut summary = ArchiveSummary::default();
        summary.add(false, None, limits).unwrap();
        summary.add(true, Some(0), limits).unwrap();
        summary.add(true, None, limits).unwrap();
        assert_eq!(
            (
                summary.entries,
                summary.unknown_sizes,
                summary.declared_bytes
            ),
            (3, 1, 0)
        );
        assert!(summary.add(false, None, limits).is_err());
        assert!(space_required(summary, 4096, 0).is_none());
    }
    #[test]
    fn counters_and_space_estimates_cannot_wrap() {
        assert!(charge(u64::MAX, 1, u64::MAX, "test").is_err());
        let summary = ArchiveSummary {
            regular_files: 2,
            declared_bytes: 100,
            ..Default::default()
        };
        assert_eq!(space_required(summary, 4096, 1000), Some(9290));
        assert!(space_required(summary, u64::MAX, 0).is_none());
    }
}
