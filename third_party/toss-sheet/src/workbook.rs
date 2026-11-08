//! Toss-owned workbook access; UI conventions derive from xleak.
use anyhow::{Context, Result, anyhow};
use calamine::{Data, ExcelDateTime, Range, Reader, Sheets, Table, open_workbook_auto_from_rs};
use std::{io::Cursor, path::Path, sync::Arc};
#[path = "model.rs"]
mod model;
pub use model::LazySheetData;

pub struct Workbook {
    sheets: Option<Sheets<Cursor<Arc<[u8]>>>>,
    csv: Option<Arc<Range<Data>>>,
    source_bytes: Option<Arc<[u8]>>,
    csv_name: String,
    no_header: bool,
    catalog: Option<crate::colors::Catalog>,
    // Store shared style indices, not a copy of four borders for every cell.
    cell_colors: std::collections::HashMap<(u32, u32), usize>,
    number_catalog: Option<crate::number_format::Catalog>,
    number_styles: crate::number_format::StyleIndices,
}
impl Workbook {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_options(path, false, None)
    }
    pub fn open_with_options(
        path: impl AsRef<Path>,
        no_header: bool,
        delimiter: Option<u8>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if extension.eq_ignore_ascii_case("csv") || extension.eq_ignore_ascii_case("tsv") {
            let sep = delimiter.unwrap_or(if extension.eq_ignore_ascii_case("tsv") {
                b'\t'
            } else {
                b','
            });
            return Ok(Self {
                sheets: None,
                source_bytes: None,
                csv: Some(Arc::new(crate::csv_input::read(path, sep)?)),
                csv_name: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                no_header,
                catalog: None,
                cell_colors: Default::default(),
                number_catalog: None,
                number_styles: Default::default(),
            });
        }
        if delimiter.is_some() {
            anyhow::bail!("a delimiter applies only to CSV/TSV");
        }
        let bytes = crate::admission::snapshot(path)?;
        let reader = Cursor::new(bytes.clone());
        let sheets = if ["xlsx", "xlsm", "xlam"]
            .iter()
            .any(|e| extension.eq_ignore_ascii_case(e))
        {
            // The auto reader discards individual parser errors while trying formats.
            Sheets::Xlsx(calamine::Xlsx::new(reader).context("Failed to open XLSX workbook")?)
        } else {
            open_workbook_auto_from_rs(reader).context("Failed to open workbook")?
        };
        Ok(Self {
            sheets: Some(sheets),
            source_bytes: Some(bytes),
            csv: None,
            csv_name: String::new(),
            no_header,
            catalog: None,
            cell_colors: Default::default(),
            number_catalog: None,
            number_styles: Default::default(),
        })
    }

    pub fn enable_colors(&mut self) {
        if matches!(self.sheets, Some(Sheets::Xlsx(_))) {
            match crate::colors::Catalog::load(self.source_bytes.as_ref().unwrap()) {
                Ok(c) => self.catalog = Some(c),
                Err(e) => eprintln!("Excel colors unavailable; using terminal theme: {e}"),
            }
        }
    }
    pub(crate) fn enable_number_formats(&mut self) {
        if matches!(self.sheets, Some(Sheets::Xlsx(_))) && self.number_catalog.is_none() {
            match crate::number_format::Catalog::load(self.source_bytes.as_ref().unwrap()) {
                Ok(c) if c.active() => self.number_catalog = Some(c),
                Ok(_) => {}
                Err(e) => eprintln!("Excel number formats unavailable; showing raw values: {e}"),
            }
        }
    }
    pub(crate) fn display_value(&self, row: usize, col: usize, value: &CellValue) -> String {
        self.number_catalog
            .as_ref()
            .and_then(|catalog| {
                catalog.render(
                    self.number_styles
                        .get(&(row as u32, col as u32))
                        .copied()
                        .unwrap_or(0),
                    value,
                )
            })
            .unwrap_or_else(|| value.to_string())
    }
    pub fn uses_headers(&self) -> bool {
        !self.no_header
    }
    pub fn prepare_colors(&mut self, name: &str) -> Result<()> {
        if self.catalog.is_none() {
            self.enable_colors();
        }
        if let (Some(Sheets::Xlsx(book)), Some(catalog)) = (&mut self.sheets, &self.catalog) {
            self.cell_colors = crate::xlsx_stream::collect_styles(book, name, catalog)?;
        }
        Ok(())
    }
    pub fn color_at(&self, row: usize, col: usize) -> Option<crate::colors::CellColors> {
        self.catalog.as_ref().map(|c| {
            self.cell_colors
                .get(&(row as u32, col as u32))
                .copied()
                .map_or_else(|| c.get(0), |index| c.get(index))
        })
    }
    pub fn sheet_names(&self) -> Vec<String> {
        self.sheets
            .as_ref()
            .map(|s| s.sheet_names())
            .unwrap_or_else(|| vec![self.csv_name.clone()])
    }
    pub fn load_sheet(&mut self, name: &str) -> Result<SheetData> {
        Ok(self.load_sheet_lazy(name)?.to_sheet_data())
    }
    pub fn load_sheet_lazy(&mut self, name: &str) -> Result<LazySheetData> {
        self.cell_colors.clear();
        self.number_styles.clear();
        let (range, formulas) = if let Some(sheets) = &mut self.sheets {
            let (range, formulas) = match sheets {
                Sheets::Xlsx(book) => crate::xlsx_stream::load_formatted(
                    book,
                    name,
                    self.catalog.as_ref(),
                    &mut self.cell_colors,
                    self.number_catalog
                        .as_ref()
                        .map(|c| (c, &mut self.number_styles)),
                )?,
                other => {
                    let range = other
                        .worksheet_range(name)
                        .with_context(|| format!("Sheet '{name}' not found"))?;
                    let formulas = other.worksheet_formula(name).ok();
                    (range, formulas)
                }
            };
            (Arc::new(range), formulas)
        } else {
            if name != self.csv_name {
                anyhow::bail!("Sheet not found");
            }
            (
                self.csv.as_ref().expect("CSV backend invariant").clone(),
                None,
            )
        };
        crate::admission::check_dense_ranges(
            range.start().zip(range.end()),
            formulas.as_ref().and_then(|f| f.start().zip(f.end())),
        )?;
        let mut data = LazySheetData::with_shared(range.clone(), formulas, self.no_header);
        if !self.no_header
            && self.number_catalog.is_some()
            && let Some(row) = data.data_start.0.checked_sub(1)
        {
            for (col, header) in data.headers.iter_mut().enumerate() {
                if let Some(value) = range.get_value((row, data.data_start.1 + col as u32)) {
                    *header = self.display_value(
                        row as usize,
                        data.data_start.1 as usize + col,
                        &SheetData::datatype_to_cellvalue(value),
                    );
                }
            }
        }
        Ok(data)
    }
    // ===== Table API (Xlsx only) =====

    /// Load table metadata from the workbook (Xlsx only)
    pub fn load_tables(&mut self) -> Result<()> {
        if let Some(bytes) = &self.source_bytes {
            crate::admission::preflight(bytes)?;
        }
        match &mut self.sheets {
            Some(Sheets::Xlsx(xlsx)) => xlsx
                .load_tables()
                .context("Failed to load table metadata")
                .map_err(|e| anyhow!("{e}")),
            _ => Err(anyhow!("Tables are only supported in .xlsx files")),
        }
    }

    /// Get all table names in the workbook (Xlsx only)
    pub fn table_names(&self) -> Result<Vec<String>> {
        match &self.sheets {
            Some(Sheets::Xlsx(xlsx)) => {
                Ok(xlsx.table_names().iter().map(|s| (*s).clone()).collect())
            }
            _ => Err(anyhow!("Tables are only supported in .xlsx files")),
        }
    }

    /// Get table names in a specific sheet (Xlsx only)
    pub fn table_names_in_sheet(&self, sheet_name: &str) -> Result<Vec<String>> {
        match &self.sheets {
            Some(Sheets::Xlsx(xlsx)) => Ok(xlsx
                .table_names_in_sheet(sheet_name)
                .iter()
                .map(|s| (*s).clone())
                .collect()),
            _ => Err(anyhow!("Tables are only supported in .xlsx files")),
        }
    }

    /// Get table data by name (Xlsx only)
    pub fn table_by_name(&mut self, table_name: &str) -> Result<TableData> {
        match &mut self.sheets {
            Some(Sheets::Xlsx(xlsx)) => {
                let table = xlsx
                    .table_by_name(table_name)
                    .map_err(|e| anyhow!("Table '{table_name}' not found: {e}"))?;

                Ok(TableData::from_calamine_table(table))
            }
            _ => Err(anyhow!("Tables are only supported in .xlsx files")),
        }
    }
}

