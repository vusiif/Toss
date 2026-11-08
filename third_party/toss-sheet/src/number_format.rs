//! Conservative numeric display subset. Unsupported codes retain raw values.
use crate::colors::{attr, part, xml};
use crate::workbook::CellValue;
use anyhow::Result;
use std::{collections::HashMap, io::Cursor, sync::Arc};

/// Common numeric columns have long runs of one format. Keep those as ranges;
/// fall back to exact per-cell storage if the XML is unordered or duplicated.
#[derive(Default)]
pub struct StyleIndices {
    runs: Vec<StyleRun>,
    unordered: Option<HashMap<(u32, u32), usize>>,
}
struct StyleRun {
    start: u64,
    end: u64,
    index: usize,
}
impl StyleIndices {
    fn key(pos: (u32, u32)) -> u64 {
        u64::from(pos.0) * 16_384 + u64::from(pos.1)
    }
    pub fn clear(&mut self) {
        self.runs.clear();
        self.unordered = None;
    }
    pub fn insert(&mut self, pos: (u32, u32), index: usize) {
        if let Some(map) = &mut self.unordered {
            if index == 0 {
                map.remove(&pos);
            } else {
                map.insert(pos, index);
            }
            return;
        }
        let key = Self::key(pos);
        if index == 0 && self.runs.last().is_none_or(|r| key > r.end) {
            return;
        }
        if self.runs.last().is_some_and(|r| key <= r.end) {
            let mut map = HashMap::new();
            for run in std::mem::take(&mut self.runs) {
                for key in run.start..=run.end {
                    map.insert(((key / 16_384) as u32, (key % 16_384) as u32), run.index);
                }
            }
            if index == 0 {
                map.remove(&pos);
            } else {
                map.insert(pos, index);
            }
            self.unordered = Some(map);
            return;
        }
        if let Some(last) = self.runs.last_mut()
            && last.index == index
            && last.end + 1 == key
        {
            last.end = key;
            return;
        }
        self.runs.push(StyleRun {
            start: key,
            end: key,
            index,
        });
    }
    pub fn get(&self, pos: &(u32, u32)) -> Option<&usize> {
        if let Some(map) = &self.unordered {
            return map.get(pos);
        }
        let key = Self::key(*pos);
        let end = self.runs.partition_point(|r| r.start <= key);
        self.runs
            .get(end.checked_sub(1)?)
            .filter(|r| key <= r.end)
            .map(|r| &r.index)
    }
}

pub struct Catalog {
    styles: Vec<Option<Arc<NumberFormat>>>,
}

