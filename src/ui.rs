//! Drawing.

use std::time::SystemTime;

use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block as Panel, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Mode};
use crate::editor::{char_width, TAB_WIDTH};
use crate::markdown::{self, Block};
use crate::theme::Palette;

const LOGO: &str = "◆";
const PIN: &str = "󰐃 ";
const SEARCH: &str = "󰍉 ";
const BAR: &str = "▌";
/// Rows per item in the note list: title, snippet, gap.
const ITEM_HEIGHT: u16 = 3;
const EDITOR_MAX_WIDTH: u16 = 86;

pub fn draw(f: &mut Frame, app: &mut App, p: &Palette) {
    let area = f.area().inner(Margin {
        horizontal: 2,
        vertical: 1,
    });
    if area.width < 20 || area.height < 6 {
        return;
    }
    let [header, rule, _, body, _, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(f, app, p, header);
    f.render_widget(
        Line::styled("─".repeat(rule.width as usize), Style::new().fg(p.border)),
        rule,
    );
    match app.mode {
        Mode::Edit => draw_editor(f, app, p, body),
        Mode::Help if app.editing.is_some() => draw_editor(f, app, p, body),
        _ => draw_browser(f, app, p, body),
    }
    draw_footer(f, app, p, footer);
    if app.mode == Mode::Help {
        draw_help(f, p, area);
    }
}

fn muted(p: &Palette) -> Style {
    Style::new().fg(p.muted)
}

fn draw_header(f: &mut Frame, app: &App, p: &Palette, area: Rect) {
    let dot = Span::styled("  ·  ", Style::new().fg(p.border));
    let mut left = vec![
        Span::styled(format!("{LOGO} "), Style::new().fg(p.accent)),
        Span::styled("jot", Style::new().fg(p.fg).add_modifier(Modifier::BOLD)),
        dot.clone(),
    ];
    let right: Vec<Span> = match &app.editing {
        Some(e) => {
            left.push(Span::styled(
                if e.is_new { "new note" } else { "editing" },
                muted(p),
            ));
            let state = if e.editor.changed {
                Span::styled("● unsaved", Style::new().fg(p.accent))
            } else {
                Span::styled("saved", muted(p))
            };
            vec![
                Span::styled(plural(e.editor.words(), "word"), muted(p)),
                dot,
                state,
            ]
        }
        None => {
            let count = if app.query.is_empty() {
                plural(app.notes.len(), "note")
            } else {
                format!("{} of {}", app.visible.len(), app.notes.len())
            };
            left.push(Span::styled(count, muted(p)));
            vec![Span::styled(tilde(app.store.dir()), muted(p))]
        }
    };
    let (left, right) = (Line::from(left), Line::from(right));
    // Drop the right side rather than overlap on narrow windows.
    if left.width() + right.width() + 2 <= area.width as usize {
        f.render_widget(right.right_aligned(), area);
    }
    f.render_widget(left, area);
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn tilde(path: &std::path::Path) -> String {
    match dirs::home_dir().and_then(|h| path.strip_prefix(h).ok().map(|r| r.to_path_buf())) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// "now", "5m", "3h", "2d", "12 oct".
fn ago(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match secs {
        0..60 => "now".into(),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h", secs / 3600),
        86400..604800 => format!("{}d", secs / 86400),
        _ => chrono::DateTime::<chrono::Local>::from(t)
            .format("%-d %b")
            .to_string()
            .to_lowercase(),
    }
}

/// Cut `s` to `width` columns, with an ellipsis if it was longer.
fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn draw_browser(f: &mut Frame, app: &mut App, p: &Palette, area: Rect) {
    if app.notes.is_empty() && app.query.is_empty() && app.mode != Mode::Search {
        return draw_empty(f, p, area);
    }
    let list_w = (area.width * 38 / 100).clamp(26.min(area.width), 46);
    let [list, _, sep, _, preview] = Layout::horizontal([
        Constraint::Length(list_w),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(1),
    ])
    .areas(area);

    let [search, _, items] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(list);
    draw_search(f, app, p, search);
    draw_list(f, app, p, items);

    let sep_lines: Vec<Line> = (0..sep.height)
        .map(|_| Line::styled("│", Style::new().fg(p.border)))
        .collect();
    f.render_widget(Paragraph::new(sep_lines), sep);
    draw_preview(f, app, p, preview);
}

fn draw_empty(f: &mut Frame, p: &Palette, area: Rect) {
    let lines = vec![
        Line::styled(LOGO, Style::new().fg(p.accent)),
        Line::default(),
        Line::styled(
            "no notes yet",
            Style::new().fg(p.fg).add_modifier(Modifier::ITALIC),
        ),
        Line::default(),
        Line::from(vec![
            Span::styled("press ", muted(p)),
            Span::styled("n", Style::new().fg(p.accent)),
            Span::styled(" to write the first one", muted(p)),
        ]),
    ];
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 3;
    let rect = Rect {
        y,
        height: h.min(area.height),
        ..area
    };
    f.render_widget(Paragraph::new(lines).centered(), rect);
}

fn draw_search(f: &mut Frame, app: &App, p: &Palette, area: Rect) {
    let active = app.mode == Mode::Search;
    let icon = Style::new().fg(if active || !app.query.is_empty() {
        p.accent
    } else {
        p.muted
    });
    let mut spans = vec![Span::styled(SEARCH, icon)];
    if app.query.is_empty() && !active {
        spans.push(Span::styled("search", muted(p)));
    } else {
        let room = (area.width as usize).saturating_sub(4);
        // Show the end of a long query, where the cursor is.
        let shown: String = {
            let mut w = 0;
            let mut chars: Vec<char> = app
                .query
                .chars()
                .rev()
                .take_while(|c| {
                    w += char_width(*c);
                    w <= room
                })
                .collect();
            chars.reverse();
            chars.into_iter().collect()
        };
        spans.push(Span::styled(shown.clone(), Style::new().fg(p.fg)));
        if active {
            let x = area.x + 2 + shown.width() as u16;
            f.set_cursor_position(Position {
                x: x.min(area.right() - 1),
                y: area.y,
            });
        }
    }
    f.render_widget(Line::from(spans), area);
}

fn draw_list(f: &mut Frame, app: &mut App, p: &Palette, area: Rect) {
    if app.visible.is_empty() {
        let lines = vec![
            Line::styled(
                truncate(&format!("nothing matches “{}”", app.query), area.width as usize),
                muted(p),
            ),
            Line::default(),
            Line::from(vec![
                Span::styled("⏎ ", Style::new().fg(p.accent)),
                Span::styled("make it a new note", muted(p)),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), area);
        return;
    }

    let capacity = ((area.height + 1) / ITEM_HEIGHT).max(1) as usize;
    if app.selected < app.list_offset {
        app.list_offset = app.selected;
    } else if app.selected >= app.list_offset + capacity {
        app.list_offset = app.selected + 1 - capacity;
    }
    app.list_offset = app.list_offset.min(app.visible.len().saturating_sub(capacity));

    let width = area.width as usize;
    let mut lines = Vec::new();
    for (vi, &ni) in app
        .visible
        .iter()
        .enumerate()
        .skip(app.list_offset)
        .take(capacity)
    {
        let note = &app.notes[ni];
        let selected = vi == app.selected;
        let bg = if selected && !p.reverse_selection {
            Style::new().bg(p.selection_bg)
        } else {
            Style::new()
        };
        let bar = Span::styled(
            if selected { BAR } else { " " },
            Style::new().fg(p.accent).bg(if selected && !p.reverse_selection {
                p.selection_bg
            } else {
                ratatui::style::Color::Reset
            }),
        );

        let time = ago(note.modified);
        let pin_w = if note.pinned { PIN.width() } else { 0 };
        let title_room = width.saturating_sub(2 + pin_w + time.width() + 2);
        let title = truncate(&note.title(), title_room);
        let mut title_style = Style::new().fg(p.fg);
        if selected {
            title_style = title_style.add_modifier(Modifier::BOLD);
        }
        let mut top = vec![bar.clone(), Span::raw(" ")];
        if note.pinned {
            top.push(Span::styled(PIN, Style::new().fg(p.accent)));
        }
        top.push(Span::styled(title.clone(), title_style));
        let pad = width.saturating_sub(2 + pin_w + title.width() + time.width() + 1);
        top.push(Span::raw(" ".repeat(pad)));
        top.push(Span::styled(time, muted(p)));
        top.push(Span::raw(" "));
        lines.push(Line::from(top).style(bg));

        let snippet = truncate(&note.snippet(), width.saturating_sub(3));
        let snippet_w = snippet.width();
        lines.push(
            Line::from(vec![
                bar,
                Span::raw(" "),
                Span::styled(snippet, muted(p)),
                Span::raw(" ".repeat(width.saturating_sub(2 + snippet_w))),
            ])
            .style(bg),
        );
        lines.push(Line::default());
    }
    f.render_widget(Paragraph::new(lines), area);
}

fn draw_preview(f: &mut Frame, app: &mut App, p: &Palette, area: Rect) {
    let Some(note) = app.selected_note() else { return };
    let [meta, _, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let edited = match ago(note.modified).as_str() {
        "now" => "edited just now".to_string(),
        t if t.ends_with(['m', 'h', 'd']) => format!("edited {t} ago"),
        t => format!("edited {t}"),
    };
    let dot = Span::styled(" · ", Style::new().fg(p.border));
    let mut spans = vec![
        Span::styled(edited, muted(p)),
        dot.clone(),
        Span::styled(plural(note.words(), "word"), muted(p)),
    ];
    if note.pinned {
        spans.extend([dot, Span::styled("pinned", Style::new().fg(p.accent))]);
    }
    f.render_widget(Line::from(spans), meta);

    let lines = markdown::render(&note.text, p, body.width as usize);
    // Clamp the scroll to the wrapped height of the note.
    let w = body.width.max(1) as usize;
    let total: usize = lines.iter().map(|l| l.width().max(1).div_ceil(w)).sum();
    let max = total.saturating_sub(body.height as usize) as u16;
    app.preview_scroll = app.preview_scroll.min(max);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((app.preview_scroll, 0)),
        body,
    );
}

fn draw_editor(f: &mut Frame, app: &mut App, p: &Palette, area: Rect) {
    let Some(e) = &mut app.editing else { return };
    let width = area.width.min(EDITOR_MAX_WIDTH);
    let area = Rect {
        x: area.x + (area.width - width) / 2,
        width,
        ..area
    };
    let ed = &mut e.editor;
    ed.width = area.width as usize;
    ed.height = area.height as usize;

    let rows = ed.rows();
    let (cy, cx) = ed.scroll_to_cursor(&rows);
    let blocks = markdown::classify(ed.lines());
    let empty = ed.is_blank();

    let mut lines = Vec::new();
    for row in rows.iter().skip(ed.scroll).take(area.height as usize) {
        let line = &ed.lines()[row.line];
        let (block, marker_bytes) = blocks[row.line];
        let marker_chars = line[..marker_bytes.min(line.len())].chars().count();
        let base = markdown::block_style(block, p);
        // Fences and rules are drawn whole in their own color.
        let marker = if matches!(block, Block::Fence | Block::Rule) {
            base
        } else {
            markdown::marker_style(block, p)
        };
        let mut spans = Vec::new();
        let mut chunk = String::new();
        let mut in_marker = true;
        for (i, c) in line.chars().enumerate().skip(row.start).take(row.end - row.start) {
            let now_marker = i < marker_chars;
            if now_marker != in_marker && !chunk.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut chunk),
                    if in_marker { marker } else { base },
                ));
            }
            in_marker = now_marker;
            if c == '\t' {
                chunk.push_str(&" ".repeat(TAB_WIDTH));
            } else {
                chunk.push(c);
            }
        }
        if !chunk.is_empty() {
            spans.push(Span::styled(chunk, if in_marker { marker } else { base }));
        }
        lines.push(Line::from(spans));
    }
    if empty {
        lines = vec![Line::styled(
            "start typing… the first line is the title",
            muted(p).add_modifier(Modifier::ITALIC),
        )];
    }
    f.render_widget(Paragraph::new(lines), area);
    if app.mode == Mode::Edit {
        f.set_cursor_position(Position {
            x: area.x + cx as u16,
            y: area.y + cy as u16,
        });
    }
}

