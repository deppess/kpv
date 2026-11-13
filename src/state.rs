use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keybinds::Keybinds;

use crate::{config::ConfigAction, threads::renderer::SearchMatch};

#[derive(Debug, PartialEq)]
pub enum Mode {
    Normal,
    /* Text typed after `:` */
    Command(String),
    /* Text typed after `/` */
    Search(String),
    Help { scroll: usize },
    /* The contents panel has the keys */
    Toc,
}

#[derive(Debug, PartialEq)]
pub struct SearchState {
    pub needle: String,
    pub matches: Vec<SearchMatch>,
    pub current: Option<usize>,
    /* Waiting for the renderer's results */
    pub pending: bool,
}

impl SearchState {
    pub fn new(needle: String) -> Self {
        Self {
            needle,
            matches: Vec::new(),
            current: None,
            pending: true,
        }
    }

    /* Takes the renderer's results and picks the first match on or after `page` */
    pub fn set_results(&mut self, matches: Vec<SearchMatch>, page: usize) {
        self.current = if matches.is_empty() {
            None
        } else {
            Some(matches.iter().position(|m| m.page >= page).unwrap_or(0))
        };
        self.matches = matches;
        self.pending = false;
    }

    /* Moves `steps` matches forward (or backward when negative), wrapping around */
    pub fn step(&mut self, steps: isize) -> Option<&SearchMatch> {
        let len = self.matches.len() as isize;
        if len == 0 {
            return None;
        }
        let current = self.current.unwrap_or(0) as isize;
        let next = (current + steps).rem_euclid(len) as usize;
        self.current = Some(next);
        self.matches.get(next)
    }

    pub fn current_match(&self) -> Option<&SearchMatch> {
        self.current.and_then(|i| self.matches.get(i))
    }

    /* `/needle 2/7` for the status bar */
    pub fn status(&self) -> String {
        if self.pending {
            format!("/{} …", self.needle)
        } else {
            match self.current {
                Some(i) => format!("/{} {}/{}", self.needle, i + 1, self.matches.len()),
                None => format!("/{} no match", self.needle),
            }
        }
    }
}

pub enum PromptKey {
    Edited,
    Cancel,
    Submit,
}

/* Applies a key to a `:` or `/` prompt */
pub fn edit_prompt(text: &mut String, key: KeyEvent) -> PromptKey {
    match key.code {
        KeyCode::Esc => PromptKey::Cancel,
        KeyCode::Enter => PromptKey::Submit,
        KeyCode::Backspace => {
            if text.pop().is_none() {
                PromptKey::Cancel
            } else {
                PromptKey::Edited
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            text.clear();
            PromptKey::Edited
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            text.push(c);
            PromptKey::Edited
        }
        _ => PromptKey::Edited,
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Empty,
    /* 1-indexed position in the file, from `:#12` */
    Page(usize),
    /* A printed page label such as `45` or `xiv` */
    Goto(String),
    Quit,
    ClearSearch,
    Unknown(String),
}

pub fn parse_command(text: &str) -> Command {
    let text = text.trim();
    match text {
        "" => Command::Empty,
        "q" | "q!" | "quit" => Command::Quit,
        "noh" | "nohl" | "nohlsearch" => Command::ClearSearch,
        _ => match text.strip_prefix('#') {
            Some(number) => match number.trim().parse::<usize>() {
                Ok(page) if page > 0 => Command::Page(page),
                _ => Command::Unknown(text.to_owned()),
            },
            None => Command::Goto(text.to_owned()),
        },
    }
}

/* Adds a typed digit to a pending count; `None` when the key isn't part of one */
pub fn count_digit(count: Option<usize>, key: &KeyEvent) -> Option<usize> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    if key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return None;
    }
    let digit = c.to_digit(10)? as usize;
    /* A leading 0 is not a count */
    if digit == 0 && count.is_none() {
        return None;
    }
    Some((count.unwrap_or(0) * 10 + digit).min(99_999))
}

