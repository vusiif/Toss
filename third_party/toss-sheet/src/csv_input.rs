//! Bounded UTF-8 CSV/TSV input for the Toss prototype; no type inference.
use anyhow::{Context, Result, bail};
use calamine::{Cell, Data, Range};
use std::{fs::File, io::Read, path::Path};
const MAX_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CELLS: usize = 2_000_000;
const MAX_ROWS: usize = 250_000;
const MAX_FIELD: usize = 1024 * 1024;

pub fn read(path: &Path, delimiter: u8) -> Result<Range<Data>> {
    if matches!(delimiter, 0 | b'\n' | b'\r' | b'"') {
        bail!("invalid CSV delimiter");
    }
    let file = File::open(path).context("cannot open CSV")?;
    if file.metadata()?.len() > MAX_BYTES {
        bail!("CSV input exceeds the prototype's 64 MiB limit");
    }
    let mut bounded = file.take(MAX_BYTES + 1);
    let range = parse(&mut bounded, delimiter)?;
    if bounded.limit() == 0 {
        bail!("CSV grew beyond the 64 MiB input limit");
    }
    Ok(range)
}

fn parse(mut source: impl Read, delimiter: u8) -> Result<Range<Data>> {
    // csv-core recognizes the initial BOM only when its three bytes arrive
    // together with a following byte (a BOM-only chunk looks like EOF).
    // Coalesce just this prefix, even for short/chunked readers.
    let mut head = Vec::with_capacity(4);
    source.by_ref().take(4).read_to_end(&mut head)?;
    let source = std::io::Cursor::new(head).chain(source);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .buffer_capacity(8192)
        .from_reader(RecordPrefixes::new(source));
    let mut cells = Vec::new();
    let mut rows = 0usize;
    let mut width = 0usize;
    let mut record = csv::StringRecord::new();
    loop {
        let start = reader.position().byte();
        reader.get_mut().mark(start);
        let present = reader
            .read_record(&mut record)
            .context("invalid UTF-8 or CSV record")?;
        let blanks = reader.get_ref().prefix.blanks;
        if blanks > 0 {
            let end = rows.checked_add(blanks).context("CSV row count overflow")?;
            width = width.max(1);
            check_shape(end, width)?;
            for row in rows..end {
                cells.push(Cell::new((row as u32, 0), Data::String(String::new())));
            }
            rows = end;
        }
        if !present {
            break;
        }
        rows += 1;
        width = width.max(record.len());
        check_shape(rows, width)?;
        for (col, field) in record.iter().enumerate() {
            if field.len() > MAX_FIELD {
                bail!("CSV field exceeds 1 MiB");
            }
            cells.push(Cell::new(
                ((rows - 1) as u32, col as u32),
                Data::String(field.to_owned()),
            ));
        }
    }
    Ok(Range::from_sparse(cells))
}
fn check_shape(rows: usize, width: usize) -> Result<()> {
    if rows > MAX_ROWS || rows.checked_mul(width).is_none_or(|n| n > MAX_CELLS) {
        bail!("CSV row/cell budget exceeded");
    }
    Ok(())
}