/// Eagerly-loaded sheet data (loads all rows immediately)
#[derive(Debug, Clone)]
pub struct SheetData {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
    pub formulas: Vec<Vec<Option<String>>>, // Parallel structure to rows with formulas
    pub width: usize,
    pub height: usize,
    pub data_start: (u32, u32),
}

#[derive(Debug, Clone)]
pub enum CellValue {
    Empty,
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Error(String),
    DateTime(ExcelDateTime), // Preserve epoch and duration metadata.
}

impl CellValue {
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        matches!(self, CellValue::Empty)
    }

    #[allow(dead_code)]
    pub fn is_numeric(&self) -> bool {
        matches!(self, CellValue::Int(_) | CellValue::Float(_))
    }

    /// Returns unformatted value (for export/clipboard)
    pub fn to_raw_string(&self) -> String {
        match self {
            CellValue::Empty => String::new(),
            CellValue::String(s) => s.clone(),
            CellValue::Int(i) => i.to_string(),
            CellValue::Float(val) => {
                if val.fract() == 0.0 {
                    format!("{val:.0}")
                } else {
                    val.to_string()
                }
            }
            CellValue::Bool(b) => b.to_string(),
            CellValue::Error(e) => format!("#{e}"),
            CellValue::DateTime(dt) => format_datetime(dt),
        }
    }
}

