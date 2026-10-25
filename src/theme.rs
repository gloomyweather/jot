//! Color palette: terminal ANSI colors, or the current Omarchy theme with live reload.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub fg: Color,
    pub muted: Color,
    pub accent: Color,
    pub selection_bg: Color,
    pub danger: Color,
    pub border: Color,
    /// Draw the selection with reversed video instead of `selection_bg`.
    pub reverse_selection: bool,
}

impl Palette {
    /// The terminal's own colors, so any terminal theme (light or dark) works.
    pub fn terminal() -> Self {
        Self {
            fg: Color::Reset,
            muted: Color::DarkGray,
            accent: Color::Blue,
            selection_bg: Color::Reset,
            danger: Color::Red,
            border: Color::DarkGray,
            reverse_selection: true,
        }
    }

    /// Build a palette from an Omarchy `colors.toml`. Missing keys fall back
    /// to terminal colors. The background is never painted.
    pub fn from_omarchy(colors_toml: &str) -> Result<Self> {
        let table: toml::Table = colors_toml.parse()?;
        let get = |key: &str| table.get(key).and_then(|v| v.as_str()).and_then(parse_hex);
        let rgb = |key: &str, fallback: Color| get(key).map_or(fallback, to_color);

        // Omarchy themes disagree on whether `muted` or `dark_foreground` is the
        // more readable one: use the higher-contrast one for text, the other for borders.
        let (muted, border) = match (get("muted"), get("dark_foreground")) {
            (Some(m), Some(d)) => match get("background") {
                Some(bg) if contrast(d, bg) > contrast(m, bg) => (Some(d), Some(m)),
                _ => (Some(m), Some(d)),
            },
            (one, other) => (one.or(other), one.or(other)),
        };

        let base = Self::terminal();
        let selection = get("selection");
        Ok(Self {
            fg: rgb("foreground", base.fg),
            muted: muted.map_or(base.muted, to_color),
            accent: rgb("accent", base.accent),
            selection_bg: selection.map_or(base.selection_bg, to_color),
            danger: rgb("red", base.danger),
            border: border.map_or(base.border, to_color),
            reverse_selection: selection.is_none(),
        })
    }
}

fn to_color((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.trim().strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

/// WCAG relative luminance contrast ratio.
fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    fn lum((r, g, b): (u8, u8, u8)) -> f32 {
        let ch = |c: u8| {
            let c = c as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    }
    let (la, lb) = (lum(a), lum(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Identity of the active Omarchy theme file, to notice theme switches.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThemeStamp {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    name: String,
}

/// Tracks the active theme and rebuilds the palette when it changes.
pub struct ThemeSource {
    enabled: bool,
    /// Omarchy `current` directories to look in, newest layout first.
    roots: Vec<PathBuf>,
    seen: Option<ThemeStamp>,
    palette: Palette,
}

impl ThemeSource {
    /// With `enabled` false the terminal colors are always used.
    pub fn new(enabled: bool, home: Option<&Path>) -> Self {
        let roots = home
            .map(|h| {
                vec![
                    h.join(".local/state/omarchy/current"),
                    h.join(".config/omarchy/current"),
                ]
            })
            .unwrap_or_default();
        let mut source = Self {
            enabled,
            roots,
            seen: None,
            palette: Palette::terminal(),
        };
        source.poll();
        source
    }

    pub fn palette(&self) -> Palette {
        self.palette
    }

    fn current(&self) -> Option<ThemeStamp> {
        if !self.enabled {
            return None;
        }
        self.roots.iter().find_map(|root| {
            let file = root.join("theme/colors.toml");
            let meta = fs::metadata(&file).ok()?;
            Some(ThemeStamp {
                path: fs::canonicalize(&file).unwrap_or(file),
                modified: meta.modified().ok(),
                len: meta.len(),
                name: fs::read_to_string(root.join("theme.name")).unwrap_or_default(),
            })
        })
    }

    /// Re-check the theme; returns true when the palette changed.
    pub fn poll(&mut self) -> bool {
        let current = self.current();
        if current == self.seen {
            return false;
        }
        let palette = match &current {
            None => Palette::terminal(),
            Some(stamp) => {
                // A theme switch may be mid-write; keep the old palette and retry next tick.
                let Ok(text) = fs::read_to_string(&stamp.path) else {
                    return false;
                };
                let Ok(p) = Palette::from_omarchy(&text) else {
                    return false;
                };
                p
            }
        };
        self.seen = current;
        let changed = palette != self.palette;
        self.palette = palette;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ember() {
        let p = Palette::from_omarchy(
            r##"accent = "#d97757"
foreground = "#ffffff"
background = "#000000"
muted = "#737373"
dark_foreground = "#525252"
selection = "#333333"
red = "#e5534b""##,
        )
        .unwrap();
        assert_eq!(p.accent, Color::Rgb(0xd9, 0x77, 0x57));
        assert_eq!(p.muted, Color::Rgb(0x73, 0x73, 0x73));
        assert_eq!(p.border, Color::Rgb(0x52, 0x52, 0x52));
        assert!(!p.reverse_selection);
    }

    #[test]
    fn partial_file_falls_back() {
        let p = Palette::from_omarchy("accent = \"#112233\"\nred = \"nonsense\"").unwrap();
        let t = Palette::terminal();
        assert_eq!(p.accent, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(p.danger, t.danger);
        assert!(p.reverse_selection);
        assert!(Palette::from_omarchy("not = [toml").is_err());
    }

    #[test]
    fn live_reload() {
        let home = tempfile::tempdir().unwrap();
        let current = home.path().join(".local/state/omarchy/current");
        fs::create_dir_all(current.join("theme")).unwrap();
        let mut src = ThemeSource::new(true, Some(home.path()));
        assert_eq!(src.palette(), Palette::terminal());

        fs::write(current.join("theme/colors.toml"), "accent = \"#d97757\"").unwrap();
        assert!(src.poll());
        assert_eq!(src.palette().accent, Color::Rgb(0xd9, 0x77, 0x57));
        assert!(!src.poll());

        let off = ThemeSource::new(false, Some(home.path()));
        assert_eq!(off.palette(), Palette::terminal());
    }
}
