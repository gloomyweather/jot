//! Just enough markdown to make notes look good: headings, lists, checkboxes,
//! quotes, code, rules, **bold**, `code`, ~~strike~~ and links.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Palette;

pub const BOX_OPEN: &str = "󰄱 ";
pub const BOX_DONE: &str = "󰄵 ";

/// How a line of a note looks, by its markdown block type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Heading(usize),
    Task { done: bool },
    Bullet,
    Numbered,
    Quote,
    Rule,
    Fence,
    Code,
    Text,
}

/// Classify every line; returns the block type and the byte length of its marker.
pub fn classify(lines: &[impl AsRef<str>]) -> Vec<(Block, usize)> {
    let mut in_code = false;
    lines
        .iter()
        .map(|line| {
            let line = line.as_ref();
            let body = line.trim_start();
            let indent = line.len() - body.len();
            if body.starts_with("```") {
                in_code = !in_code;
                return (Block::Fence, line.len());
            }
            if in_code {
                return (Block::Code, 0);
            }
            let hashes = body.bytes().take_while(|b| *b == b'#').count();
            if (1..=6).contains(&hashes) && body[hashes..].starts_with(' ') {
                return (Block::Heading(hashes), indent + hashes + 1);
            }
            for (m, done) in [
                ("- [ ] ", false),
                ("* [ ] ", false),
                ("- [x] ", true),
                ("- [X] ", true),
                ("* [x] ", true),
            ] {
                if body.starts_with(m) {
                    return (Block::Task { done }, indent + m.len());
                }
            }
            let trimmed = body.trim_end();
            if trimmed.len() >= 3
                && ["-", "*", "_"]
                    .iter()
                    .any(|c| trimmed.chars().all(|x| x.to_string() == *c))
            {
                return (Block::Rule, line.len());
            }
            if ["- ", "* ", "+ "].iter().any(|m| body.starts_with(m)) {
                return (Block::Bullet, indent + 2);
            }
            if body.starts_with("> ") || body == ">" {
                return (Block::Quote, indent + body.len().min(2));
            }
            let digits = body.bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 && body[digits..].starts_with(". ") {
                return (Block::Numbered, indent + digits + 2);
            }
            (Block::Text, 0)
        })
        .collect()
}

/// Base text style for a block.
pub fn block_style(block: Block, p: &Palette) -> Style {
    let fg = Style::new().fg(p.fg);
    match block {
        Block::Heading(1) => Style::new().fg(p.accent).add_modifier(Modifier::BOLD),
        Block::Heading(2) => fg.add_modifier(Modifier::BOLD),
        Block::Heading(_) => Style::new().fg(p.muted).add_modifier(Modifier::BOLD),
        Block::Task { done: true } => Style::new().fg(p.muted).add_modifier(Modifier::CROSSED_OUT),
        Block::Quote => Style::new().fg(p.muted).add_modifier(Modifier::ITALIC),
        Block::Fence | Block::Rule => Style::new().fg(p.border),
        Block::Code => Style::new().fg(p.muted),
        _ => fg,
    }
}

/// Style for a block's marker (`- `, `1. `, `# `).
pub fn marker_style(block: Block, p: &Palette) -> Style {
    match block {
        Block::Task { done: true } | Block::Heading(_) => Style::new().fg(p.border),
        _ => Style::new().fg(p.accent),
    }
}

/// Render a note for reading.
pub fn render(text: &str, p: &Palette, width: usize) -> Vec<Line<'static>> {
    let lines: Vec<&str> = text.lines().collect();
    classify(&lines)
        .into_iter()
        .zip(&lines)
        .map(|((block, marker), line)| {
            let base = block_style(block, p);
            let accent = marker_style(block, p);
            let indent = &line[..line.len() - line.trim_start().len()];
            let rest = line.get(marker..).unwrap_or("");
            let mut spans = vec![Span::raw(indent.to_string())];
            match block {
                Block::Fence => return Line::default(),
                Block::Code => {
                    return Line::from(vec![
                        Span::styled("│ ", Style::new().fg(p.border)),
                        Span::styled(line.to_string(), base),
                    ])
                }
                Block::Rule => return Line::styled("─".repeat(width.min(48)), base),
                Block::Heading(_) => spans.clear(),
                Block::Task { done } => {
                    let (glyph, style) = if done {
                        (BOX_DONE, Style::new().fg(p.muted))
                    } else {
                        (BOX_OPEN, Style::new().fg(p.accent))
                    };
                    spans.push(Span::styled(glyph, style));
                }
                Block::Bullet => spans.push(Span::styled("• ", accent)),
                Block::Numbered => spans.push(Span::styled(
                    line.trim_start()[..marker - indent.len()].to_string(),
                    accent,
                )),
                Block::Quote => spans.push(Span::styled("▎ ", accent)),
                Block::Text => {}
            }
            spans.extend(inline(rest, base, p));
            Line::from(spans)
        })
        .collect()
}

/// Inline markup inside a line.
pub fn inline(text: &str, base: Style, p: &Palette) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    let flush = |plain: &mut String, spans: &mut Vec<Span<'static>>| {
        if !plain.is_empty() {
            spans.push(Span::styled(std::mem::take(plain), base));
        }
    };
    while let Some(c) = rest.chars().next() {
        let delimited = [
            ("**", base.add_modifier(Modifier::BOLD)),
            ("~~", base.add_modifier(Modifier::CROSSED_OUT)),
            ("`", Style::new().fg(p.accent)),
        ]
        .into_iter()
        .find_map(|(d, style)| {
            let inner = rest.strip_prefix(d)?;
            let end = inner.find(d).filter(|e| *e > 0)?;
            Some((inner[..end].to_string(), style, d.len() * 2 + end))
        });
        if let Some((inner, style, len)) = delimited {
            flush(&mut plain, &mut spans);
            spans.push(Span::styled(inner, style));
            rest = &rest[len..];
            continue;
        }
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            flush(&mut plain, &mut spans);
            spans.push(Span::styled(
                rest[..end].to_string(),
                Style::new().fg(p.accent).add_modifier(Modifier::UNDERLINED),
            ));
            rest = &rest[end..];
            continue;
        }
        plain.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut plain, &mut spans);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks() {
        let lines = [
            "# Title",
            "- [ ] a",
            "  - [x] b",
            "- c",
            "1. d",
            "> e",
            "---",
            "```",
            "# not",
            "```",
            "plain",
            "#nospace",
        ];
        let kinds: Vec<Block> = classify(&lines).into_iter().map(|(b, _)| b).collect();
        assert_eq!(
            kinds,
            [
                Block::Heading(1),
                Block::Task { done: false },
                Block::Task { done: true },
                Block::Bullet,
                Block::Numbered,
                Block::Quote,
                Block::Rule,
                Block::Fence,
                Block::Code,
                Block::Fence,
                Block::Text,
                Block::Text
            ]
        );
        assert_eq!(classify(&["  - [x] b"])[0].1, 8);
    }

    #[test]
    fn inline_markup() {
        let p = Palette::terminal();
        let text: Vec<String> = inline("a **b** `c` d https://x.y ok **open", Style::new(), &p)
            .into_iter()
            .map(|s| s.content.into_owned())
            .collect();
        assert_eq!(text, ["a ", "b", " ", "c", " d ", "https://x.y", " ok **open"]);
    }
}
