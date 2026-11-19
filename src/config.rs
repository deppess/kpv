use std::{collections::BTreeMap, path::PathBuf};

use toml::{Table, Value};

use keybinds::Keybinds;
use serde::Deserialize;

use crate::globals::{CONFIG_FILENAME, DEFAULT_CONFIG};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub viewer: ConfigViewer,
    pub notes: ConfigNotes,
    /* Key sequences that start a layer, and the layer's name */
    #[serde(default)]
    pub layers: BTreeMap<String, String>,
    pub bindings: Option<Keybinds<ConfigAction>>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigNotes {
    pub vault: String,
    pub folder: String,
    pub shell: String,
    pub editor: String,
    pub template: String,
    /* Percent of the window */
    pub width: u16,
}

#[derive(Debug, Deserialize)]
pub struct ConfigViewer {
    pub scroll_step: f32,
    /* Percent of the page width */
    pub zoom_step: f32,
    pub resolution: f32,
    pub page_gap: f32,
    pub invert_wheel: bool,
    pub link_hints: bool,
    pub remember_position: bool,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfigAction {
    ToggleAlpha,
    ToggleInverse,
    CenterViewer,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    ZoomIn,
    ZoomOut,
    JumpFirstPage,
    JumpLastPage,
    PrevPage,
    NextPage,
    Quit,
    ShowHelp,
    CommandLine,
    Search,
    NextMatch,
    PrevMatch,
    ClearSearch,
    ShowOutline,
    JumpBack,
    JumpForward,
    SetMark,
    GoToMark,
    ToggleRuler,
    HalfPageDown,
    HalfPageUp,
    FitWidth,
    FitPage,
    FocusNote,
    FocusPdf,
    HideNote,
    WidenNote,
    NarrowNote,
    TagPage,
    GoToTag,
}

impl ConfigAction {
    /* Every action, for the tests that check each one has a key and a place in the
     * key list */
    #[cfg(test)]
    pub const ALL: [ConfigAction; 37] = [
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
        ConfigAction::JumpBack,
        ConfigAction::JumpForward,
        ConfigAction::SetMark,
        ConfigAction::GoToMark,
        ConfigAction::CommandLine,
        ConfigAction::Search,
        ConfigAction::NextMatch,
        ConfigAction::PrevMatch,
        ConfigAction::ClearSearch,
        ConfigAction::ShowOutline,
        ConfigAction::ZoomIn,
        ConfigAction::ZoomOut,
        ConfigAction::FitWidth,
        ConfigAction::FitPage,
        ConfigAction::CenterViewer,
        ConfigAction::ToggleRuler,
        ConfigAction::ToggleInverse,
        ConfigAction::ToggleAlpha,
        ConfigAction::FocusNote,
        ConfigAction::FocusPdf,
        ConfigAction::HideNote,
        ConfigAction::WidenNote,
        ConfigAction::NarrowNote,
        ConfigAction::TagPage,
        ConfigAction::GoToTag,
        ConfigAction::ShowHelp,
        ConfigAction::Quit,
    ];

    /* What the notes panel does; with Ctrl or Alt they work from inside the editor too */
    pub fn is_pane(self) -> bool {
        matches!(
            self,
            ConfigAction::FocusNote
                | ConfigAction::FocusPdf
                | ConfigAction::HideNote
                | ConfigAction::WidenNote
                | ConfigAction::NarrowNote
                | ConfigAction::TagPage
                | ConfigAction::GoToTag
        )
    }

    pub fn describe(self) -> &'static str {
        match self {
            ConfigAction::MoveDown => "Scroll down (ruler on: next line)",
            ConfigAction::MoveUp => "Scroll up (ruler on: line before)",
            ConfigAction::MoveLeft => "Scroll left",
            ConfigAction::MoveRight => "Scroll right",
            ConfigAction::NextPage => "Next page",
            ConfigAction::PrevPage => "Previous page",
            ConfigAction::JumpFirstPage => "First page, or with a count that page",
            ConfigAction::JumpLastPage => "Last page, or with a count that page",
            ConfigAction::CommandLine => "Type a command",
            ConfigAction::Search => "Search",
            ConfigAction::NextMatch => "Next match",
            ConfigAction::PrevMatch => "Previous match",
            ConfigAction::ClearSearch => "Clear the search",
            ConfigAction::ShowOutline => "Open the contents",
            ConfigAction::JumpBack => "Back to where the last jump started",
            ConfigAction::JumpForward => "Forward again",
            ConfigAction::SetMark => "Set a mark, then a letter",
            ConfigAction::GoToMark => "Go to a mark, then its letter",
            ConfigAction::ZoomIn => "Zoom in",
            ConfigAction::ZoomOut => "Zoom out",
            ConfigAction::CenterViewer => "Center the page",
            ConfigAction::ToggleInverse => "Dark pages, in the theme's colors",
            ConfigAction::HalfPageDown => "Half a screen down",
            ConfigAction::HalfPageUp => "Half a screen up",
            ConfigAction::FitWidth => "Fit the width (100%)",
            ConfigAction::FitPage => "Fit the whole page",
            ConfigAction::ToggleRuler => "Reading ruler on or off",
            ConfigAction::ToggleAlpha => "Transparent page background",
            ConfigAction::FocusNote => "Go to the notes terminal, opening it",
            ConfigAction::FocusPdf => "Go to the PDF",
            ConfigAction::HideNote => "Hide the notes terminal (helix saves)",
            ConfigAction::WidenNote => "Note wider, pages narrower",
            ConfigAction::NarrowNote => "Note narrower, pages wider",
            ConfigAction::TagPage => "Tag the page you're reading in the note",
            ConfigAction::GoToTag => "Go to the page tagged here in the note",
            ConfigAction::ShowHelp => "This list",
            ConfigAction::Quit => "Quit viewmer",
        }
    }
}

