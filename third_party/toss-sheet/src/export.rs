//! Toss-owned serializers: escaping and write errors are handled by libraries.
use crate::workbook::{CellValue, SheetData};
use anyhow::Result;
use serde::{Serialize, Serializer, ser::SerializeSeq};
use std::io::Write;
struct Cell<'a>(&'a CellValue);
impl Serialize for Cell<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            CellValue::Empty => s.serialize_none(),
            CellValue::Int(v) => s.serialize_i64(*v),
            CellValue::Float(v) if v.is_finite() => s.serialize_f64(*v),
            CellValue::Float(_) => s.serialize_none(),
            CellValue::Bool(v) => s.serialize_bool(*v),
            CellValue::String(v) => s.serialize_str(v),
            value => s.serialize_str(&value.to_raw_string()),
        }
    }
}
struct Row<'a>(&'a [CellValue]);
impl Serialize for Row<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for cell in self.0 {
            seq.serialize_element(&Cell(cell))?;
        }
        seq.end()
    }
}
struct Rows<'a>(&'a [Vec<CellValue>]);
impl Serialize for Rows<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for row in self.0 {
            seq.serialize_element(&Row(row))?;
        }
        seq.end()
    }
}
#[derive(Serialize)]
struct Document<'a> {
    sheet: &'a str,
    rows: usize,
    columns: usize,
    headers: &'a [String],
    data: Rows<'a>,
}
pub fn json(mut writer: impl Write, data: &SheetData, sheet: &str) -> Result<()> {
    serde_json::to_writer(
        &mut writer,
        &Document {
            sheet,
            rows: data.height,
            columns: data.width,
            headers: &data.headers,
            data: Rows(&data.rows),
        },
    )?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}
pub fn csv(mut writer: impl Write, data: &SheetData) -> Result<()> {
    if data.headers.is_empty() && data.rows.is_empty() {
        // A zero-column sheet must not become one empty CSV field.
        writer.flush()?;
        return Ok(());
    }
    let mut writer = csv::WriterBuilder::new().from_writer(writer);
    writer.write_record(&data.headers)?;
    for row in &data.rows {
        writer.write_record(row.iter().map(CellValue::to_raw_string))?;
    }
    writer.flush()?;
    Ok(())
}
