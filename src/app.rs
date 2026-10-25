//! App state and key handling.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::editor::Editor;
use crate::notes::{Note, Store};

/// Save a note being edited after this much quiet time, so closing the window never loses text.
const AUTOSAVE_AFTER: Duration = Duration::from_millis(800);
const STATUS_FOR: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    List,
    Search,
    Edit,
    Help,
}

pub struct Editing {
    pub editor: Editor,
    /// The file being edited and its text as last saved; None for a new note not saved yet.
    pub saved: Option<(PathBuf, String)>,
    /// Created in this editing session (it may already be autosaved).
    pub is_new: bool,
    pub last_change: Instant,
}

pub struct Status {
    pub text: String,
    pub error: bool,
    at: Instant,
}

pub struct App {
    pub store: Store,
    pub notes: Vec<Note>,
    pub query: String,
    /// Indices into `notes` that match the query.
    pub visible: Vec<usize>,
    /// Index into `visible`.
    pub selected: usize,
    /// First visible list item, kept by the renderer.
    pub list_offset: usize,
    pub preview_scroll: u16,
    pub mode: Mode,
    /// Mode to go back to when help closes.
    help_from: Mode,
    pub editing: Option<Editing>,
    /// Started as a quick capture: quit when the note is closed.
    pub capture: bool,
    pub status: Option<Status>,
    last_trashed: Option<(PathBuf, PathBuf, bool)>,
    /// Ask the main loop to open this file in the external editor.
    pub external_edit: Option<PathBuf>,
    pub should_quit: bool,
}

impl App {
    pub fn new(mut store: Store) -> anyhow::Result<Self> {
        let notes = store.load()?;
        let mut app = Self {
            store,
            notes,
            query: String::new(),
            visible: Vec::new(),
            selected: 0,
            list_offset: 0,
            preview_scroll: 0,
            mode: Mode::List,
            help_from: Mode::List,
            editing: None,
            capture: false,
            status: None,
            last_trashed: None,
            external_edit: None,
            should_quit: false,
        };
        app.refilter(None);
        Ok(app)
    }

    pub fn selected_note(&self) -> Option<&Note> {
        self.visible.get(self.selected).map(|&i| &self.notes[i])
    }

    fn selected_path(&self) -> Option<PathBuf> {
        self.selected_note().map(|n| n.path.clone())
    }

    /// Recompute matches for the query; keep `keep` (or the current note) selected.
    fn refilter(&mut self, keep: Option<PathBuf>) {
        let keep = keep.or_else(|| self.selected_path());
        let words: Vec<String> = self
            .query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        self.visible = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                let text = n.text.to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .map(|(i, _)| i)
            .collect();
        let found = keep.and_then(|k| self.visible.iter().position(|&i| self.notes[i].path == k));
        let before = self.selected;
        self.selected = found.unwrap_or(self.selected.min(self.visible.len().saturating_sub(1)));
        if self.selected != before {
            self.preview_scroll = 0;
        }
    }

