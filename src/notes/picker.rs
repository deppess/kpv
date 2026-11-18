/* Choosing the note for a PDF: a new one named after it, one named by hand, or any
 * note already in the vault. Typing filters the list. */

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::state::{edit_prompt, PromptKey};

#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    /* A new note with this name */
    New(String),
    /* A note that exists */
    Open(PathBuf),
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    /* The new note named after the PDF, or the existing note with that name */
    Suggested(Choice),
    /* Asks for a name */
    Named,
    Note(PathBuf),
}

pub enum PickerKey {
    Handled,
    Close,
    Chosen(Choice),
}

pub struct Picker {
    entries: Vec<Entry>,
    vault: PathBuf,
    pub filter: String,
    /* A name being typed for a new note */
    pub naming: Option<String>,
    pub selected: usize,
    pub scroll: usize,
}

/* One line of the picker, for drawing */
pub struct PickerRow {
    pub label: String,
    pub hint: String,
    pub selected: bool,
}

impl Picker {
    /* `suggested` is the note's name made from the PDF; `existing` the note with that
     * name if the vault has one. `notes` are the vault's notes, newest first. */
    pub fn new(
        vault: &Path,
        suggested: &str,
        existing: Option<PathBuf>,
        notes: Vec<PathBuf>,
    ) -> Self {
        let first = match existing {
            Some(path) => Choice::Open(path),
            None => Choice::New(suggested.to_owned()),
        };
        let mut entries = vec![Entry::Suggested(first.clone()), Entry::Named];
        entries.extend(
            notes
                .into_iter()
                .filter(|p| first != Choice::Open(p.clone()))
                .map(Entry::Note),
        );
        Self {
            entries,
            vault: vault.to_path_buf(),
            filter: String::new(),
            naming: None,
            selected: 0,
            scroll: 0,
        }
    }