/// Excel Table data
#[derive(Debug, Clone)]
pub struct TableData {
    pub name: String,
    pub sheet_name: String,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<CellValue>>,
}

impl TableData {
    pub fn from_calamine_table(table: Table<Data>) -> Self {
        let name = table.name().to_string();
        let sheet_name = table.sheet_name().to_string();
        let headers = table.columns().to_vec();

        let rows: Vec<Vec<CellValue>> = table
            .data()
            .rows()
            .map(|row| row.iter().map(SheetData::datatype_to_cellvalue).collect())
            .collect();

        Self {
            name,
            sheet_name,
            headers,
            rows,
        }
    }
}

impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CellValue::Empty => write!(f, ""),
            CellValue::String(s) => write!(f, "{s}"),
            CellValue::Int(i) => write!(f, "{i}"),
            CellValue::Float(value) => write!(f, "{value}"),
            CellValue::Bool(b) => {
                // Use lowercase for booleans
                write!(f, "{}", if *b { "true" } else { "false" })
            }
            CellValue::Error(e) => write!(f, "ERROR: {e}"),
            CellValue::DateTime(d) => write!(f, "{}", format_datetime(d)),
        }
    }
}

impl SheetData {
    pub fn from_range_with_formulas(range: Range<Data>, formulas: Option<Range<String>>) -> Self {
        LazySheetData::from_range_with_formulas(range, formulas).to_sheet_data()
    }
    fn cell_to_string(cell: &Data) -> String {
        match cell {
            Data::Empty => String::new(),
            Data::String(s) => s.clone(),
            Data::Int(i) => i.to_string(),
            Data::Float(f) => {
                if f.fract() == 0.0 {
                    format!("{f:.0}")
                } else {
                    f.to_string()
                }
            }
            Data::Bool(b) => b.to_string(),
            Data::Error(e) => format!("ERROR: {e:?}"),
            Data::DateTime(d) => format_datetime(d),
            Data::DateTimeIso(s) => s.clone(),
            Data::DurationIso(s) => s.clone(),
        }
    }

