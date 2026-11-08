use crate::workbook::{CellValue, SheetData, Workbook};
use calamine::{Data, ExcelDateTime, ExcelDateTimeType, Range};
use std::{
    io::{self, Write},
    path::Path,
};
fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}
#[test]
fn actual_1904_workbook_and_offset_formula() {
    let data = Workbook::open(fixture("date1904.xlsx"))
        .unwrap()
        .load_sheet("Dates")
        .unwrap();
    assert_eq!(data.rows[0][0].to_raw_string(), "2026-11-02");
    let data = Workbook::open(fixture("offset.xlsx"))
        .unwrap()
        .load_sheet("Offset")
        .unwrap();
    assert_eq!(data.formulas[0][1].as_deref(), Some("1+2"));
    assert_eq!(data.data_start, (5, 3));
}
#[test]
fn no_header_preserves_first_row_and_actual_origin() {
    let mut workbook = Workbook::open_with_options(fixture("offset.xlsx"), true, None).unwrap();
    let data = workbook.load_sheet_lazy("Offset").unwrap();
    assert_eq!(data.headers, ["D", "E"]);
    assert_eq!(data.height, 2);
    assert_eq!(data.data_start, (4, 3));
    let (rows, formulas) = data.get_rows(0, usize::MAX);
    assert_eq!(rows[0][0].to_raw_string(), "Header");
    assert_eq!(formulas[1][1].as_deref(), Some("1+2"));
    assert!(data.get_rows(usize::MAX, usize::MAX).0.is_empty());
}
#[test]
fn formula_without_cached_value_is_included_in_bounds() {
    let mut data = Range::new((4, 3), (4, 3));
    data.set_value((4, 3), Data::String("Header".into()));
    let mut formulas = Range::new((9, 5), (9, 5));
    formulas.set_value((9, 5), "1+2".into());
    let sheet = SheetData::from_range_with_formulas(data, Some(formulas));
    assert_eq!((sheet.height, sheet.width), (5, 3));
    assert_eq!(sheet.formulas[4][2].as_deref(), Some("1+2"));
}
#[test]
fn export_escapes_headers_sheet_and_all_text() {
    let sheet = Workbook::open(fixture("basic.xlsx"))
        .unwrap()
        .load_sheet("Quote\"Sheet")
        .unwrap();
    let mut out = Vec::new();
    crate::export::json(&mut out, &sheet, "Quote\"Sheet").unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["sheet"], "Quote\"Sheet");
    assert_eq!(value["headers"][0], "head,\"quoted\"");
    assert_eq!(value["data"][0][0], "back\\slash\nand\ttab");
    out.clear();
    crate::export::csv(&mut out, &sheet).unwrap();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(out.as_slice());
    let rows: Vec<_> = reader.records().collect::<Result<_, _>>().unwrap();
    assert_eq!(rows[0].get(0), Some(sheet.headers[0].as_str()));
    assert_eq!(rows[0].get(1), Some("line\nbreak"));
    assert_eq!(rows[1].get(0), Some("back\\slash\nand\ttab"));
}
struct FailAfter(usize);
impl Write for FailAfter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0 == 0 {
            return Err(io::Error::other("injected output failure"));
        }
        let n = bytes.len().min(self.0);
        self.0 -= n;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("injected flush failure"))
    }
}
#[test]
fn serializers_propagate_short_write_and_flush_errors() {
    let sheet = Workbook::open(fixture("basic.xlsx"))
        .unwrap()
        .load_sheet("Data")
        .unwrap();
    for cap in [0, 5, usize::MAX] {
        assert!(crate::export::csv(FailAfter(cap), &sheet).is_err());
        assert!(crate::export::json(FailAfter(cap), &sheet, "Data").is_err());
    }
}
#[test]
fn datetime_metadata_rounding_duration_and_invalid_values() {
    let dt = |v, t, epoch| CellValue::DateTime(ExcelDateTime::new(v, t, epoch));
    assert_eq!(
        dt(0.0, ExcelDateTimeType::DateTime, true).to_raw_string(),
        "1904-01-01"
    );
    assert_eq!(
        dt(1.9999999999, ExcelDateTimeType::DateTime, false).to_raw_string(),
        "1900-01-02"
    );
    assert_eq!(
        dt(1.5, ExcelDateTimeType::TimeDelta, false).to_raw_string(),
        "36:00:00.000"
    );
    assert!(
        dt(f64::INFINITY, ExcelDateTimeType::DateTime, false)
            .to_raw_string()
            .starts_with("Date[")
    );
    assert!(
        dt(1e300, ExcelDateTimeType::DateTime, false)
            .to_raw_string()
            .starts_with("Date[")
    );
}
#[test]
fn csv_no_header_multiline_bom_strings_and_tsv() {
    let data = Workbook::open_with_options(fixture("edge.csv"), true, None)
        .unwrap()
        .load_sheet("edge")
        .unwrap();
    assert_eq!(data.headers, ["A", "B", "C"]);
    assert_eq!(data.height, 3);
    assert_eq!(data.rows[0][0].to_raw_string(), "00123");
    assert_eq!(data.rows[0][1].to_raw_string(), "comma,quote\"\nnext");
    assert_eq!(data.rows[1][2].to_raw_string(), "tail");
    assert_eq!(data.rows[2][2].to_raw_string(), "");
    assert_eq!(
        Workbook::open(fixture("edge.tsv"))
            .unwrap()
            .load_sheet("edge")
            .unwrap()
            .rows[0][1]
            .to_raw_string(),
        "中文"
    );
    assert_eq!(
        Workbook::open_with_options(fixture("semicolon.csv"), false, Some(b';'))
            .unwrap()
            .load_sheet("semicolon")
            .unwrap()
            .rows[0][0]
            .to_raw_string(),
        "001"
    );
}
#[test]
fn csv_refuses_invalid_utf8_and_oversized_fields() {
    assert!(Workbook::open(fixture("invalid-utf8.csv")).is_err());
    assert!(Workbook::open(fixture("large-field.csv")).is_err());
    assert!(Workbook::open_with_options(fixture("edge.csv"), false, Some(b'\n')).is_err());
}