    /* Opens straight on typing a name */
    pub fn naming(mut self) -> Self {
        self.naming = Some(String::new());
        self
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.vault)
            .unwrap_or(path)
            .with_extension("")
            .to_string_lossy()
            .into_owned()
    }

    fn visible(&self) -> Vec<&Entry> {
        let filter = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|e| match e {
                Entry::Note(path) => {
                    filter.is_empty()
                        || self.relative(path).to_lowercase().contains(&filter)
                }
                _ => true,
            })
            .collect()
    }

    pub fn rows(&self) -> Vec<PickerRow> {
        self.visible()
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let (label, hint) = match e {
                    Entry::Suggested(Choice::New(name)) => (format!("+ {}", name), "new"),
                    Entry::Suggested(Choice::Open(path)) => {
                        (format!("  {}", self.relative(path)), "has the name")
                    }
                    Entry::Named => ("+ Name it yourself…".to_owned(), "new"),
                    Entry::Note(path) => (format!("  {}", self.relative(path)), ""),
                };
                PickerRow {
                    label,
                    hint: hint.to_owned(),
                    selected: i == self.selected,
                }
            })
            .collect()
    }

    /* Keeps the selection on screen in a list `visible` rows high */
    fn follow(&mut self, visible: usize) {
        let visible = visible.max(1);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        }
    }

    pub fn key(&mut self, key: KeyEvent, visible: usize) -> PickerKey {
        if let Some(name) = &mut self.naming {
            return match edit_prompt(name, key) {
                PromptKey::Edited => PickerKey::Handled,
                PromptKey::Cancel => {
                    self.naming = None;
                    PickerKey::Handled
                }
                PromptKey::Submit => {
                    let name = name.trim().trim_end_matches(".md").trim().to_owned();
                    self.naming = None;
                    if name.is_empty() {
                        PickerKey::Handled
                    } else {
                        PickerKey::Chosen(Choice::New(name))
                    }
                }
            };
        }

        let count = self.visible().len();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.selected = 0;
            }
            KeyCode::Esc => return PickerKey::Close,
            KeyCode::Down => {
                self.selected = (self.selected + 1).min(count.saturating_sub(1))
            }
            KeyCode::Char('n' | 'j') if ctrl => {
                self.selected = (self.selected + 1).min(count.saturating_sub(1))
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Char('p' | 'k') if ctrl => {
                self.selected = self.selected.saturating_sub(1)
            }
            KeyCode::Enter => {
                return match self.visible().get(self.selected) {
                    Some(Entry::Suggested(choice)) => PickerKey::Chosen(choice.clone()),
                    Some(Entry::Named) => {
                        self.naming = Some(String::new());
                        PickerKey::Handled
                    }
                    Some(Entry::Note(path)) => {
                        PickerKey::Chosen(Choice::Open(path.clone()))
                    }
                    None => PickerKey::Handled,
                }
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.selected = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                self.filter.push(c);
                /* The first note that matches, past the two entries for new notes */
                self.selected = if self.visible().len() > 2 { 2 } else { 0 };
            }
            _ => (),
        }
        self.follow(visible);
        PickerKey::Handled
    }

    /* The entry at row `row` of the list was clicked */
    pub fn click(&mut self, row: usize, visible: usize) -> PickerKey {
        let index = self.scroll + row;
        if index >= self.visible().len() {
            return PickerKey::Handled;
        }
        self.selected = index;
        self.follow(visible);
        self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), visible)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn picker(existing: bool) -> Picker {
        let vault = Path::new("/v");
        let notes = vec![
            vault.join("000 Rough Writes/Book.md"),
            vault.join("100 Writings/Essay.md"),
            vault.join("100 Writings/Other.md"),
        ];
        let found = existing.then(|| vault.join("000 Rough Writes/Book.md"));
        Picker::new(vault, "Book", found, notes)
    }

    fn labels(p: &Picker) -> Vec<String> {
        p.rows().into_iter().map(|r| r.label).collect()
    }

    #[test]
    fn a_new_note_named_after_the_pdf_comes_first() {
        let mut p = picker(false);
        assert_eq!(labels(&p)[..2], ["+ Book", "+ Name it yourself…"]);
        assert!(matches!(
            p.key(key(KeyCode::Enter), 10),
            PickerKey::Chosen(Choice::New(n)) if n == "Book"
        ));
    }

    #[test]
    fn a_note_that_has_the_name_already_is_offered_instead() {
        let mut p = picker(true);
        assert_eq!(labels(&p)[0], "  000 Rough Writes/Book");
        assert_eq!(labels(&p).len(), 4, "listed once");
        assert!(matches!(
            p.key(key(KeyCode::Enter), 10),
            PickerKey::Chosen(Choice::Open(path)) if path.ends_with("Book.md")
        ));
    }

    #[test]
    fn typing_filters_and_selects_the_first_match() {
        let mut p = picker(false);
        for c in "essay".chars() {
            p.key(key(KeyCode::Char(c)), 10);
        }
        assert_eq!(labels(&p).len(), 3);
        assert!(matches!(
            p.key(key(KeyCode::Enter), 10),
            PickerKey::Chosen(Choice::Open(path)) if path.ends_with("Essay.md")
        ));
    }

    #[test]
    fn naming_a_note_yourself() {
        let mut p = picker(false);
        p.key(key(KeyCode::Down), 10);
        p.key(key(KeyCode::Enter), 10);
        assert!(p.naming.is_some());
        for c in "My take.md".chars() {
            p.key(key(KeyCode::Char(c)), 10);
        }
        assert!(matches!(
            p.key(key(KeyCode::Enter), 10),
            PickerKey::Chosen(Choice::New(n)) if n == "My take"
        ));
    }

    #[test]
    fn escape_clears_the_filter_then_closes() {
        let mut p = picker(false);
        p.key(key(KeyCode::Char('x')), 10);
        assert!(matches!(p.key(key(KeyCode::Esc), 10), PickerKey::Handled));
        assert!(p.filter.is_empty());
        assert!(matches!(p.key(key(KeyCode::Esc), 10), PickerKey::Close));
    }

    #[test]
    fn the_selection_stays_in_view() {
        let mut p = picker(false);
        for _ in 0..4 {
            p.key(key(KeyCode::Down), 2);
        }
        assert_eq!(p.selected, 4);
        assert_eq!(p.scroll, 3);
    }
}