// Inspect only the raw prefix at boundaries supplied by csv::Reader. Quoted
// multiline fields stay entirely under that parser's control. Keep its latest
// input chunk because it can read ahead across many records in one Read call.
struct RecordPrefixes<R> {
    inner: R,
    chunk: Vec<u8>,
    chunk_start: u64,
    total: u64,
    prefix: Prefix,
}
impl<R> RecordPrefixes<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            chunk: Vec::new(),
            chunk_start: 0,
            total: 0,
            prefix: Prefix::new(false, true),
        }
    }
    fn mark(&mut self, at: u64) {
        let previous_cr = at
            .checked_sub(1)
            .and_then(|p| p.checked_sub(self.chunk_start))
            .and_then(|i| self.chunk.get(i as usize))
            .is_some_and(|b| *b == b'\r');
        self.prefix = Prefix::new(previous_cr, at == 0);
        if let Some(i) = at
            .checked_sub(self.chunk_start)
            .filter(|i| *i <= self.chunk.len() as u64)
        {
            self.prefix.scan(&self.chunk[i as usize..]);
        }
    }
}
impl<R: Read> Read for RecordPrefixes<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(out)?;
        if n != 0 {
            self.prefix.scan(&out[..n]);
            self.chunk_start = self.total;
            self.total += n as u64;
            self.chunk.clear();
            self.chunk.extend_from_slice(&out[..n]);
        }
        Ok(n)
    }
}
struct Prefix {
    active: bool,
    blanks: usize,
    skip_lf: bool,
    bom: Option<usize>,
}
impl Prefix {
    fn new(skip_lf: bool, first: bool) -> Self {
        Self {
            active: true,
            blanks: 0,
            skip_lf,
            bom: first.then_some(0),
        }
    }
    fn scan(&mut self, input: &[u8]) {
        if !self.active {
            return;
        }
        for &b in input {
            if let Some(matched) = self.bom {
                if b == [0xEF, 0xBB, 0xBF][matched] {
                    self.bom = (matched < 2).then_some(matched + 1);
                    continue;
                }
                self.bom = None;
                if matched != 0 {
                    self.active = false;
                    break;
                }
            }
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.blanks += 1;
                    self.skip_lf = true;
                }
                b'\n' => self.blanks += 1,
                _ => {
                    self.active = false;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod blank_tests {
    use super::*;
    use std::io::Cursor;
    fn rows(input: &[u8], chunk: usize) -> Vec<Vec<String>> {
        struct Chunks<'a> {
            data: &'a [u8],
            chunk: usize,
        }
        impl Read for Chunks<'_> {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                let n = out.len().min(self.chunk).min(self.data.len());
                out[..n].copy_from_slice(&self.data[..n]);
                self.data = &self.data[n..];
                Ok(n)
            }
        }
        parse(Chunks { data: input, chunk }, b',')
            .unwrap()
            .rows()
            .map(|r| {
                r.iter()
                    .map(|c| match c {
                        Data::String(s) => s.clone(),
                        Data::Empty => "<pad>".into(),
                        _ => panic!("unexpected type"),
                    })
                    .collect()
            })
            .collect()
    }
    #[test]
    fn empty_records_crlf_quotes_bom_and_chunk_boundaries() {
        let cases: &[(&[u8], &[&str])] = &[
            (b"", &[]),
            (b"\n\n", &["", ""]),
            (b"foo\n\n", &["foo", ""]),
            (b"foo\r\n", &["foo"]),
            (b"\r\nfoo\r\n\r\n", &["", "foo", ""]),
            (b"foo\r\rbar\r", &["foo", "", "bar"]),
            (b"\n\"one\n\ntwo\"\n\n", &["", "one\n\ntwo", ""]),
            (b"\xEF\xBB\xBF\nfoo\n\n", &["", "foo", ""]),
            (b"foo\n\xEF\xBB\xBFbar\n", &["foo", "\u{feff}bar"]),
        ];
        for &(input, expected) in cases {
            for chunk in [1, 2, 3, 7, 8192] {
                let actual = rows(input, chunk);
                assert_eq!(
                    actual,
                    expected
                        .iter()
                        .map(|s| vec![s.to_string()])
                        .collect::<Vec<_>>(),
                    "{input:?}, chunk={chunk}"
                );
            }
        }
    }
    #[test]
    fn multiline_fields_and_blank_rows_share_rectangular_bounds() {
        let actual = rows(b"a,b\r\n\r\n1,\"two\r\n\r\nlines\"\r\n\r\n", 1);
        assert_eq!(
            actual,
            vec![
                vec!["a", "b"],
                vec!["", "<pad>"],
                vec!["1", "two\r\n\r\nlines"],
                vec!["", "<pad>"]
            ]
        );
    }
    #[test]
    fn long_record_does_not_lose_prefix_and_empty_rows_are_budgeted() {
        let mut input = b"\n\n".to_vec();
        input.extend(vec![b'x'; 20000]);
        input.extend(b"\r\n\r\n");
        let actual = rows(&input, 8192);
        assert_eq!(actual.len(), 4);
        assert_eq!(actual[2][0].len(), 20000);
        let err = parse(Cursor::new(vec![b'\n'; MAX_ROWS + 1]), b',')
            .expect_err("must reject blank-row excess");
        assert!(err.to_string().contains("row/cell budget"));
    }
}
