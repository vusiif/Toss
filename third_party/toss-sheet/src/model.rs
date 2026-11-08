//! Toss-owned range mapping. Upstream calamine remains the format parser.
use super::{CellValue, SheetData};
use calamine::{Data, Range};
use std::sync::Arc;

/// Parsed ranges are retained; only display cells are materialized on demand.
/// This is not a streaming XLSX parser or a bound on calamine's allocations.
pub struct LazySheetData {
    range: Arc<Range<Data>>,
    formulas: Option<Range<String>>,
    origin: (u32, u32),
    header_rows: usize,
    pub headers: Vec<String>,
    pub width: usize,
    pub height: usize,
    /// Absolute zero-based workbook coordinate of the first data cell.
    pub data_start: (u32, u32),
}

impl LazySheetData {
    pub fn from_range_with_formulas(range: Range<Data>, formulas: Option<Range<String>>) -> Self {
        Self::with_options(range, formulas, false)
    }
    pub fn with_options(
        range: Range<Data>,
        formulas: Option<Range<String>>,
        no_header: bool,
    ) -> Self {
        Self::with_shared(Arc::new(range), formulas, no_header)
    }
    pub fn with_shared(
        range: Arc<Range<Data>>,
        formulas: Option<Range<String>>,
        no_header: bool,
    ) -> Self {
        // Include formula-only cells with missing cached values without expanding
        // or cloning the data range just to align its coordinate system.
        let bounds = [
            range.start().zip(range.end()),
            formulas.as_ref().and_then(|f| f.start().zip(f.end())),
        ];
        let bounds: Vec<_> = bounds.into_iter().flatten().collect();
        let origin = bounds
            .iter()
            .map(|(a, _)| *a)
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1)))
            .unwrap_or((0, 0));
        let end = bounds
            .iter()
            .map(|(_, b)| *b)
            .reduce(|a, b| (a.0.max(b.0), a.1.max(b.1)))
            .unwrap_or(origin);
        let (total_height, width) = if bounds.is_empty() {
            (0, 0)
        } else {
            (
                (end.0 - origin.0) as usize + 1,
                (end.1 - origin.1) as usize + 1,
            )
        };
        let header_rows = usize::from(!no_header && total_height > 0);
        let headers = (0..width)
            .map(|c| {
                if header_rows == 0 {
                    column_name(origin.1 as usize + c)
                } else {
                    range
                        .get_value((origin.0, origin.1 + c as u32))
                        .map(SheetData::cell_to_string)
                        .unwrap_or_default()
                }
            })
            .collect();
        Self {
            range,
            formulas,
            origin,
            header_rows,
            headers,
            width,
            height: total_height.saturating_sub(header_rows),
            data_start: (origin.0 + header_rows as u32, origin.1),
        }
    }
    pub fn get_rows(
        &self,
        start: usize,
        count: usize,
    ) -> (Vec<Vec<CellValue>>, Vec<Vec<Option<String>>>) {
        let start = start.min(self.height);
        let end = start.saturating_add(count).min(self.height);
        let mut values = Vec::with_capacity(end - start);
        let mut formulas = Vec::with_capacity(end - start);
        for row in start..end {
            let absolute_row = self.origin.0 + (row + self.header_rows) as u32;
            let mut cells = Vec::with_capacity(self.width);
            let mut expressions = Vec::with_capacity(self.width);
            for col in 0..self.width {
                let pos = (absolute_row, self.origin.1 + col as u32);
                cells.push(
                    self.range
                        .get_value(pos)
                        .map(SheetData::datatype_to_cellvalue)
                        .unwrap_or(CellValue::Empty),
                );
                expressions.push(
                    self.formulas
                        .as_ref()
                        .and_then(|f| f.get_value(pos))
                        .filter(|s| !s.is_empty())
                        .cloned(),
                );
            }
            values.push(cells);
            formulas.push(expressions);
        }
        (values, formulas)
    }
    pub fn to_sheet_data(self) -> SheetData {
        let (rows, formulas) = self.get_rows(0, self.height);
        SheetData {
            headers: self.headers,
            rows,
            formulas,
            width: self.width,
            height: self.height,
            data_start: self.data_start,
        }
    }
}

pub fn column_name(mut index: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    chars.into_iter().rev().collect()
}

#[cfg(test)]
mod sharing_tests {
    use super::*;
    #[test]
    fn csv_ranges_share_backing_storage_instead_of_cloning_cells() {
        let mut range = Range::new((0, 0), (1, 0));
        range.set_value((0, 0), Data::String("Header".into()));
        range.set_value((1, 0), Data::String("001".into()));
        let shared = Arc::new(range);
        let first = LazySheetData::with_shared(shared.clone(), None, false);
        let second = LazySheetData::with_shared(shared.clone(), None, true);
        assert!(Arc::ptr_eq(&first.range, &second.range));
        assert_eq!(Arc::strong_count(&shared), 3);
        assert_eq!(first.get_rows(0, 1).0[0][0].to_raw_string(), "001");
        assert_eq!(second.height, 2);
    }
}
