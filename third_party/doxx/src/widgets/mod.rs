pub mod document;

use ratatui::text::Line;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

pub use document::{DocumentWidget, render_element_to_text};

/// Bounded FIFO cache for immutable text layout. The byte estimate includes
/// retained Vec capacities and span text, not allocator overhead or process RSS.
#[derive(Debug)]
pub struct LayoutCache {
    cache: HashMap<(usize, u16), CachedLines>,
    order: VecDeque<(usize, u16)>,
    retained_bytes: usize,
    /// Last known terminal width for invalidation
    last_width: u16,
    last_color: Option<bool>,
}

#[derive(Debug)]
struct CachedLines {
    lines: Vec<Line<'static>>,
    bytes: usize,
}

impl Default for LayoutCache {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutCache {
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    const MAX_ENTRIES: usize = 256;

    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            order: VecDeque::new(),
            retained_bytes: 0,
            last_width: 0,
            last_color: None,
        }
    }

    /// Get cached lines for an element, if available
    pub fn get(&self, element_index: usize, width: u16) -> Option<&Vec<Line<'static>>> {
        self.cache
            .get(&(element_index, width))
            .map(|entry| &entry.lines)
    }

    /// Store wrapped lines for an element
    pub fn insert(&mut self, element_index: usize, width: u16, lines: Vec<Line<'static>>) {
        let bytes = Self::line_bytes(&lines);
        let key = (element_index, width);
        if let Some(old) = self.cache.remove(&key) {
            self.retained_bytes -= old.bytes;
            self.order.retain(|entry| *entry != key);
        }
        if bytes > Self::MAX_BYTES {
            return;
        }
        while self.retained_bytes + bytes > Self::MAX_BYTES || self.cache.len() >= Self::MAX_ENTRIES
        {
            if let Some(old_key) = self.order.pop_front() {
                if let Some(old) = self.cache.remove(&old_key) {
                    self.retained_bytes -= old.bytes;
                }
            } else {
                break;
            }
        }
        self.retained_bytes += bytes;
        self.order.push_back(key);
        self.cache.insert(key, CachedLines { lines, bytes });
    }

    /// Borrow on a hit. Oversized layouts still render, without entering cache.
    pub fn get_or_insert_with(
        &mut self,
        element_index: usize,
        width: u16,
        create: impl FnOnce() -> Vec<Line<'static>>,
    ) -> Cow<'_, [Line<'static>]> {
        let key = (element_index, width);
        if !self.cache.contains_key(&key) {
            let lines = create();
            if Self::line_bytes(&lines) > Self::MAX_BYTES {
                return Cow::Owned(lines);
            }
            self.insert(element_index, width, lines);
        }
        Cow::Borrowed(&self.cache.get(&key).expect("admitted layout").lines)
    }

    fn line_bytes(lines: &Vec<Line<'static>>) -> usize {
        let mut bytes = lines.capacity().saturating_mul(std::mem::size_of::<Line>());
        for line in lines {
            bytes = bytes.saturating_add(
                line.spans
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ratatui::text::Span>()),
            );
            for span in &line.spans {
                if let Cow::Owned(text) = &span.content {
                    bytes = bytes.saturating_add(text.capacity());
                }
            }
        }
        bytes
    }

    pub fn check_layout(&mut self, width: u16, color: bool) {
        self.check_width(width);
        if self.last_color != Some(color) {
            self.clear();
            self.last_color = Some(color);
        }
    }

    fn clear(&mut self) {
        self.cache.clear();
        self.order.clear();
        self.retained_bytes = 0;
    }

    /// Invalidate cache if terminal width changed
    pub fn check_width(&mut self, width: u16) {
        if width != self.last_width {
            self.clear();
            self.last_width = width;
        }
    }
}
