//! Prototype admission checks, not a hard RSS/CPU or complete parser sandbox.
use anyhow::{Context, Result, bail};
use quick_xml::{Reader, events::Event};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, BufReader, Cursor, Read},
    path::Path,
    sync::Arc,
};
const MAX_INPUT: u64 = 64 * 1024 * 1024;
const MAX_MEMBER: u64 = 64 * 1024 * 1024;
const MAX_TOTAL: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 4096;
const MAX_CELLS: u64 = 4_000_000;
const MAX_TEXT: usize = 1024 * 1024;

pub fn snapshot(path: &Path) -> Result<Arc<[u8]>> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    if len > MAX_INPUT {
        bail!("workbook input exceeds prototype 64 MiB admission limit");
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len as usize + 1)
        .context("cannot reserve workbook input")?;
    file.take(MAX_INPUT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_INPUT {
        bail!("workbook grew beyond input admission limit");
    }
    let bytes: Arc<[u8]> = bytes.into();
    preflight_streamed(&bytes)?;
    Ok(bytes)
}

struct Counted<'a, R> {
    reader: R,
    part: u64,
    total: &'a mut u64,
}
impl<R: Read> Read for Counted<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let remaining = (MAX_MEMBER - self.part).min(MAX_TOTAL - *self.total);
        let len = out.len().min((remaining + 1) as usize);
        let n = self.reader.read(&mut out[..len])?;
        if n as u64 > remaining {
            return Err(io::Error::other(
                "actual workbook expansion budget exceeded",
            ));
        }
        self.part += n as u64;
        *self.total += n as u64;
        Ok(n)
    }
}
pub fn preflight(bytes: &[u8]) -> Result<()> {
    preflight_mode(bytes, true)
}
/// Worksheet shape/text checks are performed by xlsx_stream during the actual
/// parser pass. Retain byte-counted expansion and allocation-sensitive metadata.
pub fn preflight_streamed(bytes: &[u8]) -> Result<()> {
    preflight_mode(bytes, false)
}
fn preflight_mode(bytes: &[u8], strict_worksheets: bool) -> Result<()> {
    if !bytes.starts_with(b"PK") {
        return Ok(());
    }
    // Reject excessive central-directory counts before ZipArchive constructs
    // its index. ZIP64 central directories are outside this prototype scope.
    let tail = bytes.len().saturating_sub(65_557);
    let end = (tail..bytes.len().saturating_sub(21))
        .rev()
        .find(|&i| {
            bytes.get(i..i + 4) == Some(b"PK\x05\x06")
                && i + 22 + usize::from(u16::from_le_bytes([bytes[i + 20], bytes[i + 21]]))
                    == bytes.len()
        })
        .context("missing ZIP end record")?;
    let members = u16::from_le_bytes([bytes[end + 10], bytes[end + 11]]);
    if members == u16::MAX {
        bail!("ZIP64 central directory is not admitted by this prototype");
    }
    if usize::from(members) > MAX_ENTRIES {
        bail!("workbook ZIP has too many members");
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).context("invalid workbook ZIP")?;
    if archive.len() > MAX_ENTRIES {
        bail!("workbook ZIP has too many members");
    }
    let mut declared = 0u64;
    let mut names = HashSet::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        if !names.insert(file.name().to_owned()) {
            bail!("duplicate workbook ZIP member");
        }
        if file.size() > MAX_MEMBER {
            bail!("declared workbook member exceeds 64 MiB expansion limit");
        }
        declared = declared
            .checked_add(file.size())
            .context("workbook expansion size overflow")?;
        if declared > MAX_TOTAL {
            bail!("declared workbook total exceeds 256 MiB expansion limit");
        }
    }
    let mut total = 0u64;
    let mut events = 0usize;
    let mut sheets = HashSet::new();
    let mut targets = HashMap::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let name = file.name().to_owned();
        let counted = Counted {
            reader: file,
            part: 0,
            total: &mut total,
        };
        let lower = name.to_ascii_lowercase();
        if !strict_worksheets && name.starts_with("xl/worksheets/") && lower.ends_with(".xml") {
            io::copy(&mut BufReader::new(counted), &mut io::sink())?;
        } else if lower.ends_with(".xml") || lower.ends_with(".rels") {
            scan_xml(
                BufReader::new(counted),
                &name,
                &mut events,
                &mut sheets,
                &mut targets,
            )?;
        } else {
            io::copy(&mut BufReader::new(counted), &mut io::sink())?;
        }
    }
    for id in sheets {
        let (target, external) = targets.get(&id).context("missing worksheet relationship")?;
        if *external || !target.to_ascii_lowercase().ends_with(".xml") {
            bail!("worksheet target must be an internal XML part for admission checks");
        }
    }
    Ok(())
}