fn hints(app: &App) -> Vec<(&'static str, &'static str)> {
    match app.mode {
        Mode::Edit => vec![
            ("esc", "done"),
            ("ctrl+t", "checkbox"),
            ("tab", "indent"),
            ("ctrl+z", "undo"),
            ("ctrl+s", "save"),
        ],
        Mode::Search if app.visible.is_empty() => vec![("⏎", "new note"), ("esc", "clear")],
        Mode::Search => vec![
            ("⏎", "open"),
            ("↑↓", "move"),
            ("tab", "keep filter"),
            ("esc", "clear"),
        ],
        Mode::Help => vec![("any key", "close")],
        Mode::List if app.notes.is_empty() => vec![("n", "new"), ("q", "quit")],
        Mode::List => vec![
            ("n", "new"),
            ("⏎", "edit"),
            ("/", "search"),
            ("p", "pin"),
            ("d", "delete"),
            ("y", "copy"),
            ("e", "nvim"),
            ("q", "quit"),
        ],
    }
}

fn draw_footer(f: &mut Frame, app: &App, p: &Palette, area: Rect) {
    if let Some(s) = &app.status {
        let color = if s.error { p.danger } else { p.accent };
        let line = Line::from(vec![
            Span::styled("● ", Style::new().fg(color)),
            Span::styled(
                truncate(&s.text, area.width as usize - 2),
                Style::new().fg(if s.error { p.danger } else { p.fg }),
            ),
        ]);
        f.render_widget(line, area);
        return;
    }
    let help = ("?", "help");
    let help = if app.mode == Mode::Edit {
        ("f1", "help")
    } else {
        help
    };
    let item_w = |(k, d): (&str, &str)| k.width() + d.width() + 4;
    let mut room = (area.width as usize).saturating_sub(item_w(help));
    let mut spans = Vec::new();
    let all = hints(app);
    let with_help = app.mode != Mode::Help;
    for item in all.into_iter().chain(with_help.then_some(help)) {
        let w = item_w(item);
        if item != help && w > room {
            continue;
        }
        room = room.saturating_sub(w);
        spans.push(Span::styled(item.0, Style::new().fg(p.accent)));
        spans.push(Span::styled(format!(" {}   ", item.1), muted(p)));
    }
    f.render_widget(Line::from(spans), area);
}