    fn datatype_to_cellvalue(cell: &Data) -> CellValue {
        match cell {
            Data::Empty => CellValue::Empty,
            Data::String(s) => CellValue::String(s.clone()),
            Data::Int(i) => CellValue::Int(*i),
            Data::Float(f) => CellValue::Float(*f),
            Data::Bool(b) => CellValue::Bool(*b),
            Data::Error(e) => CellValue::Error(format!("{e:?}")),
            Data::DateTime(d) => CellValue::DateTime(*d),
            Data::DateTimeIso(s) => CellValue::String(s.clone()),
            Data::DurationIso(s) => CellValue::String(s.clone()),
        }
    }
}

fn format_datetime(value: &ExcelDateTime) -> String {
    let raw = value.as_f64();
    if !raw.is_finite() || raw.abs() > 4_000_000.0 {
        return format!("Date[{raw}]");
    }
    if value.is_duration() {
        if let Some(duration) = value.as_duration() {
            let ms = duration.num_milliseconds();
            let sign = if ms < 0 { "-" } else { "" };
            let abs = ms.unsigned_abs();
            return format!(
                "{sign}{}:{:02}:{:02}.{:03}",
                abs / 3_600_000,
                (abs / 60_000) % 60,
                (abs / 1000) % 60,
                abs % 1000
            );
        }
    } else if let Some(dt) = value.as_datetime() {
        return if dt.time() == chrono::NaiveTime::MIN {
            dt.format("%Y-%m-%d").to_string()
        } else if dt.and_utc().timestamp_subsec_millis() == 0 {
            dt.format("%Y-%m-%d %H:%M:%S").to_string()
        } else {
            dt.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
        };
    }
    format!("Date[{raw}]")
}

#[cfg(test)]
mod tests {
    #[test]
    fn general_numeric_display_preserves_precision_without_invented_grouping() {
        for value in [
            0.004,
            -0.0004,
            1.2345678901234567,
            0.30000000000000004,
            f64::MIN_POSITIVE,
            f64::MAX,
            -0.0,
        ] {
            let cell = super::CellValue::Float(value);
            let display = cell.to_string();
            assert_eq!(display.parse::<f64>().unwrap().to_bits(), value.to_bits());
            assert!(!display.contains(','));
            assert_eq!(
                cell.to_raw_string().parse::<f64>().unwrap().to_bits(),
                value.to_bits()
            );
        }
        assert_eq!(
            super::CellValue::Int(i64::MIN).to_string(),
            i64::MIN.to_string()
        );
        assert_eq!(
            super::CellValue::String("001234".into()).to_string(),
            "001234"
        );
    }
    use super::*;

    #[test]
    fn test_cellvalue_display_integer() {
        let val = CellValue::Int(1234567);
        assert_eq!(val.to_string(), "1234567");
    }

    #[test]
    fn test_cellvalue_display_negative_integer() {
        let val = CellValue::Int(-1234567);
        assert_eq!(val.to_string(), "-1234567");
    }

    #[test]
    fn test_cellvalue_display_float() {
        let val = CellValue::Float(1234567.89);
        assert_eq!(val.to_string(), "1234567.89");
    }

    #[test]
    fn test_cellvalue_display_float_whole_number() {
        let val = CellValue::Float(1000.0);
        assert_eq!(val.to_string(), "1000");
    }

    #[test]
    fn test_cellvalue_display_boolean() {
        assert_eq!(CellValue::Bool(true).to_string(), "true");
        assert_eq!(CellValue::Bool(false).to_string(), "false");
    }

    #[test]
    fn test_cellvalue_display_string() {
        let val = CellValue::String("Hello, World!".to_string());
        assert_eq!(val.to_string(), "Hello, World!");
    }

