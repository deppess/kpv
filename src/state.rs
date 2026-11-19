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
    /* Scroll to the first match when the results come. A search run again after the
     * file reloads only refreshes the highlights. */
    pub follow: bool,
}

impl SearchState {
    pub fn new(needle: String) -> Self {
        Self {
            needle,
            matches: Vec::new(),
            current: None,
            pending: true,
            follow: true,
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
    /* `:offset 12`, or `:offset` alone to show it */
    Offset(Option<i32>),
    Marks,
    /* `:delm ab` deletes marks a and b, `:delm!` all of them */
    DeleteMarks(Option<String>),
    Unknown(String),
}

pub fn parse_command(text: &str) -> Command {
    let text = text.trim();
    match text {
        "" => Command::Empty,
        "q" | "q!" | "quit" => Command::Quit,
        "noh" | "nohl" | "nohlsearch" => Command::ClearSearch,
        "offset" => Command::Offset(None),
        "marks" => Command::Marks,
        "delm!" | "delmarks!" => Command::DeleteMarks(None),
        _ if text.starts_with("delm ") || text.starts_with("delmarks ") => {
            let letters = text.split_once(' ').map(|(_, l)| l).unwrap_or("");
            Command::DeleteMarks(Some(letters.split_whitespace().collect()))
        }
        _ if text.starts_with("offset ") => {
            match text["offset ".len()..].trim().parse() {
                Ok(offset) => Command::Offset(Some(offset)),
                Err(_) => Command::Unknown(text.to_owned()),
            }
        }
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

/* One line of the key list: a group's name, or keys and what they do */
#[derive(Clone, Debug, PartialEq)]
pub enum HelpItem {
    Heading(&'static str),
    Keys(String, String),
}

/* A group of the key list: its name, its actions, then keys that work inside a panel
 * or prompt with what they do */
type Group = (
    &'static str,
    &'static [ConfigAction],
    &'static [(&'static str, &'static str)],
);

const GROUPS: [Group; 9] = [
    (
        "Moving",
        &[
            ConfigAction::MoveDown,
            ConfigAction::MoveUp,
            ConfigAction::MoveLeft,
            ConfigAction::MoveRight,
            ConfigAction::HalfPageDown,
            ConfigAction::HalfPageUp,
            ConfigAction::NextPage,
            ConfigAction::PrevPage,
            ConfigAction::JumpFirstPage,
            ConfigAction::JumpLastPage,
        ],
        &[("1-9", "A count first: 5j, 12G, 3 Ctrl+f")],
    ),
    (
        "Jumps and marks",
        &[
            ConfigAction::JumpBack,
            ConfigAction::JumpForward,
            ConfigAction::SetMark,
            ConfigAction::GoToMark,
        ],
        &[("`` or ''", "Back to where the last jump started")],
    ),
    (
        "Search",
        &[
            ConfigAction::Search,
            ConfigAction::NextMatch,
            ConfigAction::PrevMatch,
            ConfigAction::ClearSearch,
        ],
        &[],
    ),
    (
        "View",
        &[
            ConfigAction::ZoomIn,
            ConfigAction::ZoomOut,
            ConfigAction::FitWidth,
            ConfigAction::FitPage,
            ConfigAction::CenterViewer,
            ConfigAction::ToggleRuler,
            ConfigAction::ToggleInverse,
            ConfigAction::ToggleAlpha,
        ],
        &[],
    ),
    (
        "Contents",
        &[ConfigAction::ShowOutline],
        &[
            ("j k", "Move in the list"),
            ("l h", "Unfold, fold"),
            ("za zM zR", "Fold one, fold all, unfold all"),
            ("/", "Keep only entries with the text"),
            ("Enter", "Go there and close"),
            ("Space", "Go there, keep it open"),
            ("+ -", "Wider, narrower (kept for next time)"),
            ("Esc", "Clear the filter, then close"),
        ],
    ),
    (
        "Notes",
        &[
            ConfigAction::FocusNote,
            ConfigAction::FocusPdf,
            ConfigAction::HideNote,
            ConfigAction::SwitchNote,
            ConfigAction::NewNote,
        ],
        &[
            ("type", "In the picker: find a note"),
            ("Up Down", "In the picker: move"),
            ("Enter", "In the picker: open"),
        ],
    ),
    (
        "Commands",
        &[ConfigAction::CommandLine],
        &[
            (":45", "The page printed as 45"),
            (":#45", "The 45th page of the file"),
            (":offset 12", "Make the 13th page printed page 1"),
            (":marks", "List the marks"),
            (":delm a", "Delete mark a (:delm! all)"),
            (":noh", "Clear the search"),
            (":q", "Quit"),
        ],
    ),
    (
        "This list",
        &[ConfigAction::ShowHelp],
        &[
            ("j k  d u", "Scroll a line, half a screen"),
            ("g G", "Top, bottom"),
            ("q Esc", "Close"),
        ],
    ),
    ("Quitting", &[ConfigAction::Quit], &[]),
];

/* A key as the list shows it: `+` for Plus, Space spelled out */
fn key_name(key: &keybinds::KeyInput) -> String {
    let name = key.to_string();
    match name.as_str() {
        "Plus" => "+".to_owned(),
        _ => name,
    }
}

/* A key sequence as the list shows it: `gg` rather than `g g` when every key is one
 * plain character */
pub fn sequence_name(seq: &[keybinds::KeyInput]) -> String {
    let names: Vec<String> = seq.iter().map(key_name).collect();
    if names.len() > 1 && names.iter().all(|n| n.chars().count() == 1) {
        names.concat()
    } else {
        names.join(" ")
    }
}

/* The key list from the bindings in use: every bound action in its group, its keys
 * together, and the keys of the panels and prompts */
pub fn help_entries(binds: &Keybinds<ConfigAction>) -> Vec<HelpItem> {
    let mut items = Vec::new();
    for (heading, actions, extra) in GROUPS {
        items.push(HelpItem::Heading(heading));
        for action in actions {
            let keys: Vec<String> = binds
                .as_slice()
                .iter()
                .filter(|b| b.action == *action)
                .map(|b| sequence_name(b.seq.as_slice()))
                .collect();
            if !keys.is_empty() {
                items.push(HelpItem::Keys(
                    keys.join(", "),
                    action.describe().to_owned(),
                ));
            }
        }
        for (keys, what) in extra {
            items.push(HelpItem::Keys(keys.to_string(), what.to_string()));
        }
    }
    items
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
        assert_eq!(parse_command("offset 12"), Command::Offset(Some(12)));
        assert_eq!(parse_command("offset -2"), Command::Offset(Some(-2)));
        assert_eq!(parse_command("offset"), Command::Offset(None));
        assert_eq!(parse_command("marks"), Command::Marks);
        assert_eq!(parse_command("delm!"), Command::DeleteMarks(None));
        assert_eq!(
            parse_command("delm a b"),
            Command::DeleteMarks(Some("ab".to_owned()))
        );
        assert_eq!(
            parse_command("offset x"),
            Command::Unknown("offset x".to_owned())
        );
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
        binds.bind("Plus", ConfigAction::ZoomIn).unwrap();
        binds.bind("Ctrl+Space x", ConfigAction::HideNote).unwrap();
        let items = help_entries(&binds);
        let keys = |k: &str| {
            items
                .iter()
                .any(|i| matches!(i, HelpItem::Keys(x, _) if x == k))
        };
        assert_eq!(items[0], HelpItem::Heading("Moving"));
        assert_eq!(
            items[1],
            HelpItem::Keys(
                "j, Down".to_owned(),
                ConfigAction::MoveDown.describe().to_owned()
            )
        );
        assert!(keys("gg") && keys("?") && keys("+") && keys("Ctrl+Space x"));
        assert!(
            keys(":45") && keys("za zM zR"),
            "panel and command keys are listed"
        );
    }

    #[test]
    fn every_action_is_in_a_group_of_the_key_list() {
        for action in ConfigAction::ALL {
            assert!(
                GROUPS
                    .iter()
                    .any(|(_, actions, _)| actions.contains(&action)),
                "{:?} isn't in the key list",
                action
            );
        }
    }
}