fn draw_help(f: &mut Frame, p: &Palette, area: Rect) {
    let sections: [(&str, &[(&str, &str)]); 2] = [
        (
            "notes",
            &[
                ("n", "new note"),
                ("⏎  i", "edit in place"),
                ("e", "open in $EDITOR"),
                ("/  f", "search, ⏎ on no match makes a note"),
                ("j k  ↑ ↓", "move"),
                ("J K", "scroll the preview"),
                ("p", "pin to the top"),
                ("d", "delete (to .trash)"),
                ("u", "undo delete"),
                ("y", "copy note to clipboard"),
                ("q  esc", "quit"),
            ],
        ),
        (
            "editor",
            &[
                ("esc", "save and close"),
                ("ctrl+t", "toggle checkbox - [ ]"),
                ("tab  ⇧tab", "indent / outdent"),
                ("ctrl+z  ctrl+y", "undo / redo"),
                ("ctrl+← →", "jump by word"),
                ("ctrl+w", "delete word"),
                ("ctrl+s", "save now (also autosaves)"),
            ],
        ),
    ];
    let key_w = 16;
    let mut lines = Vec::new();
    for (i, (title, keys)) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::styled(
            *title,
            Style::new().fg(p.fg).add_modifier(Modifier::BOLD),
        ));
        for (k, d) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {k:<key_w$}"), Style::new().fg(p.accent)),
                Span::styled(*d, muted(p)),
            ]));
        }
    }
    let w = 60.min(area.width);
    let h = (lines.len() as u16 + 4).min(area.height);
    let rect = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Panel::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(p.accent))
        .title(Span::styled(" keys ", Style::new().fg(p.fg)))
        .padding(ratatui::widgets::Padding::new(2, 2, 1, 0));
    f.render_widget(Paragraph::new(lines).block(block), rect);
}
