//! Preview: direct cell font and solid-fill colors, loaded only for the TUI.
use anyhow::{Result, bail};
use quick_xml::{Reader, events::Event};
use ratatui::style::{Color, Style};
use std::io::{Cursor, Read};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellColors {
    pub fg: Color,
    pub bg: Color,
    /// Top, right, bottom, left; None means an auxiliary grid line only.
    pub borders: [Option<CellBorder>; 4],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellBorder {
    pub color: Color,
    pub kind: BorderKind,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum BorderKind {
    Dotted,
    Dashed,
    Thin,
    Thick,
    Double,
}
impl BorderKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "hair" | "dotted" => Self::Dotted,
            "dashed" | "dashDot" | "dashDotDot" | "mediumDashed" | "mediumDashDot"
            | "mediumDashDotDot" | "slantDashDot" => Self::Dashed,
            "thin" => Self::Thin,
            "medium" | "thick" => Self::Thick,
            "double" => Self::Double,
            _ => return None,
        })
    }
    pub fn glyph(self, horizontal: bool) -> &'static str {
        match (self, horizontal) {
            (Self::Dotted, true) => "┈",
            (Self::Dotted, false) => "┊",
            (Self::Dashed, true) => "╌",
            (Self::Dashed, false) => "╎",
            (Self::Thick, true) => "━",
            (Self::Thick, false) => "┃",
            (Self::Double, true) => "═",
            (Self::Double, false) => "║",
            (_, true) => "─",
            (_, false) => "│",
        }
    }
}
impl Default for CellColors {
    fn default() -> Self {
        Self {
            fg: Color::Rgb(0, 0, 0),
            bg: Color::Rgb(255, 255, 255),
            borders: [None; 4],
        }
    }
}
impl CellColors {
    pub fn apply(self, style: Style) -> Style {
        style.fg(self.fg).bg(self.bg)
    }
}
#[derive(Default, Clone)]
struct RefColor {
    rgb: Option<String>,
    theme: Option<usize>,
    indexed: Option<usize>,
    tint: f64,
}
pub(crate) type Attrs = Vec<(String, String)>;
pub(crate) fn attr<'a>(a: &'a Attrs, key: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}
fn color(a: &Attrs) -> RefColor {
    RefColor {
        rgb: attr(a, "rgb").map(str::to_owned),
        theme: attr(a, "theme").and_then(|v| v.parse().ok()),
        indexed: attr(a, "indexed").and_then(|v| v.parse().ok()),
        tint: attr(a, "tint").and_then(|v| v.parse().ok()).unwrap_or(0.0),
    }
}
fn rgb(s: &str) -> Option<Color> {
    let s = match s.len() {
        8 => &s[2..],
        6 => s,
        _ => return None,
    };
    u32::from_str_radix(s, 16)
        .ok()
        .map(|v| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}
pub(crate) fn xml(bytes: &[u8], mut f: impl FnMut(bool, &str, &Attrs)) -> Result<()> {
    let mut r = Reader::from_reader(bytes);
    r.config_mut().expand_empty_elements = true;
    let mut depth = 0usize;
    loop {
        match r.read_event()? {
            Event::Start(e) => {
                depth += 1;
                if depth > 64 {
                    bail!("color metadata nesting limit");
                }
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs = e
                    .attributes()
                    .map(|a| {
                        let a = a?;
                        Ok((
                            String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned(),
                            a.decode_and_unescape_value(r.decoder())?.into_owned(),
                        ))
                    })
                    .collect::<Result<Attrs>>()?;
                f(true, &name, &attrs);
            }
            Event::End(e) => {
                depth = depth.saturating_sub(1);
                f(
                    false,
                    &String::from_utf8_lossy(e.local_name().as_ref()),
                    &Vec::new(),
                );
            }
            Event::Eof => {
                if depth != 0 {
                    bail!("incomplete color metadata");
                }
                break;
            }
            _ => {}
        }
    }
    Ok(())
}
pub(crate) fn part(z: &mut zip::ZipArchive<Cursor<&[u8]>>, path: &str) -> Result<Vec<u8>> {
    let file = match z.by_name(path) {
        Ok(f) => f,
        Err(zip::result::ZipError::FileNotFound) => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        bail!("color metadata exceeds 2 MiB");
    }
    Ok(bytes)
}
pub struct Catalog {
    styles: Vec<CellColors>,
}
impl Catalog {
    pub fn get(&self, index: usize) -> CellColors {
        self.styles.get(index).copied().unwrap_or_default()
    }
    pub fn load(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let theme_xml = part(&mut zip, "xl/theme/theme1.xml")?;
        let styles_xml = part(&mut zip, "xl/styles.xml")?;
        let mut theme = [None; 12];
        let mut slot = None;
        let mut scheme = false;
        const NAMES: [&str; 12] = [
            "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5",
            "accent6", "hlink", "folHlink",
        ];
        xml(&theme_xml, |start, n, a| {
            if n == "clrScheme" {
                scheme = start;
            }
            if !scheme {
                return;
            }
            if start {
                if let Some(i) = NAMES.iter().position(|v| *v == n) {
                    slot = Some(i);
                } else if let Some(i) = slot {
                    if n == "srgbClr" {
                        theme[i] = attr(a, "val").and_then(rgb);
                    } else if n == "sysClr" {
                        theme[i] = attr(a, "lastClr").and_then(rgb);
                    }
                }
            } else if NAMES.contains(&n) {
                slot = None;
            }
        })?;
        let mut indexed: Vec<_> = PALETTE.split_whitespace().map(rgb).collect();
        let mut custom = Vec::new();
        let mut fonts = Vec::new();
        let mut fills = Vec::new();
        let mut borders = Vec::new();
        let mut border: [Option<(BorderKind, RefColor)>; 4] = Default::default();
        let mut border_side = None;
        let mut bases = Vec::new();
        let mut xfs = Vec::new();
        let mut section = String::new();
        let mut font = RefColor::default();
        let mut fill = RefColor::default();
        let mut solid = false;
        xml(&styles_xml, |start, n, a| {
            if [
                "fonts",
                "fills",
                "borders",
                "cellStyleXfs",
                "cellXfs",
                "indexedColors",
            ]
            .contains(&n)
            {
                if start {
                    section = n.to_owned();
                } else {
                    section.clear();
                }
                return;
            }
            if start {
                match (section.as_str(), n) {
                    ("borders", "border") => {
                        border = Default::default();
                        border_side = None;
                    }
                    ("borders", "top" | "right" | "bottom" | "left") => {
                        let side = ["top", "right", "bottom", "left"]
                            .iter()
                            .position(|s| *s == n)
                            .unwrap();
                        border_side = Some(side);
                        border[side] = attr(a, "style")
                            .and_then(BorderKind::parse)
                            .map(|kind| (kind, RefColor::default()));
                    }
                    ("borders", "color") => {
                        if let Some(side) = border_side
                            && let Some((_, c)) = &mut border[side]
                        {
                            *c = color(a);
                        }
                    }
                    ("fonts", "font") => font = RefColor::default(),
                    ("fonts", "color") => font = color(a),
                    ("fills", "fill") => {
                        fill = RefColor::default();
                        solid = false;
                    }
                    ("fills", "patternFill") => solid = attr(a, "patternType") == Some("solid"),
                    ("fills", "fgColor") => fill = color(a),
                    ("indexedColors", "rgbColor") => custom.push(attr(a, "rgb").and_then(rgb)),
                    ("cellStyleXfs", "xf") => bases.push(a.clone()),
                    ("cellXfs", "xf") => xfs.push(a.clone()),
                    _ => {}
                }
            } else {
                match (section.as_str(), n) {
                    ("borders", "border") => borders.push(border.clone()),
                    ("borders", "top" | "right" | "bottom" | "left") => border_side = None,
                    ("fonts", "font") => fonts.push(font.clone()),
                    ("fills", "fill") => fills.push(if solid {
                        fill.clone()
                    } else {
                        RefColor::default()
                    }),
                    _ => {}
                }
            }
        })?;
        if !custom.is_empty() {
            indexed = custom;
        }
        let resolve = |c: &RefColor| -> Option<Color> {
            let base = c
                .rgb
                .as_deref()
                .and_then(rgb)
                .or_else(|| c.theme.and_then(|i| theme.get(i).copied().flatten()))
                .or_else(|| c.indexed.and_then(|i| indexed.get(i).copied().flatten()));
            base.map(|v| tint(v, c.tint))
        };
        let styles = xfs
            .iter()
            .map(|xf| {
                let parent = attr(xf, "xfId")
                    .and_then(|s| s.parse::<usize>().ok())
                    .and_then(|i| bases.get(i));
                let id = |key: &str, apply: &str| {
                    let direct = if attr(xf, apply) == Some("0") {
                        None
                    } else {
                        attr(xf, key)
                    };
                    direct
                        .or_else(|| parent.and_then(|p| attr(p, key)))
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0)
                };
                let defaults = CellColors::default();
                CellColors {
                    fg: fonts
                        .get(id("fontId", "applyFont"))
                        .and_then(resolve)
                        .unwrap_or(defaults.fg),
                    bg: fills
                        .get(id("fillId", "applyFill"))
                        .and_then(resolve)
                        .unwrap_or(defaults.bg),
                    borders: borders
                        .get(id("borderId", "applyBorder"))
                        .map(|sides| {
                            sides.each_ref().map(|side| {
                                side.as_ref().map(|(kind, c)| CellBorder {
                                    kind: *kind,
                                    color: resolve(c).unwrap_or(Color::Rgb(0, 0, 0)),
                                })
                            })
                        })
                        .unwrap_or([None; 4]),
                }
            })
            .collect();
        Ok(Self { styles })
    }
}
// Tint changes HLS luminance, preserving hue and saturation.
fn tint(c: Color, t: f64) -> Color {
    let Color::Rgb(r, g, b) = c else {
        return c;
    };
    if !t.is_finite() || t == 0.0 {
        return c;
    }
    let [r, g, b] = [r, g, b].map(|v| f64::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let l = (max + min) / 2.0;
    let s = if d == 0.0 {
        0.0
    } else {
        d / (1.0 - (2.0 * l - 1.0).abs())
    };
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let t = t.clamp(-1.0, 1.0);
    let l = if t < 0.0 {
        l * (1.0 + t)
    } else {
        l * (1.0 - t) + t
    };
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = chroma * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let m = l - chroma / 2.0;
    let out = match h as u8 {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    }
    .map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8);
    Color::Rgb(out[0], out[1], out[2])
}
const PALETTE: &str = "000000 FFFFFF FF0000 00FF00 0000FF FFFF00 FF00FF 00FFFF 000000 FFFFFF FF0000 00FF00 0000FF FFFF00 FF00FF 00FFFF 800000 008000 000080 808000 800080 008080 C0C0C0 808080 9999FF 993366 FFFFCC CCFFFF 660066 FF8080 0066CC CCCCFF 000080 FF00FF FFFF00 00FFFF 800080 800000 008080 0000FF 00CCFF CCFFFF CCFFCC FFFF99 99CCFF FF99CC CC99FF FFCC99 3366FF 33CCCC 99CC00 FFCC00 FF9900 FF6600 666699 969696 003366 339966 003300 333300 993300 993366 333399 333333";

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tint_luminance_and_argb() {
        assert_eq!(rgb("FF123456"), Some(Color::Rgb(18, 52, 86)));
        assert_eq!(rgb("nonsense"), None);
        assert_eq!(
            tint(Color::Rgb(79, 129, 189), 0.4),
            Color::Rgb(149, 179, 215)
        );
        assert_eq!(tint(Color::Rgb(79, 129, 189), -1.0), Color::Rgb(0, 0, 0));
        assert_eq!(
            tint(Color::Rgb(79, 129, 189), 1.0),
            Color::Rgb(255, 255, 255)
        );
    }
}