    /// Re-read the notes folder, keeping `keep` (or the current note) selected.
    pub fn reload(&mut self, keep: Option<PathBuf>) {
        match self.store.load() {
            Ok(notes) => {
                let keep = keep.or_else(|| self.selected_path());
                self.notes = notes;
                self.refilter(keep);
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: false,
            at: Instant::now(),
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.status = Some(Status {
            text: text.into(),
            error: true,
            at: Instant::now(),
        });
    }

    /// Housekeeping between key presses: autosave and expiring messages.
    pub fn tick(&mut self) {
        if let Some(e) = &self.editing {
            if e.editor.changed && !e.editor.is_blank() && e.last_change.elapsed() >= AUTOSAVE_AFTER {
                self.save_editing();
            }
        }
        if self
            .status
            .as_ref()
            .is_some_and(|s| !s.error && s.at.elapsed() >= STATUS_FOR)
        {
            self.status = None;
        }
    }

    // Notes

    pub fn new_note(&mut self, text: &str) {
        self.editing = Some(Editing {
            editor: Editor::new(text),
            saved: None,
            is_new: true,
            last_change: Instant::now(),
        });
        if let Some(e) = &mut self.editing {
            e.editor.changed = !text.is_empty();
        }
        self.mode = Mode::Edit;
    }

    fn edit_selected(&mut self) {
        let Some(note) = self.selected_note() else {
            return self.new_note("");
        };
        let saved = Some((note.path.clone(), note.text.clone()));
        let editor = Editor::new(&note.text);
        self.editing = Some(Editing {
            editor,
            saved,
            is_new: false,
            last_change: Instant::now(),
        });
        self.mode = Mode::Edit;
    }

    /// Write the note being edited. Returns false if that failed.
    fn save_editing(&mut self) -> bool {
        let Some(e) = &mut self.editing else { return true };
        let text = e.editor.text();
        let old = e.saved.as_ref().map(|(p, t)| (p.as_path(), t.as_str()));
        match self.store.save(old, &text) {
            Ok(path) => {
                e.saved = Some((path, text));
                e.editor.changed = false;
                true
            }
            Err(err) => {
                // Don't retry on every tick.
                e.last_change = Instant::now() + Duration::from_secs(5);
                self.error(format!("{err:#}"));
                false
            }
        }
    }

    /// Save whatever is being edited, before quitting.
    pub fn finish(&mut self) {
        self.capture = true;
        self.close_editor();
    }

    /// Save and leave the editor. A note emptied out goes to the trash.
    fn close_editor(&mut self) {
        let Some(e) = &self.editing else { return };
        let mut keep = e.saved.as_ref().map(|(p, _)| p.clone());
        if e.editor.is_blank() {
            if let Some(path) = keep.take() {
                self.trash(&path);
            }
        } else if e.editor.changed {
            if !self.save_editing() {
                return;
            }
            keep = self
                .editing
                .as_ref()
                .and_then(|e| e.saved.as_ref().map(|(p, _)| p.clone()));
            self.info("saved");
        }
        self.editing = None;
        if self.capture {
            self.should_quit = true;
            return;
        }
        self.mode = Mode::List;
        self.reload(keep);
    }

    fn trash(&mut self, path: &Path) {
        let pinned = self.notes.iter().any(|n| n.path == path && n.pinned);
        match self.store.trash(path) {
            Ok(dest) => {
                self.last_trashed = Some((dest, path.to_path_buf(), pinned));
                self.info("moved to trash · u to undo");
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn trash_selected(&mut self) {
        let Some(path) = self.selected_path() else { return };
        self.trash(&path);
        self.reload(None);
    }

    fn undo_trash(&mut self) {
        let Some((trashed, original, pinned)) = self.last_trashed.take() else {
            return self.info("nothing to undo");
        };
        match self.store.restore(&trashed, &original) {
            Ok(path) => {
                if pinned {
                    let _ = self.store.set_pinned(&path, true);
                }
                self.info("restored");
                self.reload(Some(path));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn toggle_pin(&mut self) {
        let Some(note) = self.selected_note() else { return };
        let (path, pinned) = (note.path.clone(), !note.pinned);
        match self.store.set_pinned(&path, pinned) {
            Ok(()) => {
                self.info(if pinned { "pinned" } else { "unpinned" });
                self.reload(Some(path));
            }
            Err(e) => self.error(format!("{e:#}")),
        }
    }

    fn copy_selected(&mut self) {
        let Some(text) = self.selected_note().map(|n| n.text.clone()) else {
            return;
        };
        let result = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .take()
                    .expect("piped stdin")
                    .write_all(text.as_bytes())?;
                child.wait()
            });
        match result {
            Ok(s) if s.success() => self.info("copied to clipboard"),
            _ => self.error("could not copy, is wl-clipboard installed?"),
        }
    }

    fn select(&mut self, index: usize) {
        let index = index.min(self.visible.len().saturating_sub(1));
        if index != self.selected {
            self.selected = index;
            self.preview_scroll = 0;
        }
    }

    fn move_by(&mut self, delta: isize) {
        let i = (self.selected as isize + delta).max(0) as usize;
        self.select(i);
    }

    // Keys

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.mode == Mode::Edit {
                self.close_editor();
            }
            self.should_quit = true;
            return;
        }
        match self.mode {
            Mode::List => self.list_key(key),
            Mode::Search => self.search_key(key),
            Mode::Edit => self.edit_key(key),
            Mode::Help => self.mode = self.help_from,
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match self.mode {
            Mode::Edit => {
                if let Some(e) = &mut self.editing {
                    e.editor.insert_str(text);
                    e.last_change = Instant::now();
                }
            }
            Mode::Search => {
                self.query.push_str(&text.replace(['\n', '\r'], " "));
                self.refilter(None);
            }
            // Pasting into the list starts a new note with it.
            Mode::List => self.new_note(text),
            Mode::Help => {}
        }
    }

    fn open_help(&mut self) {
        self.help_from = self.mode;
        self.mode = Mode::Help;
    }

    fn list_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = 5;
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                self.refilter(None);
            }
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1),
            KeyCode::Char('d') if ctrl => self.move_by(page),
            KeyCode::Char('u') if ctrl => self.move_by(-page),
            KeyCode::PageDown => self.move_by(page),
            KeyCode::PageUp => self.move_by(-page),
            KeyCode::Char('g') | KeyCode::Home => self.select(0),
            KeyCode::Char('G') | KeyCode::End => self.select(usize::MAX),
            KeyCode::Char('J') => self.preview_scroll = self.preview_scroll.saturating_add(3),
            KeyCode::Char('K') => self.preview_scroll = self.preview_scroll.saturating_sub(3),
            KeyCode::Char('n') | KeyCode::Char('a') | KeyCode::Char('o') => self.new_note(""),
            KeyCode::Enter | KeyCode::Char('i') | KeyCode::Char('l') | KeyCode::Right => self.edit_selected(),
            KeyCode::Char('e') => self.external_edit = self.selected_path(),
            KeyCode::Char('/') | KeyCode::Char('f') => self.mode = Mode::Search,
            KeyCode::Char('p') => self.toggle_pin(),
            KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => self.trash_selected(),
            KeyCode::Char('u') => self.undo_trash(),
            KeyCode::Char('y') => self.copy_selected(),
            KeyCode::Char('?') => self.open_help(),
            _ => {}
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.query.clear();
                self.refilter(None);
                self.mode = Mode::List;
            }
            KeyCode::Tab => self.mode = Mode::List,
            // No match: Enter turns the query into a new note.
            KeyCode::Enter if self.visible.is_empty() => {
                let text = std::mem::take(&mut self.query);
                self.refilter(None);
                self.new_note(&text);
            }
            KeyCode::Enter => self.edit_selected(),
            KeyCode::Down => self.move_by(1),
            KeyCode::Up => self.move_by(-1),
            KeyCode::Char('n') | KeyCode::Char('j') if ctrl => self.move_by(1),
            KeyCode::Char('p') | KeyCode::Char('k') if ctrl => self.move_by(-1),
            KeyCode::Char('w') | KeyCode::Char('h') if ctrl => {
                let trimmed = self.query.trim_end();
                let cut = trimmed.rfind(' ').map_or(0, |i| i + 1);
                self.query.truncate(cut);
                self.refilter(None);
            }
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.refilter(None);
            }
            KeyCode::Backspace if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.clear();
                self.refilter(None);
            }
            KeyCode::Backspace => {
                if self.query.pop().is_none() {
                    self.mode = Mode::List;
                }
                self.refilter(None);
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                self.query.push(c);
                self.refilter(None);
            }
            _ => {}
        }
    }

    fn edit_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Esc => return self.close_editor(),
            KeyCode::Char('s') if ctrl => {
                if self.save_editing() {
                    self.info("saved");
                }
                return;
            }
            KeyCode::F(1) => return self.open_help(),
            _ => {}
        }
        let Some(e) = &mut self.editing else { return };
        let ed = &mut e.editor;
        match key.code {
            KeyCode::Char('z') if ctrl && shift => _ = ed.redo(),
            KeyCode::Char('Z') if ctrl => _ = ed.redo(),
            KeyCode::Char('z') if ctrl => _ = ed.undo(),
            KeyCode::Char('y') if ctrl => _ = ed.redo(),
            KeyCode::Char('t') if ctrl => ed.toggle_checkbox(),
            KeyCode::Char('w') | KeyCode::Char('h') if ctrl => ed.delete_word_back(),
            KeyCode::Char('a') if ctrl => ed.home(),
            KeyCode::Char('e') if ctrl => ed.end(),
            KeyCode::Backspace if ctrl || key.modifiers.contains(KeyModifiers::ALT) => ed.delete_word_back(),
            KeyCode::Char(c) if !ctrl => ed.insert_char(c),
            KeyCode::Enter => ed.newline(),
            KeyCode::Backspace => ed.backspace(),
            KeyCode::Delete => ed.delete(),
            KeyCode::Tab => ed.indent(false),
            KeyCode::BackTab => ed.indent(true),
            KeyCode::Left if ctrl => ed.word_left(),
            KeyCode::Right if ctrl => ed.word_right(),
            KeyCode::Left => ed.left(),
            KeyCode::Right => ed.right(),
            KeyCode::Up => ed.vertical(-1),
            KeyCode::Down => ed.vertical(1),
            KeyCode::Home if ctrl => ed.doc_start(),
            KeyCode::End if ctrl => ed.doc_end(),
            KeyCode::Home => ed.home(),
            KeyCode::End => ed.end(),
            KeyCode::PageUp => ed.page(false),
            KeyCode::PageDown => ed.page(true),
            _ => {}
        }
        // Autosave waits for a pause in typing.
        if ed.changed {
            e.last_change = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_key(key(if c == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(c)
            }));
        }
    }

    #[test]
    fn write_search_edit_trash() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Store::open(dir.path().to_path_buf()).unwrap()).unwrap();

        app.handle_key(key(KeyCode::Char('n')));
        typed(&mut app, "Groceries\n- milk");
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::List);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("groceries.md")).unwrap(),
            "Groceries\n- milk"
        );

        // Unknown search + Enter creates a note titled after the query.
        app.handle_key(key(KeyCode::Char('/')));
        typed(&mut app, "call mom");
        assert!(app.visible.is_empty());
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Esc));
        assert!(dir.path().join("call-mom.md").exists());
        assert_eq!(app.notes.len(), 2);
        assert_eq!(app.selected_note().unwrap().title(), "call mom");

        // Emptying a note trashes it; u brings it back.
        app.handle_key(key(KeyCode::Enter));
        for _ in 0..8 {
            app.handle_key(key(KeyCode::Backspace));
        }
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.notes.len(), 1);
        app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(app.notes.len(), 2);

        // Search narrows the list.
        app.handle_key(key(KeyCode::Char('/')));
        typed(&mut app, "MILK");
        assert_eq!(app.visible.len(), 1);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.visible.len(), 2);
    }

    #[test]
    fn capture_quits_and_blank_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(Store::open(dir.path().to_path_buf()).unwrap()).unwrap();
        app.capture = true;
        app.new_note("");
        app.handle_key(key(KeyCode::Esc));
        assert!(app.should_quit);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