#[test]
fn custom_worksheet_path_keeps_values_and_formulas() {
    let mut book =
        Workbook::open(fixture("custom-sheet-path.xlsx")).unwrap_or_else(|e| panic!("{e:#}"));
    let data = book.load_sheet("Data").unwrap();
    assert_eq!(data.rows[0][0].to_raw_string(), "Alpha");
    assert_eq!(data.formulas[0][3].as_deref(), Some("B2*2"));
}

#[test]
fn custom_relationship_paths_resolve_within_package() {
    for name in [
        "custom-relative.xlsx",
        "custom-normalized.xlsx",
        "custom-two-sheets.xlsx",
    ] {
        let mut book = Workbook::open(fixture(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        let data = book.load_sheet("Data").unwrap();
        assert_eq!(data.rows[0][0].to_raw_string(), "Alpha", "{name}");
        assert_eq!(data.formulas[0][3].as_deref(), Some("B2*2"), "{name}");
        assert_eq!(
            book.load_sheet("Second").unwrap().rows[0][0].to_raw_string(),
            "second-only"
        );
    }
}
#[test]
fn invalid_relationships_keep_actionable_errors() {
    for name in [
        "custom-root-escape.xlsx",
        "custom-backslash.xlsx",
        "custom-colon.xlsx",
    ] {
        let err = Workbook::open(fixture(name))
            .err()
            .expect("invalid target must fail");
        assert!(
            format!("{err:#}").contains("sheet:target"),
            "{name}: {err:#}"
        );
    }
    let mut book = Workbook::open(fixture("custom-missing.xlsx")).unwrap();
    let err = book
        .load_sheet("Data")
        .expect_err("missing target must fail");
    assert!(format!("{err:#}").contains("missing"), "{err:#}");
}

#[test]
fn actual_formula_cache_distinguishes_absent_and_empty_string() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/explicit-empty-formula-cache.xlsx");
    let data = Workbook::open(path).unwrap().load_sheet("Cached").unwrap();
    assert!(matches!(data.rows[0][1], CellValue::Empty));
    assert!(matches!(&data.rows[1][1], CellValue::String(s) if s.is_empty()));
    assert_eq!(data.formulas[0][1].as_deref(), Some("A2*2"));
    assert_eq!(data.formulas[1][1].as_deref(), Some("IF(1,\"\",\"\")"));
}
#[test]
fn empty_sheet_csv_has_no_phantom_column() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/style-only.xlsx");
    let data = Workbook::open(path).unwrap().load_sheet("Empty").unwrap();
    let mut out = Vec::new();
    crate::export::csv(&mut out, &data).unwrap();
    assert!(out.is_empty());
}