    #[test]
    fn test_cellvalue_display_empty() {
        let val = CellValue::Empty;
        assert_eq!(val.to_string(), "");
    }

    #[test]
    fn test_cellvalue_display_error() {
        let val = CellValue::Error("DIV/0!".to_string());
        assert_eq!(val.to_string(), "ERROR: DIV/0!");
    }

    #[test]
    fn test_cellvalue_to_raw_string_integer() {
        let val = CellValue::Int(1234567);
        assert_eq!(val.to_raw_string(), "1234567");
    }

    #[test]
    fn test_cellvalue_to_raw_string_float() {
        let val = CellValue::Float(123.45);
        assert_eq!(val.to_raw_string(), "123.45");
    }

    #[test]
    fn test_cellvalue_to_raw_string_large_float() {
        // Regression: large whole-number floats were exported with thousands separators
        // (e.g. "18,441,600,422") making CSV/text output unparseable as numbers (#34)
        let val = CellValue::Float(18_441_600_422.0);
        assert_eq!(val.to_raw_string(), "18441600422");
        // General display must not invent grouping absent from the source format.
        assert_eq!(val.to_string(), "18441600422");
    }

    #[test]
    fn test_cellvalue_to_raw_string_large_int() {
        let val = CellValue::Int(18_441_600_422);
        assert_eq!(val.to_raw_string(), "18441600422");
        assert_eq!(val.to_string(), "18441600422");
    }

    #[test]
    fn test_cellvalue_is_empty() {
        assert!(CellValue::Empty.is_empty());
        assert!(!CellValue::Int(0).is_empty());
        assert!(!CellValue::String("".to_string()).is_empty());
    }

    #[test]
    fn test_cellvalue_is_numeric() {
        assert!(CellValue::Int(123).is_numeric());
        assert!(CellValue::Float(123.45).is_numeric());
        assert!(!CellValue::String("123".to_string()).is_numeric());
        assert!(!CellValue::Empty.is_numeric());
    }

    #[test]
    fn test_datetime_display() {
        // Excel date: January 1, 1900 is day 1
        let val = CellValue::DateTime(ExcelDateTime::new(
            1.0,
            calamine::ExcelDateTimeType::DateTime,
            false,
        ));
        let display = val.to_string();
        // Should contain a date in YYYY-MM-DD format
        assert!(display.contains("1900") || display.contains("1899"));
    }

    #[test]
    fn test_datetime_with_time() {
        // Excel datetime with time component
        // Day 1 + 0.5 = 12:00:00 on Jan 1, 1900
        let val = CellValue::DateTime(ExcelDateTime::new(
            1.5,
            calamine::ExcelDateTimeType::DateTime,
            false,
        ));
        let display = val.to_string();
        // Should contain both date and time
        assert!(display.contains(":"));
        assert!(display.len() > 10); // Date + time is longer than just date
    }

    #[test]
    fn test_workbook_open_real_file() {
        // Test with actual test file if it exists
        if let Ok(wb) = Workbook::open("tests/fixtures/test_data.xlsx") {
            let sheet_names = wb.sheet_names();
            assert!(!sheet_names.is_empty(), "Should have at least one sheet");
        }
        // If file doesn't exist, test passes (integration test needs real file)
    }

    #[test]
    fn test_sheet_data_structure() {
        // Test SheetData structure can be created
        let sheet = SheetData {
            headers: vec!["Name".to_string(), "Age".to_string()],
            rows: vec![
                vec![CellValue::String("Alice".to_string()), CellValue::Int(30)],
                vec![CellValue::String("Bob".to_string()), CellValue::Int(25)],
            ],
            formulas: vec![vec![None, None], vec![None, None]],
            width: 2,
            height: 2,
            data_start: (1, 0),
        };

        assert_eq!(sheet.width, 2);
        assert_eq!(sheet.height, 2);
        assert_eq!(sheet.headers.len(), 2);
        assert_eq!(sheet.rows.len(), 2);
    }
}