/* `(keys, description)` rows for the help overlay, keys grouped per action */
pub fn help_entries(binds: &Keybinds<ConfigAction>) -> Vec<(String, String)> {
    let mut entries: Vec<(String, String)> = ConfigAction::ALL
        .iter()
        .filter_map(|action| {
            let keys: Vec<String> = binds
                .as_slice()
                .iter()
                .filter(|b| b.action == *action)
                .map(|b| b.seq.to_string())
                .collect();
            (!keys.is_empty()).then(|| (keys.join(", "), action.describe().to_owned()))
        })
        .collect();
    entries.push((
        "1-9".to_owned(),
        "Count before a key: 12G page #12, 3 Ctrl+f three pages".to_owned(),
    ));
    entries.push((
        "Esc".to_owned(),
        "In a prompt or this list: cancel or close".to_owned(),
    ));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn hit(page: usize) -> SearchMatch {
        SearchMatch {
            page,
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn commands_parse() {
        assert_eq!(parse_command(" 42 "), Command::Goto("42".to_owned()));
        assert_eq!(parse_command("xiv"), Command::Goto("xiv".to_owned()));
        assert_eq!(parse_command("#42"), Command::Page(42));
        assert_eq!(parse_command("# 7"), Command::Page(7));
        assert_eq!(parse_command("#0"), Command::Unknown("#0".to_owned()));
        assert_eq!(parse_command("#x"), Command::Unknown("#x".to_owned()));
        assert_eq!(parse_command("q"), Command::Quit);
        assert_eq!(parse_command("noh"), Command::ClearSearch);
        assert_eq!(parse_command(""), Command::Empty);
    }

    #[test]
    fn counts_build_from_digits() {
        let c = count_digit(None, &key(KeyCode::Char('1')));
        assert_eq!(c, Some(1));
        assert_eq!(count_digit(c, &key(KeyCode::Char('2'))), Some(12));
        assert_eq!(count_digit(None, &key(KeyCode::Char('0'))), None);
        assert_eq!(count_digit(Some(1), &key(KeyCode::Char('0'))), Some(10));
        assert_eq!(count_digit(None, &key(KeyCode::Char('j'))), None);
        let ctrl = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::CONTROL);
        assert_eq!(count_digit(None, &ctrl), None);
    }

    #[test]
    fn prompt_editing() {
        let mut text = String::new();
        assert!(matches!(
            edit_prompt(&mut text, key(KeyCode::Char('4'))),
            PromptKey::Edited
        ));
        assert!(matches!(
            edit_prompt(&mut text, key(KeyCode::Char('2'))),
            PromptKey::Edited
        ));
        assert_eq!(text, "42");
        assert!(matches!(
            edit_prompt(&mut text, key(KeyCode::Backspace)),
            PromptKey::Edited
        ));
        assert_eq!(text, "4");
        assert!(matches!(
            edit_prompt(&mut text, key(KeyCode::Enter)),
            PromptKey::Submit
        ));
        let mut empty = String::new();
        assert!(matches!(
            edit_prompt(&mut empty, key(KeyCode::Backspace)),
            PromptKey::Cancel
        ));
        assert!(matches!(
            edit_prompt(&mut text, key(KeyCode::Esc)),
            PromptKey::Cancel
        ));
    }

    #[test]
    fn search_starts_at_the_current_page_and_wraps() {
        let mut s = SearchState::new("term".to_owned());
        assert_eq!(s.status(), "/term …");
        s.set_results(vec![hit(0), hit(3), hit(3), hit(9)], 2);
        assert_eq!(s.current, Some(1));
        assert_eq!(s.status(), "/term 2/4");
        assert_eq!(s.step(1).unwrap().page, 3);
        assert_eq!(s.step(1).unwrap().page, 9);
        assert_eq!(s.step(1).unwrap().page, 0);
        assert_eq!(s.step(-1).unwrap().page, 9);
        assert_eq!(s.step(-5).unwrap().page, 3);
    }

    #[test]
    fn search_past_the_last_match_starts_over() {
        let mut s = SearchState::new("x".to_owned());
        s.set_results(vec![hit(1), hit(2)], 7);
        assert_eq!(s.current, Some(0));
    }

    #[test]
    fn search_without_matches() {
        let mut s = SearchState::new("zzz".to_owned());
        s.set_results(vec![], 0);
        assert_eq!(s.status(), "/zzz no match");
        assert!(s.step(1).is_none());
    }

    #[test]
    fn help_groups_keys_per_action() {
        let mut binds = Keybinds::default();
        binds.bind("j", ConfigAction::MoveDown).unwrap();
        binds.bind("Down", ConfigAction::MoveDown).unwrap();
        binds.bind("?", ConfigAction::ShowHelp).unwrap();
        binds.bind("g g", ConfigAction::JumpFirstPage).unwrap();
        let entries = help_entries(&binds);
        assert_eq!(entries[0], ("j, Down".to_owned(), "Move down".to_owned()));
        assert!(entries.iter().any(|(k, _)| k == "g g"));
        assert!(entries.iter().any(|(k, _)| k == "?"));
        assert_eq!(entries.last().unwrap().0, "Esc");
    }
}
