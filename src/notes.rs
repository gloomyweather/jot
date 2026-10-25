//! Notes on disk: one markdown file per note, named after its title.
//!
//! The folder stays a plain folder of `.md` files, so notes can be edited,
//! synced or grepped with anything else. jot only adds two hidden things:
//! `.jot.json` (which notes are pinned) and `.trash/` (deleted notes).

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const STATE_FILE: &str = ".jot.json";
const TRASH_DIR: &str = ".trash";
const MAX_SLUG: usize = 48;

#[derive(Debug, Clone)]
pub struct Note {
    pub path: PathBuf,
    pub text: String,
    pub modified: SystemTime,
    pub pinned: bool,
}

impl Note {
    pub fn title(&self) -> String {
        title_of(&self.text)
    }

    /// The first line after the title, without markdown markers.
    pub fn snippet(&self) -> String {
        let line = content_lines(&self.text)
            .nth(1)
            .map(strip_markers)
            .unwrap_or_default();
        line.replace("**", "").replace("~~", "").replace('`', "")
    }

    pub fn words(&self) -> usize {
        self.text.split_whitespace().count()
    }
}

fn content_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("```"))
}

/// The first non-empty line, without markdown markers.
pub fn title_of(text: &str) -> String {
    content_lines(text)
        .map(strip_markers)
        .find(|l| !l.is_empty())
        .unwrap_or("untitled")
        .to_string()
}

fn strip_markers(line: &str) -> &str {
    let mut l = line.trim().trim_start_matches('#').trim_start();
    for marker in [
        "- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "- ", "* ", "+ ", "> ",
    ] {
        if let Some(rest) = l.strip_prefix(marker) {
            l = rest;
            break;
        }
    }
    l.trim()
}

/// File-name stem for a title: lowercase letters and digits joined by dashes.
/// Non-latin letters are kept, so a Russian title stays readable.
pub fn slugify(title: &str) -> String {
    let mut s = String::new();
    let mut len = 0;
    for c in title.chars().flat_map(char::to_lowercase) {
        if len >= MAX_SLUG {
            break;
        }
        if c.is_alphanumeric() {
            s.push(c);
            len += 1;
        } else if !s.is_empty() && !s.ends_with('-') {
            s.push('-');
            len += 1;
        }
    }
    let s = s.trim_end_matches('-');
    if s.is_empty() {
        "note".to_string()
    } else {
        s.to_string()
    }
}

/// Whether `stem` is `slug` or `slug-N`, i.e. a name jot gave this note.
fn stem_matches(stem: &str, slug: &str) -> bool {
    match stem.strip_prefix(slug) {
        Some("") => true,
        Some(rest) => rest
            .strip_prefix('-')
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())),
        None => false,
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    #[serde(default)]
    pinned: BTreeSet<String>,
}

pub struct Store {
    dir: PathBuf,
    state: State,
    /// Fingerprint of the folder at the last load, to notice outside edits.
    stamp: u64,
}

