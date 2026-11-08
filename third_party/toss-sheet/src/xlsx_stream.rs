//! Bound cell collection before dense Range construction using calamine's
//! public stream API, without an extra worksheet XML parser.
use anyhow::{Context, Result, bail};
use calamine::{Cell, Data, DataRef, Range, Xlsx};
use std::io::{Read, Seek};
const MAX_CELLS: usize = 4_000_000;
const MAX_TEXT: usize = 1024 * 1024;

/// Prepare display styles on demand without building a second data range.
pub fn collect_styles<R: Read + Seek>(
    book: &mut Xlsx<R>,
    name: &str,
    catalog: &crate::colors::Catalog,
) -> Result<std::collections::HashMap<(u32, u32), usize>> {
    let mut styles = std::collections::HashMap::new();
    let mut reader = book.worksheet_cells_reader(name)?;
    let default = catalog.get(0);
    let mut count = 0;
    while let Some(cell) = reader.next_cell()? {
        count += 1;
        if count > MAX_CELLS || text_len(cell.get_value()) > MAX_TEXT {
            bail!("worksheet style pass exceeds cell budget");
        }
        let index = reader.last_style_index().unwrap_or(0);
        if catalog.get(index) != default {
            styles.insert(cell.get_position(), index);
        }
    }
    Ok(styles)
}
#[derive(Default)]
struct Span {
    start: Option<(u32, u32)>,
    end: (u32, u32),
}
impl Span {
    fn add(&mut self, pos: (u32, u32)) -> Result<()> {
        if pos.0 >= 1_048_576 || pos.1 >= 16_384 {
            bail!("invalid Excel cell coordinate");
        }
        let low = self
            .start
            .map(|p| (p.0.min(pos.0), p.1.min(pos.1)))
            .unwrap_or(pos);
        let high = (self.end.0.max(pos.0), self.end.1.max(pos.1));
        if u64::from(high.0 - low.0 + 1) * u64::from(high.1 - low.1 + 1) > MAX_CELLS as u64 {
            bail!("worksheet occupied rectangle exceeds 4 million cell budget");
        }
        self.start = Some(low);
        self.end = high;
        Ok(())
    }
}
fn text_len(value: &DataRef<'_>) -> usize {
    match value {
        DataRef::String(s) | DataRef::DateTimeIso(s) | DataRef::DurationIso(s) => s.len(),
        DataRef::SharedString(s) => s.len(),
        _ => 0,
    }
}
type NumberStyles<'a> = (
    &'a crate::number_format::Catalog,
    &'a mut crate::number_format::StyleIndices,
);
pub fn load_formatted<R: Read + Seek>(
    book: &mut Xlsx<R>,
    name: &str,
    catalog: Option<&crate::colors::Catalog>,
    colors: &mut std::collections::HashMap<(u32, u32), usize>,
    mut numeric: Option<NumberStyles<'_>>,
) -> Result<(Range<Data>, Option<Range<String>>)> {
    let mut span = Span::default();
    let mut values = Vec::new();
    let mut ordered = true;
    let mut last = None;
    {
        let mut reader = book
            .worksheet_cells_reader(name)
            .with_context(|| format!("cannot read worksheet {name}"))?;
        let mut visited = 0;
        while let Some(cell) = reader.next_cell().context("cannot read worksheet cell")? {
            visited += 1;
            if visited > MAX_CELLS {
                bail!("worksheet cell count budget exceeded");
            }
            if let Some(catalog) = catalog {
                let style_index = reader.last_style_index().unwrap_or(0);
                let c = catalog.get(style_index);
                if c != catalog.get(0) {
                    colors.insert(cell.get_position(), style_index);
                }
            }
            let value = cell.get_value();
            if let Some((formats, indices)) = numeric.as_mut()
                && matches!(value, DataRef::Int(_) | DataRef::Float(_))
            {
                let index = reader.last_style_index().unwrap_or(0);
                indices.insert(
                    cell.get_position(),
                    if formats.needs_index(index) { index } else { 0 },
                );
            }
            if text_len(value) > MAX_TEXT {
                bail!("worksheet cell text exceeds 1 MiB budget");
            }
            if matches!(value, DataRef::Empty) {
                continue;
            }
            let pos = cell.get_position();
            span.add(pos)?;
            if last.is_some_and(|p| p > pos) {
                ordered = false;
            }
            last = Some(pos);
            values.push(Cell::new(pos, Data::from(value.clone())));
        }
    }
    if !ordered {
        values.sort_unstable_by_key(Cell::get_position);
    }
    let values = Range::from_sparse(values);
    let mut formulas = Vec::new();
    let mut ordered = true;
    let mut last = None;
    {
        let mut reader = book
            .worksheet_cells_reader(name)
            .with_context(|| format!("cannot read worksheet formulas {name}"))?;
        let mut visited = 0;
        while let Some(cell) = reader
            .next_formula()
            .context("cannot read worksheet formula")?
        {
            visited += 1;
            if visited > MAX_CELLS {
                bail!("worksheet formula cell count budget exceeded");
            }
            if let Some(catalog) = catalog {
                let style_index = reader.last_style_index().unwrap_or(0);
                let c = catalog.get(style_index);
                if c != catalog.get(0) {
                    colors.insert(cell.get_position(), style_index);
                }
            }
            let value = cell.get_value();
            if value.len() > MAX_TEXT {
                bail!("worksheet formula exceeds 1 MiB budget");
            }
            if value.is_empty() {
                continue;
            }
            let pos = cell.get_position();
            span.add(pos)?;
            if last.is_some_and(|p| p > pos) {
                ordered = false;
            }
            last = Some(pos);
            formulas.push(cell);
        }
    }
    if !ordered {
        formulas.sort_unstable_by_key(Cell::get_position);
    }
    Ok((values, Some(Range::from_sparse(formulas))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    #[test]
    fn span_checks_precede_dense_allocation_and_include_formulas() {
        let mut span = Span::default();
        span.add((0, 0)).unwrap();
        assert!(span.add((400000, 9)).is_err());
        for file in [
            "stream-large-value-rectangle.xlsx",
            "stream-large-formula-rectangle.xlsx",
            "stream-long-cell.xlsx",
        ] {
            let p = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(file);
            let mut workbook = crate::workbook::Workbook::open(p).unwrap();
            let error = workbook
                .load_sheet_lazy("Data")
                .err()
                .expect("must stop before dense range construction");
            assert!(
                error.to_string().contains(if file.contains("long-cell") {
                    "text exceeds"
                } else {
                    "rectangle exceeds"
                }),
                "{error:#}"
            );
        }
    }
    #[test]
    fn out_of_order_cells_are_sorted_before_range_construction() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stream-unordered.xlsx");
        let mut book = crate::workbook::Workbook::open(p).unwrap();
        let data = book.load_sheet("Data").unwrap();
        assert_eq!(data.rows[0][0].to_raw_string(), "Alpha");
        assert_eq!(data.rows[1][0].to_raw_string(), "Beta");
    }
}