/* The folder with config.toml and theme.toml: `~/.config/viewmer` on every system, or
 * under $XDG_CONFIG_HOME when that's set. Made if it isn't there. */
pub fn config_folder() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => std::env::home_dir()
            .ok_or("Could not find the home folder")?
            .join(".config"),
    };
    let folder = base.join(CONFIG_FILENAME);
    std::fs::create_dir_all(folder.as_path())
        .map_err(|x| format!("Could not create {}: {}", folder.display(), x))?;
    Ok(folder)
}

/* Lays `over` on `base`: tables merge key by key, anything else replaces */
fn merge(base: &mut Table, over: Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(base)), Value::Table(over)) => merge(base, over),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/* The settings: the user's config laid over the defaults, so that a config only needs
 * what it changes, and one from an older version gets new settings and keys as they
 * ship. A key bound to "" is unbound. */
pub fn parse(text: &str) -> Result<Config, String> {
    let mut merged: Table = DEFAULT_CONFIG.parse().map_err(|x| format!("{}", x))?;
    let user: Table = text.parse().map_err(|x| format!("{}", x))?;
    merge(&mut merged, user);
    if let Some(Value::Table(bindings)) = merged.get_mut("bindings") {
        bindings.retain(|_, action| action.as_str() != Some(""));
    }
    let config: Config = merged.try_into().map_err(|x| format!("{}", x))?;

    let viewer = &config.viewer;
    for (name, ok) in [
        ("scroll_step", viewer.scroll_step > 0.0),
        ("zoom_step", viewer.zoom_step > 0.0),
        ("resolution", viewer.resolution > 0.0),
        ("page_gap", viewer.page_gap >= 0.0),
    ] {
        if !ok {
            return Err(format!("`viewer.{}` must be more than 0", name));
        }
    }
    Ok(config)
}

/* Reads config.toml, writing the defaults there first when there is none */
pub fn config_load_or_create() -> Result<Config, String> {
    let path = config_folder()?.join("config.toml");
    let text = if path.exists() {
        std::fs::read_to_string(&path)
            .map_err(|x| format!("Could not read {}: {}", path.display(), x))?
    } else {
        std::fs::write(&path, DEFAULT_CONFIG)
            .map_err(|x| format!("Could not write {}: {}", path.display(), x))?;
        DEFAULT_CONFIG.to_owned()
    };
    parse(&text).map_err(|x| format!("{}: {}", path.display(), x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_binds_every_action() {
        let bindings = parse("").unwrap().bindings.unwrap();
        for action in ConfigAction::ALL {
            assert!(
                bindings.as_slice().iter().any(|b| b.action == action),
                "{:?} has no default key",
                action
            );
        }
    }

    #[test]
    fn a_config_holds_only_what_it_changes() {
        let config = parse(
            "[viewer]\nresolution = 2\n[bindings]\nx = \"Quit\"\n\"Ctrl+h\" = \"\"\n",
        )
        .unwrap();
        /* A whole number where a fraction is expected */
        assert_eq!(config.viewer.resolution, 2.0);
        /* The rest stays as it ships */
        assert_eq!(config.viewer.zoom_step, 10.0);
        let bindings = config.bindings.unwrap();
        let count = |a: ConfigAction| {
            bindings.as_slice().iter().filter(|b| b.action == a).count()
        };
        /* `x` is added to `q` and `Q` */
        assert_eq!(count(ConfigAction::Quit), 3);
        assert!(count(ConfigAction::NextPage) > 0);
        /* Ctrl-h, bound to "", is gone */
        assert_eq!(count(ConfigAction::FocusNote), 0);
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(parse("[viewer]\nresolution = 0\n").is_err());
        assert!(parse("[viewer]\nresolution = \"high\"\n").is_err());
        assert!(parse("[bindings]\nx = \"Fly\"\n").is_err());
        assert!(parse("not toml").is_err());
    }
}