impl Catalog {
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let bytes = part(&mut zip, "xl/styles.xml")?;
        let mut custom = HashMap::new();
        let mut bases = Vec::new();
        let mut xfs = Vec::new();
        let mut section = String::new();
        xml(&bytes, |start, n, a| {
            if ["numFmts", "cellStyleXfs", "cellXfs"].contains(&n) {
                section = if start { n.to_owned() } else { String::new() };
            } else if start {
                match (section.as_str(), n) {
                    ("numFmts", "numFmt") => {
                        if let (Some(id), Some(code)) = (
                            attr(a, "numFmtId").and_then(|s| s.parse::<u32>().ok()),
                            attr(a, "formatCode"),
                        ) && code.len() <= 1024
                        {
                            custom.insert(id, code.to_owned());
                        }
                    }
                    ("cellStyleXfs", "xf") => bases.push(a.clone()),
                    ("cellXfs", "xf") => xfs.push(a.clone()),
                    _ => {}
                }
            }
        })?;
        let mut compiled: HashMap<u32, Option<Arc<NumberFormat>>> = HashMap::new();
        let styles = xfs
            .iter()
            .map(|xf| {
                let parent = attr(xf, "xfId")
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|i| bases.get(i));
                let direct = if attr(xf, "applyNumberFormat") == Some("0") {
                    None
                } else {
                    attr(xf, "numFmtId")
                };
                let id = direct
                    .or_else(|| parent.and_then(|p| attr(p, "numFmtId")))
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                compiled
                    .entry(id)
                    .or_insert_with(|| {
                        custom
                            .get(&id)
                            .map(String::as_str)
                            .or_else(|| builtin(id))
                            .and_then(NumberFormat::parse)
                            .map(Arc::new)
                    })
                    .clone()
            })
            .collect();
        Ok(Self { styles })
    }
    pub fn active(&self) -> bool {
        self.styles.iter().any(Option::is_some)
    }
    pub fn needs_index(&self, index: usize) -> bool {
        match (
            self.styles.first().and_then(Option::as_ref),
            self.styles.get(index).and_then(Option::as_ref),
        ) {
            (None, None) => false,
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            _ => true,
        }
    }
    pub fn render(&self, index: usize, value: &CellValue) -> Option<String> {
        self.styles.get(index)?.as_ref()?.render(value)
    }
}
fn builtin(id: u32) -> Option<&'static str> {
    Some(match id {
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "\"$\"#,##0_);(\"$\"#,##0)",
        6 => "\"$\"#,##0_);[Red](\"$\"#,##0)",
        7 => "\"$\"#,##0.00_);(\"$\"#,##0.00)",
        8 => "\"$\"#,##0.00_);[Red](\"$\"#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        _ => return None,
    })
}
struct NumberFormat {
    sections: Vec<Section>,
}
impl NumberFormat {
    fn parse(code: &str) -> Option<Self> {
        let mut sections = Vec::new();
        let mut start = 0;
        let mut quoted = false;
        let mut escape = false;
        for (i, c) in code.char_indices() {
            if escape {
                escape = false;
                continue;
            }
            if c == '\\' {
                escape = true;
                continue;
            }
            if c == '"' {
                quoted = !quoted;
            }
            if c == ';' && !quoted {
                sections.push(Section::parse(&code[start..i])?);
                start = i + 1;
            }
        }
        if quoted || escape {
            return None;
        }
        if !(sections.len() == 3 && code[start..].trim() == "@") {
            sections.push(Section::parse(&code[start..])?);
        }
        if sections.len() > 3 {
            return None;
        }
        Some(Self { sections })
    }
    fn render(&self, value: &CellValue) -> Option<String> {
        let n = match value {
            CellValue::Float(v) => *v,
            CellValue::Int(v) if v.unsigned_abs() <= 9_007_199_254_740_992 => *v as f64,
            _ => return None,
        };
        if !n.is_finite() {
            return None;
        }
        let explicit_negative = n < 0.0 && self.sections.len() > 1;
        let index = if explicit_negative {
            1
        } else if n == 0.0 && self.sections.len() > 2 {
            2
        } else {
            0
        };
        self.sections[index].render(n.abs(), n < 0.0 && !explicit_negative)
    }
}
struct Section {
    prefix: String,
    suffix: String,
    min_digits: usize,
    min_decimals: usize,
    decimals: usize,
    grouped: bool,
    percent: bool,
    scientific: Option<(usize, bool)>,
    literal: bool,
}
impl Section {
    fn parse(code: &str) -> Option<Self> {
        // Preserve a fourth text section by declining the full format, not guessing.
        let mut tokens: Vec<(char, bool)> = Vec::new();
        let mut chars = code.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    let mut closed = false;
                    for c in chars.by_ref() {
                        if c == '"' {
                            closed = true;
                            break;
                        }
                        tokens.push((c, false));
                    }
                    if !closed {
                        return None;
                    }
                }
                '\\' => tokens.push((chars.next()?, false)),
                '_' | '*' => {
                    chars.next()?;
                }
                '[' => {
                    let mut s = String::new();
                    let mut closed = false;
                    for c in chars.by_ref() {
                        if c == ']' {
                            closed = true;
                            break;
                        }
                        s.push(c);
                    }
                    if !closed {
                        return None;
                    }
                    if let Some(currency) = s.strip_prefix('$') {
                        for c in currency.split('-').next()?.chars() {
                            tokens.push((c, false));
                        }
                    } else if ![
                        "Red", "Black", "Blue", "Green", "White", "Yellow", "Magenta", "Cyan",
                    ]
                    .iter()
                    .any(|v| v.eq_ignore_ascii_case(&s))
                    {
                        return None;
                    }
                }
                '0' | '#' | '.' | ',' | 'E' | 'e' | '+' | '-' | '%' => tokens.push((c, true)),
                c if c.is_ascii_alphabetic() || matches!(c, '?' | '@' | '/') => return None,
                c => tokens.push((c, false)),
            }
        }
        let positions: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter(|(_, (c, s))| *s && matches!(c, '0' | '#'))
            .map(|(i, _)| i)
            .collect();
        let Some(&first) = positions.first() else {
            return Some(Self {
                prefix: tokens.iter().map(|(c, _)| *c).collect(),
                suffix: String::new(),
                min_digits: 0,
                min_decimals: 0,
                decimals: 0,
                grouped: false,
                percent: false,
                scientific: None,
                literal: true,
            });
        };
        let last = *positions.last()?;
        let core = &tokens[first..=last];
        if core.iter().any(|(_, syntax)| !*syntax) {
            return None;
        }
        let core: String = core.iter().map(|(c, _)| *c).collect();
        let (mantissa, scientific) = if let Some(i) = core.find(['E', 'e']) {
            let exponent = &core[i + 1..];
            let plus = exponent.starts_with('+');
            let zeros = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
            if zeros.is_empty() || zeros.len() > 4 || !zeros.chars().all(|c| c == '0') {
                return None;
            }
            (&core[..i], Some((zeros.len(), plus)))
        } else {
            (core.as_str(), None)
        };
        let mut parts = mantissa.split('.');
        let integer = parts.next()?;
        let fraction = parts.next().unwrap_or("");
        if parts.next().is_some()
            || !integer.chars().all(|c| matches!(c, '0' | '#' | ','))
            || !fraction.chars().all(|c| matches!(c, '0' | '#'))
        {
            return None;
        }
        if integer.ends_with(',')
            || tokens[last + 1..]
                .iter()
                .any(|(c, s)| *s && matches!(c, ',' | '.' | '0' | '#' | 'E' | 'e'))
        {
            return None;
        }
        let percent = tokens.iter().filter(|(c, s)| *s && *c == '%').count();
        if percent > 1 {
            return None;
        }
        let min_digits = integer.chars().filter(|c| *c == '0').count();
        if min_digits == 0
            || tokens[..first]
                .iter()
                .any(|(c, s)| *s && matches!(c, '.' | ','))
        {
            return None;
        }
        if min_digits > 32 || fraction.len() > 16 {
            return None;
        }
        Some(Self {
            prefix: tokens[..first].iter().map(|(c, _)| *c).collect(),
            suffix: tokens[last + 1..].iter().map(|(c, _)| *c).collect(),
            min_digits,
            min_decimals: fraction.chars().filter(|c| *c == '0').count(),
            decimals: fraction.len(),
            grouped: integer.contains(','),
            percent: percent == 1,
            scientific,
            literal: false,
        })
    }
    fn render(&self, mut n: f64, negative: bool) -> Option<String> {
        if self.literal {
            return Some(self.prefix.clone());
        }
        if self.percent {
            n *= 100.0;
        }
        if !n.is_finite() {
            return None;
        }
        let body = if let Some((width, plus)) = self.scientific {
            let text = format!("{n:.precision$e}", precision = self.decimals);
            let (mantissa, exponent) = text.split_once('e')?;
            let exponent = exponent.parse::<i32>().ok()?;
            format!(
                "{mantissa}E{}{digits:0width$}",
                if exponent < 0 {
                    "-"
                } else if plus {
                    "+"
                } else {
                    ""
                },
                digits = exponent.unsigned_abs()
            )
        } else {
            let text = format!("{n:.precision$}", precision = self.decimals);
            let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
            let mut integer = format!("{:0>width$}", integer, width = self.min_digits);
            if self.grouped {
                let len = integer.len();
                integer = integer
                    .chars()
                    .enumerate()
                    .fold(String::new(), |mut s, (i, c)| {
                        if i > 0 && (len - i) % 3 == 0 {
                            s.push(',');
                        }
                        s.push(c);
                        s
                    });
            }
            let mut fraction = fraction.to_owned();
            while fraction.len() > self.min_decimals && fraction.ends_with('0') {
                fraction.pop();
            }
            if fraction.is_empty() {
                integer
            } else {
                format!("{integer}.{fraction}")
            }
        };
        Some(format!(
            "{}{}{}{}",
            if negative { "-" } else { "" },
            self.prefix,
            body,
            self.suffix
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn format_runs_preserve_gaps_and_fall_back_on_duplicate_or_unordered_cells() {
        let mut indices = StyleIndices::default();
        for col in 1..12 {
            indices.insert((0, col), 3);
        }
        for col in 1..12 {
            indices.insert((1, col), 3);
        }
        assert_eq!(indices.runs.len(), 2);
        assert_eq!(indices.get(&(0, 0)), None);
        assert_eq!(indices.get(&(0, 8)), Some(&3));
        assert_eq!(indices.get(&(1, 0)), None);
        indices.insert((0, 8), 4);
        assert!(indices.unordered.is_some());
        assert_eq!(indices.get(&(0, 8)), Some(&4));
        indices.insert((0, 8), 0);
        assert_eq!(indices.get(&(0, 8)), None);
        assert_eq!(indices.get(&(1, 8)), Some(&3));
        indices.clear();
        assert_eq!(indices.get(&(1, 8)), None);
    }
    #[test]
    fn style_inheritance_and_general_override_are_resolved() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("xl/styles.xml", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(br#"<styleSheet><numFmts><numFmt numFmtId="164" formatCode="000000"/></numFmts><cellStyleXfs><xf numFmtId="9"/></cellStyleXfs><cellXfs><xf numFmtId="9"/><xf numFmtId="0"/><xf numFmtId="4" xfId="0" applyNumberFormat="0"/><xf numFmtId="164"/></cellXfs></styleSheet>"#).unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let catalog = Catalog::load(&bytes).unwrap();
        assert!(catalog.needs_index(1));
        assert!(!catalog.needs_index(2));
        assert_eq!(
            catalog.render(0, &CellValue::Float(0.25)),
            Some("25%".into())
        );
        assert_eq!(catalog.render(1, &CellValue::Float(0.25)), None);
        assert_eq!(
            catalog.render(2, &CellValue::Float(0.25)),
            Some("25%".into())
        );
        assert_eq!(
            catalog.render(3, &CellValue::Int(1234)),
            Some("001234".into())
        );
    }
    #[test]
    fn common_formats_and_literals_keep_their_meaning() {
        for (code, value, expected) in [
            ("0%", 0.25, "25%"),
            ("0.00%", 0.12345, "12.35%"),
            ("\"¥\"#,##0.00", 1200.0, "¥1,200.00"),
            ("000000", 1234.0, "001234"),
            ("0.00##", 1.2, "1.20"),
            ("\"¥\"#,##0.00;-\"¥\"#,##0.00", -1200.5, "-¥1,200.50"),
            ("0.00E+00", 12345.0, "1.23E+04"),
            ("0.00;-0.00;\"zero\";@", 0.0, "zero"),
            ("0.00\"%\"", 0.25, "0.25%"),
            ("[$¥-804]#,##0.00", 1200.0, "¥1,200.00"),
        ] {
            assert_eq!(
                NumberFormat::parse(code)
                    .unwrap()
                    .render(&CellValue::Float(value))
                    .unwrap(),
                expected,
                "{code}"
            );
        }
    }
    #[test]
    fn unsupported_codes_and_non_numeric_values_fall_back() {
        for code in [
            "General",
            "[>=1]0.00;0%",
            "# ?/?",
            "yyyy-mm-dd",
            "0.00,",
            "0.00000000000000000000",
            "0.0.0",
            "\"broken",
        ] {
            assert!(NumberFormat::parse(code).is_none(), "{code}");
        }
        let format = NumberFormat::parse("000000").unwrap();
        assert!(format.render(&CellValue::String("001234".into())).is_none());
        assert!(format.render(&CellValue::Int(i64::MAX)).is_none());
        assert!(format.render(&CellValue::Float(f64::INFINITY)).is_none());
    }
}
