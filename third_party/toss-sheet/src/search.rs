//! Literal Unicode-insensitive search with a bounded compiled query and results.
use anyhow::{Result, bail};
pub const MAX_QUERY_BYTES: usize = 4096;
pub const MAX_RESULTS: usize = 4096;
pub const CELLS_PER_TICK: usize = 4096;
pub struct Search {
    pub row: usize,
    pub col: usize,
    pattern: regex::Regex,
}
impl Search {
    pub fn new(query: &str) -> Result<Self> {
        if query.len() > MAX_QUERY_BYTES {
            bail!("search query exceeds 4096 UTF-8 bytes");
        }
        let pattern = regex::RegexBuilder::new(&regex::escape(query))
            .case_insensitive(true)
            .size_limit(1024 * 1024)
            .dfa_size_limit(1024 * 1024)
            .build()?;
        Ok(Self {
            row: 0,
            col: 0,
            pattern,
        })
    }
    pub fn matches(&self, text: &str) -> bool {
        self.pattern.is_match(text)
    }
    pub fn advance(&mut self, width: usize) {
        self.col += 1;
        if self.col >= width {
            self.col = 0;
            self.row += 1;
        }
    }
}