impl Store {
    pub fn open(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
        let state = fs::read_to_string(dir.join(STATE_FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        Ok(Self { dir, state, stamp: 0 })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn entries(&self) -> io::Result<Vec<(PathBuf, fs::Metadata)>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            if hidden || path.extension().is_none_or(|e| e != "md") {
                continue;
            }
            match entry.metadata() {
                Ok(meta) if meta.is_file() => out.push((path, meta)),
                _ => {}
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(out)
    }

    fn stamp_of(entries: &[(PathBuf, fs::Metadata)]) -> u64 {
        let mut h = DefaultHasher::new();
        for (path, meta) in entries {
            path.hash(&mut h);
            meta.len().hash(&mut h);
            meta.modified().ok().hash(&mut h);
        }
        h.finish()
    }

    /// True when notes were added, removed or edited since the last load.
    pub fn changed(&self) -> bool {
        self.entries().is_ok_and(|e| Self::stamp_of(&e) != self.stamp)
    }

    /// All notes: pinned first, then the most recently edited.
    pub fn load(&mut self) -> Result<Vec<Note>> {
        let entries = self
            .entries()
            .with_context(|| format!("could not read {}", self.dir.display()))?;
        self.stamp = Self::stamp_of(&entries);
        let mut notes: Vec<Note> = entries
            .into_iter()
            .filter_map(|(path, meta)| {
                let bytes = fs::read(&path).ok()?;
                Some(Note {
                    pinned: self.state.pinned.contains(&file_name(&path)),
                    text: String::from_utf8_lossy(&bytes).into_owned(),
                    modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    path,
                })
            })
            .collect();
        notes.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.modified.cmp(&a.modified)));
        Ok(notes)
    }

    /// Write `text` as a new note, or over `old` (path and its previous text).
    /// A note whose file name came from its title is renamed when the title changes.
    pub fn save(&mut self, old: Option<(&Path, &str)>, text: &str) -> Result<PathBuf> {
        let slug = slugify(&title_of(text));
        let old = old.filter(|(path, _)| path.exists());
        let target = match old {
            Some((path, old_text)) => {
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let named_by_us = stem_matches(&stem, &slugify(&title_of(old_text)));
                if named_by_us && !stem_matches(&stem, &slug) {
                    self.free_path(&slug)
                } else {
                    path.to_path_buf()
                }
            }
            None => self.free_path(&slug),
        };

        let tmp = self.dir.join(format!(".{}.tmp", file_name(&target)));
        fs::write(&tmp, text).with_context(|| format!("could not write {}", tmp.display()))?;
        fs::rename(&tmp, &target).with_context(|| format!("could not write {}", target.display()))?;

        if let Some((path, _)) = old {
            if path != target {
                fs::remove_file(path).with_context(|| format!("could not remove {}", path.display()))?;
                if self.state.pinned.remove(&file_name(path)) {
                    self.state.pinned.insert(file_name(&target));
                    self.save_state()?;
                }
            }
        }
        Ok(target)
    }

    /// `dir/slug.md`, or `dir/slug-2.md` and so on if that is taken.
    fn free_path(&self, slug: &str) -> PathBuf {
        let mut path = self.dir.join(format!("{slug}.md"));
        let mut n = 2;
        while path.exists() {
            path = self.dir.join(format!("{slug}-{n}.md"));
            n += 1;
        }
        path
    }

    /// Move a note to `.trash/`. Returns where it went.
    pub fn trash(&mut self, path: &Path) -> Result<PathBuf> {
        let trash = self.dir.join(TRASH_DIR);
        fs::create_dir_all(&trash).with_context(|| format!("could not create {}", trash.display()))?;
        let mut dest = trash.join(file_name(path));
        if dest.exists() {
            let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            dest = trash.join(format!("{stem}.{ts}.md"));
        }
        fs::rename(path, &dest).with_context(|| format!("could not move {} to the trash", path.display()))?;
        if self.state.pinned.remove(&file_name(path)) {
            self.save_state()?;
        }
        Ok(dest)
    }

    /// Bring a trashed note back to `original` (or a free name next to it).
    pub fn restore(&mut self, trashed: &Path, original: &Path) -> Result<PathBuf> {
        let dest = if original.exists() {
            let stem = original
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.free_path(&stem)
        } else {
            original.to_path_buf()
        };
        fs::rename(trashed, &dest).with_context(|| format!("could not restore {}", dest.display()))?;
        Ok(dest)
    }

    pub fn set_pinned(&mut self, path: &Path, pinned: bool) -> Result<()> {
        let name = file_name(path);
        let changed = if pinned {
            self.state.pinned.insert(name)
        } else {
            self.state.pinned.remove(&name)
        };
        if changed {
            self.save_state()?;
        }
        Ok(())
    }

    fn save_state(&self) -> Result<()> {
        let path = self.dir.join(STATE_FILE);
        let tmp = self.dir.join(format!("{STATE_FILE}.tmp"));
        fs::write(&tmp, serde_json::to_string_pretty(&self.state)?)?;
        fs::rename(&tmp, &path).with_context(|| format!("could not write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_slugs() {
        assert_eq!(title_of("\n\n# Groceries\n- milk"), "Groceries");
        assert_eq!(title_of("- [ ] call mom"), "call mom");
        assert_eq!(title_of("   \n"), "untitled");
        assert_eq!(slugify("Idea: a Telegram bot!"), "idea-a-telegram-bot");
        assert_eq!(slugify("Список покупок"), "список-покупок");
        assert_eq!(slugify("???"), "note");
        assert!(slugify(&"x".repeat(200)).chars().count() <= MAX_SLUG);
        assert!(stem_matches("groceries-12", "groceries"));
        assert!(!stem_matches("groceries-list", "groceries"));
    }

    #[test]
    fn save_rename_trash_restore() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("notes")).unwrap();

        let a = store.save(None, "# Groceries\nmilk").unwrap();
        assert_eq!(file_name(&a), "groceries.md");
        let b = store.save(None, "Groceries").unwrap();
        assert_eq!(file_name(&b), "groceries-2.md");

        // Retitling renames the file and carries the pin over.
        store.set_pinned(&a, true).unwrap();
        let a2 = store
            .save(Some((&a, "# Groceries\nmilk")), "# Shopping\nmilk")
            .unwrap();
        assert_eq!(file_name(&a2), "shopping.md");
        assert!(!a.exists());
        let notes = store.load().unwrap();
        assert_eq!(notes.len(), 2);
        assert!(notes[0].pinned && notes[0].path == a2);

        // A file named by hand keeps its name.
        let manual = store.dir().join("my-list.md");
        fs::write(&manual, "Things").unwrap();
        let kept = store.save(Some((&manual, "Things")), "Other things").unwrap();
        assert_eq!(kept, manual);

        store.load().unwrap();
        assert!(!store.changed());
        let trashed = store.trash(&a2).unwrap();
        assert!(store.changed());
        assert!(!a2.exists() && trashed.exists());
        assert_eq!(store.load().unwrap().len(), 2);
        let back = store.restore(&trashed, &a2).unwrap();
        assert_eq!(back, a2);
        assert_eq!(fs::read_to_string(back).unwrap(), "# Shopping\nmilk");
    }
}