pub fn check_dense_ranges(
    a: Option<((u32, u32), (u32, u32))>,
    b: Option<((u32, u32), (u32, u32))>,
) -> Result<()> {
    let mut low = None::<(u32, u32)>;
    let mut high = (0u32, 0u32);
    for (start, end) in [a, b].into_iter().flatten() {
        low = Some(
            low.map(|v| (v.0.min(start.0), v.1.min(start.1)))
                .unwrap_or(start),
        );
        high = (high.0.max(end.0), high.1.max(end.1));
    }
    if let Some(low) = low
        && (u64::from(high.0) - u64::from(low.0) + 1)
            .checked_mul(u64::from(high.1) - u64::from(low.1) + 1)
            .is_none_or(|n| n > MAX_CELLS)
    {
        bail!("parsed worksheet exceeds 4 million cell display budget");
    }
    Ok(())
}
fn number(bytes: &[u8]) -> Result<u32> {
    Ok(std::str::from_utf8(bytes)?.parse()?)
}
fn address(value: &[u8]) -> Result<(u32, u32)> {
    let split = value
        .iter()
        .position(|b| !b.is_ascii_alphabetic())
        .context("missing cell row")?;
    if split == 0 {
        bail!("invalid cell coordinate");
    }
    let mut col = 0u32;
    for c in &value[..split] {
        col = col
            .checked_mul(26)
            .and_then(|v| v.checked_add(u32::from(c.to_ascii_uppercase() - b'A' + 1)))
            .context("column overflow")?;
    }
    let row = number(&value[split..])?;
    if row == 0 || row > 1_048_576 || col == 0 || col > 16_384 {
        bail!("invalid Excel cell coordinate");
    }
    Ok((row - 1, col - 1))
}
fn scan_xml(
    input: impl io::BufRead,
    part: &str,
    events: &mut usize,
    sheets: &mut HashSet<String>,
    targets: &mut HashMap<String, (String, bool)>,
) -> Result<()> {
    let mut reader = Reader::from_reader(input);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    let mut sheet = false;
    let (mut row, mut next_row, mut col) = (0u32, 0u32, 0u32);
    let (mut lo, mut hi) = (None::<(u32, u32)>, (0u32, 0u32));
    let mut cells = 0u64;
    let mut strings = 0u64;
    let mut text = 0usize;
    let mut in_cell = false;
    loop {
        buf.clear();
        let event = reader
            .read_event_into(&mut buf)
            .context("invalid or oversized workbook XML")?;
        *events += 1;
        if *events > 10_000_000 {
            bail!("workbook XML event budget exceeded");
        }
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(event, Event::Empty(_));
                if !empty {
                    depth += 1;
                    if depth > 128 {
                        bail!("workbook XML depth budget exceeded");
                    }
                }
                let local = e.local_name();
                let name = local.as_ref();
                if name == b"worksheet" {
                    sheet = true;
                }
                let mut coord = None;
                let mut row_attr = None;
                let (mut id, mut target, mut external) = (None, None, false);
                for attr in e.attributes() {
                    let attr = attr?;
                    let key = attr.key.local_name();
                    let key = key.as_ref();
                    if part == "xl/workbook.xml" && name == b"sheet" && key == b"id" {
                        sheets.insert(
                            attr.decode_and_unescape_value(reader.decoder())?
                                .into_owned(),
                        );
                    }
                    if part == "xl/_rels/workbook.xml.rels" && name == b"Relationship" {
                        if key == b"Id" {
                            id = Some(
                                attr.decode_and_unescape_value(reader.decoder())?
                                    .into_owned(),
                            );
                        }
                        if key == b"Target" {
                            target = Some(
                                attr.decode_and_unescape_value(reader.decoder())?
                                    .into_owned(),
                            );
                        }
                        if key == b"TargetMode" {
                            external = attr.value.as_ref() == b"External";
                        }
                    }
                    if name == b"sst"
                        && attr.key.local_name().as_ref() == b"uniqueCount"
                        && u64::from(number(&attr.value)?) > MAX_CELLS
                    {
                        bail!("shared-string count budget exceeded");
                    }
                    if sheet && attr.key.local_name().as_ref() == b"r" {
                        if name == b"c" {
                            coord = Some(address(&attr.value)?);
                        } else if name == b"row" {
                            row_attr = Some(number(&attr.value)?);
                        }
                    }
                }
                if let (Some(id), Some(target)) = (id, target)
                    && targets.insert(id, (target, external)).is_some()
                {
                    bail!("duplicate workbook relationship");
                }
                if name == b"si" {
                    strings += 1;
                    if strings > MAX_CELLS {
                        bail!("shared-string count budget exceeded");
                    }
                    in_cell = !empty;
                    text = 0;
                }
                if sheet && name == b"row" {
                    row = match row_attr {
                        Some(v) => v.checked_sub(1).context("invalid row coordinate")?,
                        None => next_row,
                    };
                    if row >= 1_048_576 {
                        bail!("row coordinate exceeds Excel limit");
                    }
                    next_row = row + 1;
                    col = 0;
                }
                if sheet && name == b"c" {
                    let pos = coord.unwrap_or((row, col));
                    // Explicit cell coordinates are authoritative even when
                    // the enclosing row omits its optional r attribute.
                    if pos.1 >= 16_384 {
                        bail!("column coordinate exceeds Excel limit");
                    }
                    col = pos.1 + 1;
                    cells += 1;
                    lo = Some(lo.map(|p| (p.0.min(pos.0), p.1.min(pos.1))).unwrap_or(pos));
                    hi = (hi.0.max(pos.0), hi.1.max(pos.1));
                    let low = lo.expect("just inserted");
                    let area = u64::from(hi.0 - low.0 + 1) * u64::from(hi.1 - low.1 + 1);
                    if cells > MAX_CELLS || area > MAX_CELLS {
                        bail!("worksheet occupied rectangle exceeds 4 million cell budget");
                    }
                    in_cell = !empty;
                    text = 0;
                }
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                if matches!(e.local_name().as_ref(), b"c" | b"si") {
                    in_cell = false;
                }
            }
            Event::Text(e) => {
                if in_cell {
                    text = text.saturating_add(e.len());
                    if text > MAX_TEXT {
                        bail!("worksheet cell text exceeds 1 MiB budget");
                    }
                }
            }
            Event::CData(e) => {
                if in_cell {
                    text = text.saturating_add(e.len());
                    if text > MAX_TEXT {
                        bail!("worksheet cell text exceeds 1 MiB budget");
                    }
                }
            }
            Event::GeneralRef(e) => {
                if in_cell {
                    text = text.saturating_add(e.len());
                    if text > MAX_TEXT {
                        bail!("worksheet cell text exceeds 1 MiB budget");
                    }
                }
            }
            Event::DocType(_) => bail!("workbook XML DTD is not admitted"),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn zip(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, text) in parts {
            zip.start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn central_directory_count_is_checked_before_indexing() {
        let mut bytes = zip(&[("a.xml", "<a/>")]);
        let offset = bytes.windows(4).rposition(|b| b == b"PK\x05\x06").unwrap();
        bytes[offset + 10..offset + 12].copy_from_slice(&4097u16.to_le_bytes());
        assert!(
            preflight(&bytes)
                .unwrap_err()
                .to_string()
                .contains("too many members")
        );
        assert!(check_dense_ranges(Some(((0, 0), (99999, 11))), None).is_ok());
        assert!(
            check_dense_ranges(
                Some(((0, 0), (0, 0))),
                Some(((1048575, 16383), (1048575, 16383)))
            )
            .is_err()
        );
    }
    #[test]
    fn shared_string_reservation_and_sparse_rectangle_refused_without_calamine() {
        assert!(
            preflight(&zip(&[(
                "xl/sharedStrings.xml",
                "<sst uniqueCount=\"999999999\"/>"
            )]))
            .is_err()
        );
        let sparse = "<worksheet><sheetData><row r=\"1\"><c r=\"A1\"/></row><row r=\"1048576\"><c r=\"XFD1048576\"/></row></sheetData></worksheet>";
        assert!(preflight(&zip(&[("custom/sheet.xml", sparse)])).is_err());
        assert!(
            preflight(&zip(&[(
                "sheet.xml",
                "<worksheet><sheetData><row r=\"1\"><c r=\"A0\"/></row></sheetData></worksheet>"
            )]))
            .is_err()
        );
    }
    #[test]
    fn explicit_cell_coordinates_do_not_require_row_attributes() {
        let xml = "<worksheet><sheetData><row><c r=\"D5\"/></row><row><c r=\"E6\"/></row></sheetData></worksheet>";
        assert!(preflight(&zip(&[("custom/sheet.xml", xml)])).is_ok());
    }
    #[test]
    fn worksheet_relationship_cannot_bypass_xml_scan() {
        let workbook =
            "<workbook xmlns:r=\"urn:test\"><sheets><sheet r:id=\"r1\"/></sheets></workbook>";
        let rel =
            "<Relationships><Relationship Id=\"r1\" Target=\"custom/data.bin\"/></Relationships>";
        assert!(
            preflight(&zip(&[
                ("xl/workbook.xml", workbook),
                ("xl/_rels/workbook.xml.rels", rel),
                ("custom/data.bin", "opaque")
            ]))
            .is_err()
        );
        let rel =
            "<Relationships><Relationship Id=\"r1\" Target=\"/custom/sheet.xml\"/></Relationships>";
        assert!(preflight(&zip(&[("xl/workbook.xml",workbook),("xl/_rels/workbook.xml.rels",rel),("custom/sheet.xml","<worksheet><sheetData><row><c r=\"A1\"><v>1</v></c></row></sheetData></worksheet>")])).is_ok());
    }
    #[test]
    fn actual_byte_counter_stops_at_part_and_aggregate_budgets() {
        let mut total = 0;
        let mut counted = Counted {
            reader: io::repeat(0).take(MAX_MEMBER + 1),
            part: 0,
            total: &mut total,
        };
        assert!(io::copy(&mut counted, &mut io::sink()).is_err());
        assert_eq!(total, MAX_MEMBER);
        let mut total = MAX_TOTAL - 8;
        let mut counted = Counted {
            reader: io::repeat(0).take(16),
            part: 0,
            total: &mut total,
        };
        assert!(io::copy(&mut counted, &mut io::sink()).is_err());
        assert!(total <= MAX_TOTAL);
    }
    #[test]
    fn dtd_and_long_shared_string_refused() {
        assert!(preflight(&zip(&[("a.xml", "<!DOCTYPE sst><sst/>")])).is_err());
        let text = format!("<sst><si><t>{}</t></si></sst>", "x".repeat(MAX_TEXT + 1));
        assert!(preflight(&zip(&[("xl/sharedStrings.xml", &text)])).is_err());
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    #[test]
    fn snapshot_is_immutable_and_large_input_rejected_before_reading() {
        let scratch = std::env::var_os("TOSS_SHEET_TEST_TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.sandbox-test/spreadsheet-unit")
            });
        std::fs::create_dir_all(&scratch).unwrap();
        let path = scratch.join(format!("snapshot-{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());
        use std::io::Write;
        file.write_all(b"original").unwrap();
        drop(file);
        let bytes = snapshot(&path).unwrap();
        std::fs::write(&path, b"changed").unwrap();
        assert_eq!(&*bytes, b"original");
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(MAX_INPUT + 1).unwrap();
        drop(file);
        assert!(snapshot(&path).is_err());
        assert!(check_dense_ranges(Some(((0, 0), (u32::MAX, u32::MAX))), None).is_err());
    }
}
