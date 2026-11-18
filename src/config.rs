use std::{
    collections::{BTreeMap, HashMap, HashSet},
    mem::discriminant,
    path::{Path, PathBuf},
};

use toml::{Table, Value};

use dirs::{config_dir, home_dir};
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
    pub sharpness: f64,
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
    SwitchNote,
    NewNote,
}

impl ConfigAction {
    /* Every action, for the tests that check each one has a key and a place in the
     * key list */
    #[cfg(test)]
    pub const ALL: [ConfigAction; 35] = [
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
        ConfigAction::SwitchNote,
        ConfigAction::NewNote,
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
                | ConfigAction::SwitchNote
                | ConfigAction::NewNote
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
            ConfigAction::ToggleInverse => "Invert colors",
            ConfigAction::HalfPageDown => "Half a screen down",
            ConfigAction::HalfPageUp => "Half a screen up",
            ConfigAction::FitWidth => "Fit the width (100%)",
            ConfigAction::FitPage => "Fit the whole page",
            ConfigAction::ToggleRuler => "Reading ruler on or off",
            ConfigAction::ToggleAlpha => "Transparent page background",
            ConfigAction::FocusNote => "Go to the note, opening it",
            ConfigAction::FocusPdf => "Go to the PDF",
            ConfigAction::HideNote => "Hide the note (it's saved)",
            ConfigAction::SwitchNote => "Switch to another note",
            ConfigAction::NewNote => "New note with a name you type",
            ConfigAction::ShowHelp => "This list",
            ConfigAction::Quit => "Quit viewmer",
        }
    }
}

fn fix_config_keybindings(
    current: &mut toml::Table,
    default: &toml::Table,
) -> Result<bool, String> {
    let mut current_reversed: HashMap<String, Vec<String>> = HashMap::new();
    let mut default_reversed: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in current.into_iter() {
        current_reversed
            .entry(
                b.as_str()
                    .ok_or_else(|| "Keybinding isn't a string".to_string())?
                    .to_owned(),
            )
            .or_default()
            .push(a.as_str().to_owned());
    }

    for (a, b) in default {
        default_reversed
            .entry(
                b.as_str()
                    .ok_or_else(|| "Keybinding isn't a string".to_string())?
                    .to_owned(),
            )
            .or_default()
            .push(a.as_str().to_owned());
    }

    let mut config_has_changed = false;

    let keys: HashSet<String> = current_reversed
        .keys()
        .chain(default_reversed.keys())
        .cloned()
        .collect();
    for key in keys.into_iter() {
        let key_in_current = current_reversed.contains_key(&key);
        let key_in_default = default_reversed.contains_key(&key);

        /* old key */
        if key_in_current && !key_in_default {
            for key_current in &current_reversed[&key] {
                current.remove(key_current);
            }
            config_has_changed |= true;
        /* new key */
        } else if !key_in_current && key_in_default {
            for key_default in &default_reversed[&key] {
                /* Don't add default keybindings if the user has mapped them for other action*/
                if current.contains_key(key_default) {
                    continue;
                }
                current.insert(key_default.clone(), toml::Value::from(key.clone()));
                config_has_changed |= true;
            }
        }
    }

    Ok(config_has_changed)
}

/* Default bindings that later versions moved: (key, action). A config that still has
 * one exactly as shipped gets it dropped, so the key can go to its new action and the
 * action gets its new key. A binding the user changed is left alone. */
const RETIRED_BINDINGS: [(&str, &str); 1] = [("Ctrl+o", "ToggleInverse")];

fn retire_bindings(current: &mut toml::Table, default: &toml::Table) -> bool {
    let mut changed = false;
    for (key, action) in RETIRED_BINDINGS {
        let moved = default.get(key).and_then(Value::as_str) != Some(action);
        if moved && current.get(key).and_then(Value::as_str) == Some(action) {
            current.remove(key);
            changed = true;
        }
    }
    changed
}

/* Remove old config variables, add new defaults if not existant; return true if config
 * `current` has been modified */
fn fix_config_toml(
    current: &mut toml::Table,
    default: &toml::Table,
) -> Result<bool, String> {
    let mut config_has_changed = false;

    let keys: HashSet<String> = current.keys().chain(default.keys()).cloned().collect();
    for key in keys {
        let key_in_current = current.contains_key(&key);
        let key_in_default = default.contains_key(&key);

        if key_in_current && !key_in_default {
            /* Remove old config variable that has been removed in new version */
            current.remove(&key);
            config_has_changed = true;
        } else if !key_in_current && key_in_default {
            /* Add new config variable that has been added in new version */
            current.insert(key.clone(), default[&key].clone());
            config_has_changed = true;
        } else if discriminant(&current[&key]) != discriminant(&default[&key])
            && !(is_number(&current[&key]) && is_number(&default[&key]))
        {
            /* If different variants, infer the variant from the default */
            current[&key] = default[&key].clone();
            config_has_changed = true;
        } else if let (Value::Table(current_rec), Value::Table(default_rec)) =
            (&mut current[&key], &default[&key])
        {
            /* If both were tables, check recursively */
            if key == "bindings" {
                /* Special case */
                config_has_changed |= retire_bindings(current_rec, default_rec);
                config_has_changed |= fix_config_keybindings(current_rec, default_rec)?;
            } else if key == "layers" {
                /* The user's own layers stay; only missing defaults are added */
                for (name, value) in default_rec {
                    if !current_rec.contains_key(name) {
                        current_rec.insert(name.clone(), value.clone());
                        config_has_changed = true;
                    }
                }
            } else {
                config_has_changed |= fix_config_toml(current_rec, default_rec)?;
            }
        }
    }

    Ok(config_has_changed)
}

/* The folder with config.toml and theme.toml: `~/.config/viewmer` on every system, or
 * under $XDG_CONFIG_HOME when that's set. Made if it isn't there. */
pub fn config_folder() -> Result<PathBuf, String> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => home_dir()
            .ok_or("Could not find the home folder")?
            .join(".config"),
    };
    let folder = base.join(CONFIG_FILENAME);
    std::fs::create_dir_all(folder.as_path())
        .map_err(|x| format!("Could not create {}: {}", folder.display(), x))?;
    if let Some(old) = config_dir().map(|d| d.join(CONFIG_FILENAME)) {
        move_old_files(&old, &folder);
    }
    Ok(folder)
}

/* Earlier versions kept the config and theme in the system's own config folder, which
 * on macOS is ~/Library/Application Support. They move over once; a file already in
 * the new place wins. */
fn move_old_files(old: &Path, folder: &Path) {
    if old == folder {
        return;
    }
    for name in ["config.toml", "theme.toml"] {
        let (from, to) = (old.join(name), folder.join(name));
        if from.is_file() && !to.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
}

/* Settings that later versions renamed or moved: the old place and the new key in
 * [viewer]. Their values carry over. */
const RENAMED: [(&[&str], &str); 6] = [
    (&["viewer", "scroll_speed"], "scroll_step"),
    (&["viewer", "render_precision"], "sharpness"),
    (&["viewer", "margin_bottom"], "page_gap"),
    (&["viewer", "inverse_scroll"], "invert_wheel"),
    (&["viewer", "uri_hint", "enabled"], "link_hints"),
    (&["state", "enabled"], "remember_position"),
];

/* Brings a config written by an older version up to date: renamed settings move to
 * their new names with their values, old ones go, new ones come in with the comments
 * they ship with. Everything else in the file, comments and order included, stays as
 * it is. `None` when nothing needed changing. */
pub fn upgrade(text: &str) -> Result<Option<String>, String> {
    let mut doc = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|x| format!("Could not parse the config as TOML: {}", x))?;
    let default_doc = DEFAULT_CONFIG
        .parse::<toml_edit::DocumentMut>()
        .map_err(|x| format!("Could not parse the default config: {}", x))?;

    let mut changed = false;
    for (path, new) in RENAMED {
        let Some(value) = take(doc.as_table_mut(), path) else {
            continue;
        };
        changed = true;
        if !doc.contains_key("viewer") {
            doc.insert("viewer", toml_edit::table());
        }
        let Some(viewer) = doc["viewer"].as_table_mut() else {
            continue;
        };
        if !viewer.contains_key(new) {
            let mut value = value;
            value.decor_mut().clear();
            match default_doc["viewer"].as_table().and_then(|t| t.key(new)) {
                Some(key) => {
                    viewer.insert_formatted(key, toml_edit::Item::Value(value));
                }
                None => {
                    viewer.insert(new, toml_edit::Item::Value(value));
                }
            }
        }
    }

    let mut current = doc
        .to_string()
        .parse::<Table>()
        .map_err(|x| format!("Could not parse the config as TOML: {}", x))?;
    let default = DEFAULT_CONFIG
        .parse::<Table>()
        .map_err(|x| format!("Could not parse the default config: {}", x))?;
    changed |= fix_config_toml(&mut current, &default)?;
    if !changed {
        return Ok(None);
    }
    apply(doc.as_table_mut(), &current, default_doc.as_table());
    Ok(Some(doc.to_string()))
}

/* Removes the value at `path` and returns it; a table left empty goes too */
fn take(table: &mut toml_edit::Table, path: &[&str]) -> Option<toml_edit::Value> {
    match path {
        [] => None,
        [key] => table.remove(key)?.into_value().ok(),
        [first, rest @ ..] => {
            let sub = table.get_mut(first)?.as_table_mut()?;
            let value = take(sub, rest);
            if sub.is_empty() {
                table.remove(first);
            }
            value
        }
    }
}

/* Makes `doc` hold what `fixed` holds while keeping its comments and order. Keys it
 * lacks come from `default`, with their comments. */
fn apply(doc: &mut toml_edit::Table, fixed: &Table, default: &toml_edit::Table) {
    let gone: Vec<String> = doc
        .iter()
        .map(|(k, _)| k.to_owned())
        .filter(|k| !fixed.contains_key(k))
        .collect();
    for key in gone {
        doc.remove(&key);
    }

    let empty = toml_edit::Table::new();
    for (key, value) in fixed {
        if !doc.contains_key(key) {
            match default.get_key_value(key) {
                Some((k, item)) => {
                    doc.insert_formatted(k, item.clone());
                }
                None => {
                    doc.insert(key, toml_edit::Item::None);
                }
            }
        }
        let default_sub = default
            .get(key)
            .and_then(|i| i.as_table())
            .unwrap_or(&empty);
        let item = &mut doc[key.as_str()];
        match (item.as_table_mut(), value) {
            (Some(sub), Value::Table(fixed_sub)) => apply(sub, fixed_sub, default_sub),
            _ => {
                let current = item.as_value().map(|v| {
                    let mut v = v.clone();
                    v.decor_mut().clear();
                    v.to_string()
                });
                if current.as_deref() != Some(value.to_string().as_str()) {
                    if let Ok(new) = value.to_string().parse::<toml_edit::Value>() {
                        let decor = item.as_value().map(|v| v.decor().clone());
                        let mut new = new;
                        if let Some(decor) = decor {
                            *new.decor_mut() = decor;
                        }
                        *item = toml_edit::Item::Value(new);
                    }
                }
            }
        }
    }
}

fn is_number(value: &Value) -> bool {
    matches!(value, Value::Integer(_) | Value::Float(_))
}

pub fn config_load_or_create() -> Result<Config, String> {
    let config = config_folder()?.join("config.toml");

    let mut config_content = String::new();
    if !config.as_path().exists() {
        std::fs::write(config.as_path(), DEFAULT_CONFIG.as_bytes())
            .map_err(|x| format!("Could not create and write config file: {}", x))?;
        config_content.push_str(DEFAULT_CONFIG);
    } else {
        config_content = std::fs::read_to_string(config.as_path())
            .map_err(|x| format!("Could not open and read config file: {}", x))?;

        if let Some(fixed) = upgrade(&config_content)? {
            std::fs::write(config.as_path(), fixed.as_bytes())
                .map_err(|x| format!("Could not write the updated config: {}", x))?;
            config_content = fixed;
        }
    }

    let config_parsed: Config = toml::from_str(config_content.as_str())
        .map_err(|x| format!("Could not parse config file: {}", x))?;

    /* ========================== Check constant constraints ========================= */
    let viewer = &config_parsed.viewer;
    for (name, ok) in [
        ("scroll_step", viewer.scroll_step > 0.0),
        ("zoom_step", viewer.zoom_step > 0.0),
        ("sharpness", viewer.sharpness > 0.0),
        ("page_gap", viewer.page_gap >= 0.0),
    ] {
        if !ok {
            return Err(format!("`viewer.{}` must be more than 0", name));
        }
    }

    if config_parsed.bindings.is_none() {
        return Err("`config.bindings` can not be empty!".to_string());
    }

    Ok(config_parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fix_config_no_change() {
        const REFERENCE_CONFIG: &str = r#"
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 20.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 314572800
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "white"
		# Hint bar width percentage based on terminal width
		width = 0.2 
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        const TEST_CONFIG: &str = r#"
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 20.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 314572800
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "white"
		# Hint bar width percentage based on terminal width
		width = 0.2 
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        let mut test_config_toml = TEST_CONFIG.parse::<Table>().unwrap();
        let reference_config_toml = REFERENCE_CONFIG.parse::<Table>().unwrap();

        assert!(test_config_toml == reference_config_toml);
        assert!(
            fix_config_toml(&mut test_config_toml, &reference_config_toml) == Ok(false)
        );
        assert!(test_config_toml == reference_config_toml);
    }

    #[test]
    fn test_fix_config_removal_of_non_existent_variables() {
        const REFERENCE_CONFIG: &str = r#"
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 20.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 314572800
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "white"
		# Hint bar width percentage based on terminal width
		width = 0.2 
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        const TEST_CONFIG: &str = r#"
		non_existent_1 = "hello!"
        
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 22.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 31457
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false

		non_existent_2 = 0.0
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "red"
		# Hint bar width percentage based on terminal width
		width = 0.2 

		non_existent_3 = 0.0
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		non_existent_4 = "true"
		"#;

        let mut test_config_toml = TEST_CONFIG.parse::<Table>().unwrap();
        let reference_config_toml = REFERENCE_CONFIG.parse::<Table>().unwrap();

        assert!(
            fix_config_toml(&mut test_config_toml, &reference_config_toml) == Ok(true)
        );

        /* [viewer] */
        assert!(test_config_toml.get("non_existent_1").is_none());
        assert!(test_config_toml["viewer"]["scroll_speed"] == Value::Float(22.0));
        assert!(test_config_toml["viewer"]["render_precision"] == Value::Float(1.5));
        assert!(test_config_toml["viewer"]["memory_limit"] == Value::Integer(31457));
        assert!(test_config_toml["viewer"]["scale_min"] == Value::Float(0.2));
        assert!(test_config_toml["viewer"]["scale_default"] == Value::Float(0.5));
        assert!(test_config_toml["viewer"]["scale_amount"] == Value::Float(0.5));
        assert!(test_config_toml["viewer"]["margin_bottom"] == Value::Float(10.0));
        assert!(test_config_toml["viewer"]["pages_preloaded"] == Value::Integer(3));
        assert!(test_config_toml["viewer"]["inverse_scroll"] == Value::Boolean(false));
        assert!(test_config_toml["viewer"].get("non_existent_2").is_none());

        assert!(
            test_config_toml["viewer"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["viewer"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );

        /* [viewer.uri_hint] */
        assert!(
            test_config_toml["viewer"]["uri_hint"]["enabled"] == Value::Boolean(true)
        );
        assert!(
            test_config_toml["viewer"]["uri_hint"]["background"]
                == Value::String("blue".to_owned())
        );
        assert!(
            test_config_toml["viewer"]["uri_hint"]["foreground"]
                == Value::String("red".to_owned())
        );
        assert!(test_config_toml["viewer"]["uri_hint"]["width"] == Value::Float(0.2));
        assert!(test_config_toml["viewer"]["uri_hint"]
            .get("non_existent_3")
            .is_none());

        assert!(
            test_config_toml["viewer"]["uri_hint"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["viewer"]["uri_hint"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );

        /* [bindings] */
        assert!(
            test_config_toml["bindings"]["Ctrl+a"]
                == Value::String("ToggleAlpha".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+o"]
                == Value::String("ToggleInverse".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["C"] == Value::String("CenterViewer".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["h"] == Value::String("MoveLeft".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["j"] == Value::String("MoveDown".to_owned())
        );
        assert!(test_config_toml["bindings"]["k"] == Value::String("MoveUp".to_owned()));
        assert!(
            test_config_toml["bindings"]["l"] == Value::String("MoveRight".to_owned())
        );
        assert!(test_config_toml["bindings"]["Up"] == Value::String("MoveUp".to_owned()));
        assert!(
            test_config_toml["bindings"]["Left"] == Value::String("MoveLeft".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Right"]
                == Value::String("MoveRight".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Down"] == Value::String("MoveDown".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Plus"] == Value::String("ZoomIn".to_owned())
        );
        assert!(test_config_toml["bindings"]["-"] == Value::String("ZoomOut".to_owned()));
        assert!(
            test_config_toml["bindings"]["g g"]
                == Value::String("JumpFirstPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["G"] == Value::String("JumpLastPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["PageUp"]
                == Value::String("PrevPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["PageDown"]
                == Value::String("NextPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+b"]
                == Value::String("PrevPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+f"]
                == Value::String("NextPage".to_owned())
        );
        assert!(test_config_toml["bindings"]["q"] == Value::String("Quit".to_owned()));
        assert!(test_config_toml["bindings"]["Q"] == Value::String("Quit".to_owned()));
        assert!(test_config_toml["bindings"].get("non_existent_4").is_none());

        assert!(
            test_config_toml["bindings"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["bindings"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_fix_config_addition_of_non_existent_variables() {
        const REFERENCE_CONFIG: &str = r#"
		show = true
        
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 20.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 314572800
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false

		inverse_inverse_scroll = true
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "white"
		# Hint bar width percentage based on terminal width
		width = 0.2 

		height = 1.0
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"bbbb" = "Quit2"
		"#;

        const TEST_CONFIG: &str = r#"
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 22.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 31457
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "red"
		# Hint bar width percentage based on terminal width
		width = 0.2 

		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        let mut test_config_toml = TEST_CONFIG.parse::<Table>().unwrap();
        let reference_config_toml = REFERENCE_CONFIG.parse::<Table>().unwrap();

        assert!(
            fix_config_toml(&mut test_config_toml, &reference_config_toml) == Ok(true)
        );

        assert!(test_config_toml["show"] == Value::Boolean(true));

        /* [viewer] */
        assert!(test_config_toml["viewer"]["scroll_speed"] == Value::Float(22.0));
        assert!(test_config_toml["viewer"]["render_precision"] == Value::Float(1.5));
        assert!(test_config_toml["viewer"]["memory_limit"] == Value::Integer(31457));
        assert!(test_config_toml["viewer"]["scale_min"] == Value::Float(0.2));
        assert!(test_config_toml["viewer"]["scale_default"] == Value::Float(0.5));
        assert!(test_config_toml["viewer"]["scale_amount"] == Value::Float(0.5));
        assert!(test_config_toml["viewer"]["margin_bottom"] == Value::Float(10.0));
        assert!(test_config_toml["viewer"]["pages_preloaded"] == Value::Integer(3));
        assert!(test_config_toml["viewer"]["inverse_scroll"] == Value::Boolean(false));
        assert!(
            test_config_toml["viewer"]["inverse_inverse_scroll"] == Value::Boolean(true)
        );

        assert!(
            test_config_toml["viewer"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["viewer"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );

        /* [viewer.uri_hint] */
        assert!(
            test_config_toml["viewer"]["uri_hint"]["enabled"] == Value::Boolean(true)
        );
        assert!(
            test_config_toml["viewer"]["uri_hint"]["background"]
                == Value::String("blue".to_owned())
        );
        assert!(
            test_config_toml["viewer"]["uri_hint"]["foreground"]
                == Value::String("red".to_owned())
        );
        assert!(test_config_toml["viewer"]["uri_hint"]["width"] == Value::Float(0.2));
        assert!(test_config_toml["viewer"]["uri_hint"]["height"] == Value::Float(1.0));

        assert!(
            test_config_toml["viewer"]["uri_hint"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["viewer"]["uri_hint"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );

        /* [bindings] */
        assert!(
            test_config_toml["bindings"]["Ctrl+a"]
                == Value::String("ToggleAlpha".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+o"]
                == Value::String("ToggleInverse".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["C"] == Value::String("CenterViewer".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["h"] == Value::String("MoveLeft".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["j"] == Value::String("MoveDown".to_owned())
        );
        assert!(test_config_toml["bindings"]["k"] == Value::String("MoveUp".to_owned()));
        assert!(
            test_config_toml["bindings"]["l"] == Value::String("MoveRight".to_owned())
        );
        assert!(test_config_toml["bindings"]["Up"] == Value::String("MoveUp".to_owned()));
        assert!(
            test_config_toml["bindings"]["Left"] == Value::String("MoveLeft".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Right"]
                == Value::String("MoveRight".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Down"] == Value::String("MoveDown".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Plus"] == Value::String("ZoomIn".to_owned())
        );
        assert!(test_config_toml["bindings"]["-"] == Value::String("ZoomOut".to_owned()));
        assert!(
            test_config_toml["bindings"]["g g"]
                == Value::String("JumpFirstPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["G"] == Value::String("JumpLastPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["PageUp"]
                == Value::String("PrevPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["PageDown"]
                == Value::String("NextPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+b"]
                == Value::String("PrevPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+f"]
                == Value::String("NextPage".to_owned())
        );
        assert!(test_config_toml["bindings"]["q"] == Value::String("Quit".to_owned()));
        assert!(test_config_toml["bindings"]["Q"] == Value::String("Quit".to_owned()));

        assert!(
            test_config_toml["bindings"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["bindings"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_fix_config_replacement_of_wrong_variants() {
        const REFERENCE_CONFIG: &str = r#"
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 20.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 314572800
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = "no"
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = 10.0
		# Hint bar width percentage based on terminal width
		width = 0.2 
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        const TEST_CONFIG: &str = r#"
		[viewer]
		# Determines how fast the document is scrolled
		scroll_speed = 22.0
		# Determines at what precision the pages are rendered
		render_precision = 1.5
		# Determines the image data limit that the software holds in RAM (bytes)
		memory_limit = 31457
		# Minimum scale amount allowed
		scale_min = 0.2
		# Determines the default scale of the viewer when starting the viewer
		scale_default = 0.5
		# Determines the scaling amount when zooming in or out
		scale_amount = 0.5
		# Determines the margin on the bottom of each page
		margin_bottom = 10.0
		# Determines the amount of pages that are preloaded in advance 
		pages_preloaded = 3
		# Inverse vertical scroll
		inverse_scroll = false
		
		[viewer.uri_hint]
		# Enabled URI hints
		enabled = true
		# Background color of hint bar
		background = "blue"
		# Foreground color of hint bar text
		foreground = "white"
		# Hint bar width percentage based on terminal width
		width = 0.2 
		
		[bindings]
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        let mut test_config_toml = TEST_CONFIG.parse::<Table>().unwrap();
        let reference_config_toml = REFERENCE_CONFIG.parse::<Table>().unwrap();

        assert!(
            fix_config_toml(&mut test_config_toml, &reference_config_toml) == Ok(true)
        );

        /* [viewer] */
        assert!(test_config_toml["viewer"]["scroll_speed"] == Value::Float(22.0));
        assert!(test_config_toml["viewer"]["render_precision"] == Value::Float(1.5));
        assert!(test_config_toml["viewer"]["memory_limit"] == Value::Integer(31457));
        assert!(test_config_toml["viewer"]["scale_min"] == Value::Float(0.2));
        assert!(test_config_toml["viewer"]["scale_default"] == Value::Float(0.5));
        assert!(test_config_toml["viewer"]["scale_amount"] == Value::Float(0.5));
        assert!(test_config_toml["viewer"]["margin_bottom"] == Value::Float(10.0));
        assert!(test_config_toml["viewer"]["pages_preloaded"] == Value::Integer(3));
        assert!(
            test_config_toml["viewer"]["inverse_scroll"]
                == Value::String("no".to_owned())
        );

        assert!(
            test_config_toml["viewer"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["viewer"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );

        /* [viewer.uri_hint] */
        assert!(
            test_config_toml["viewer"]["uri_hint"]["enabled"] == Value::Boolean(true)
        );
        assert!(
            test_config_toml["viewer"]["uri_hint"]["background"]
                == Value::String("blue".to_owned())
        );
        assert!(
            test_config_toml["viewer"]["uri_hint"]["foreground"] == Value::Float(10.0)
        );
        assert!(test_config_toml["viewer"]["uri_hint"]["width"] == Value::Float(0.2));

        assert!(
            test_config_toml["viewer"]["uri_hint"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["viewer"]["uri_hint"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );

        /* [bindings] */
        assert!(
            test_config_toml["bindings"]["Ctrl+a"]
                == Value::String("ToggleAlpha".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+o"]
                == Value::String("ToggleInverse".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["C"] == Value::String("CenterViewer".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["h"] == Value::String("MoveLeft".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["j"] == Value::String("MoveDown".to_owned())
        );
        assert!(test_config_toml["bindings"]["k"] == Value::String("MoveUp".to_owned()));
        assert!(
            test_config_toml["bindings"]["l"] == Value::String("MoveRight".to_owned())
        );
        assert!(test_config_toml["bindings"]["Up"] == Value::String("MoveUp".to_owned()));
        assert!(
            test_config_toml["bindings"]["Left"] == Value::String("MoveLeft".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Right"]
                == Value::String("MoveRight".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Down"] == Value::String("MoveDown".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Plus"] == Value::String("ZoomIn".to_owned())
        );
        assert!(test_config_toml["bindings"]["-"] == Value::String("ZoomOut".to_owned()));
        assert!(
            test_config_toml["bindings"]["g g"]
                == Value::String("JumpFirstPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["G"] == Value::String("JumpLastPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["PageUp"]
                == Value::String("PrevPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["PageDown"]
                == Value::String("NextPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+b"]
                == Value::String("PrevPage".to_owned())
        );
        assert!(
            test_config_toml["bindings"]["Ctrl+f"]
                == Value::String("NextPage".to_owned())
        );
        assert!(test_config_toml["bindings"]["q"] == Value::String("Quit".to_owned()));

        assert!(
            test_config_toml["bindings"]
                .as_table()
                .unwrap()
                .keys()
                .collect::<Vec<_>>()
                == reference_config_toml["bindings"]
                    .as_table()
                    .unwrap()
                    .keys()
                    .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_fix_bindings_removal_of_non_existent() {
        const REFERENCE_BINDINGS_SINGLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        const TEST_BINDINGS_SINGLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"OO" = "NonExistentAction"
		"#;

        let mut test_bindings_toml = TEST_BINDINGS_SINGLE.parse::<Table>().unwrap();
        let reference_bindings_toml = REFERENCE_BINDINGS_SINGLE.parse::<Table>().unwrap();

        assert!(
            fix_config_keybindings(&mut test_bindings_toml, &reference_bindings_toml)
                == Ok(true)
        );

        assert!(test_bindings_toml == reference_bindings_toml);

        const REFERENCE_BINDINGS_MULTIPLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        const TEST_BINDINGS_MULTIPLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"OO" = "NonExistentAction"
		"AOOOO" = "NonExistentAction2"
		"Ctrl+L" = "NonExistentAction3"
		"#;

        let mut test_bindings_toml = TEST_BINDINGS_MULTIPLE.parse::<Table>().unwrap();
        let reference_bindings_toml =
            REFERENCE_BINDINGS_MULTIPLE.parse::<Table>().unwrap();

        assert!(
            fix_config_keybindings(&mut test_bindings_toml, &reference_bindings_toml)
                == Ok(true)
        );

        assert!(test_bindings_toml == reference_bindings_toml);
    }

    #[test]
    fn test_fix_bindings_addition_of_new_and_previously_non_existent() {
        const REFERENCE_BINDINGS_SINGLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"U" = "NewAction"
		"#;

        const TEST_BINDINGS_SINGLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        let mut test_bindings_toml = TEST_BINDINGS_SINGLE.parse::<Table>().unwrap();
        let reference_bindings_toml = REFERENCE_BINDINGS_SINGLE.parse::<Table>().unwrap();

        assert!(
            fix_config_keybindings(&mut test_bindings_toml, &reference_bindings_toml)
                == Ok(true)
        );

        assert!(test_bindings_toml == reference_bindings_toml);

        const REFERENCE_BINDINGS_MULTIPLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"U" = "NewAction"
		"UU" = "NewAction2"
		"#;

        const TEST_BINDINGS_MULTIPLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"
		"#;

        let mut test_bindings_toml = TEST_BINDINGS_MULTIPLE.parse::<Table>().unwrap();
        let reference_bindings_toml =
            REFERENCE_BINDINGS_MULTIPLE.parse::<Table>().unwrap();

        assert!(
            fix_config_keybindings(&mut test_bindings_toml, &reference_bindings_toml)
                == Ok(true)
        );

        assert!(test_bindings_toml == reference_bindings_toml);
    }

    #[test]
    fn test_fix_bindings_addition_of_new_and_previously_existent() {
        const REFERENCE_BINDINGS_SINGLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"UUU" = "NewAction"
		"UU" = "NewAction"
		"U" = "NewAction"
		"#;

        const TEST_BINDINGS_SINGLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"P" = "NewAction"
		"#;

        let mut test_bindings_toml = TEST_BINDINGS_SINGLE.parse::<Table>().unwrap();
        let reference_bindings_toml = REFERENCE_BINDINGS_SINGLE.parse::<Table>().unwrap();

        let old_test_bindings_toml = test_bindings_toml.clone();

        assert!(
            fix_config_keybindings(&mut test_bindings_toml, &reference_bindings_toml)
                == Ok(false)
        );

        assert!(test_bindings_toml == old_test_bindings_toml);

        const REFERENCE_BINDINGS_MULTIPLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"UUU" = "NewAction"
		"UU" = "NewAction"
		"U" = "NewAction"
		"#;

        const TEST_BINDINGS_MULTIPLE: &str = r#"
		"Ctrl+a" = "ToggleAlpha"
		"Ctrl+o" = "ToggleInverse"
		"C" = "CenterViewer"
		"h" = "MoveLeft"
		"j" = "MoveDown"
		"k" = "MoveUp"
		"l" = "MoveRight"
		"Up" = "MoveUp"
		"Left" = "MoveLeft"
		"Right" = "MoveRight"
		"Down" = "MoveDown"
		"Plus" = "ZoomIn"
		"-" = "ZoomOut"
		"g g" = "JumpFirstPage"
		"G" = "JumpLastPage"
		"PageUp" = "PrevPage"
		"PageDown" = "NextPage"
		"Ctrl+b" = "PrevPage"
		"Ctrl+f" = "NextPage"
		"q" = "Quit"
		"Q" = "Quit"

		"P" = "NewAction"
		"PO" = "NewAction"
		"#;

        let mut test_bindings_toml = TEST_BINDINGS_MULTIPLE.parse::<Table>().unwrap();
        let reference_bindings_toml =
            REFERENCE_BINDINGS_MULTIPLE.parse::<Table>().unwrap();

        let old_test_bindings_toml = test_bindings_toml.clone();

        assert!(
            fix_config_keybindings(&mut test_bindings_toml, &reference_bindings_toml)
                == Ok(false)
        );

        assert!(test_bindings_toml == old_test_bindings_toml);
    }

    #[test]
    fn default_config_binds_every_action() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        let bindings = config.bindings.unwrap();
        for action in ConfigAction::ALL {
            assert!(
                bindings.as_slice().iter().any(|b| b.action == action),
                "{:?} has no default key",
                action
            );
        }
    }

    #[test]
    fn retired_defaults_move_to_their_new_keys() {
        let mut current = r#"
        [bindings]
        "Ctrl+o" = "ToggleInverse"
        "j" = "MoveDown"
        "#
        .parse::<Table>()
        .unwrap();
        let default = r#"
        [bindings]
        "Ctrl+r" = "ToggleInverse"
        "Ctrl+o" = "JumpBack"
        "j" = "MoveDown"
        "#
        .parse::<Table>()
        .unwrap();
        assert_eq!(fix_config_toml(&mut current, &default), Ok(true));
        assert_eq!(current, default);
    }

    #[test]
    fn changed_bindings_are_not_retired() {
        /* Invert moved to Ctrl+x by the user; Ctrl+o already bound to something else */
        let mut current = r#"
        [bindings]
        "Ctrl+x" = "ToggleInverse"
        "Ctrl+o" = "Quit"
        "#
        .parse::<Table>()
        .unwrap();
        let default = r#"
        [bindings]
        "Ctrl+r" = "ToggleInverse"
        "Ctrl+o" = "JumpBack"
        "q" = "Quit"
        "#
        .parse::<Table>()
        .unwrap();
        fix_config_toml(&mut current, &default).unwrap();
        let bindings = current["bindings"].as_table().unwrap();
        assert_eq!(bindings["Ctrl+x"].as_str(), Some("ToggleInverse"));
        assert_eq!(bindings["Ctrl+o"].as_str(), Some("Quit"));
        assert!(!bindings.contains_key("Ctrl+r"));
    }

    const OLD_CONFIG: &str = r#"# my settings
[viewer]
# fast
scroll_speed = 35.0
render_precision = 2.0
memory_limit = 314572800
scale_min = 0.2
scale_amount = 0.5
margin_bottom = 4.0
pages_preloaded = 3
inverse_scroll = true

[viewer.uri_hint]
enabled = false

[state]
enabled = true

[bindings]
# quit fast
"q" = "Quit"
"#;

    #[test]
    fn upgrading_keeps_values_and_comments() {
        let new = upgrade(OLD_CONFIG).unwrap().expect("an old config changes");
        for kept in [
            "# my settings",
            "# quit fast",
            /* A renamed setting comes with its new explanation */
            "# How far one step of j, k, the arrows or the mouse wheel moves\nscroll_step",
            "scroll_step = 35.0",
            "sharpness = 2.0",
            "page_gap = 4.0",
            "invert_wheel = true",
            "link_hints = false",
            "remember_position = true",
            "# How much + and - zoom, in percent of the page width\nzoom_step = 10",
        ] {
            assert!(new.contains(kept), "{:?} missing from\n{}", kept, new);
        }
        for gone in [
            "memory_limit",
            "scale_min",
            "scale_amount",
            "pages_preloaded",
            "uri_hint",
            "[state]",
        ] {
            assert!(!new.contains(gone), "{:?} left in\n{}", gone, new);
        }
        let config: Config = toml::from_str(&new).unwrap();
        assert_eq!(config.viewer.scroll_step, 35.0);
        assert!(!config.viewer.link_hints);
        assert_eq!(
            upgrade(&new).unwrap(),
            None,
            "a second upgrade changes nothing"
        );
    }

    #[test]
    fn the_default_config_needs_no_upgrade() {
        assert_eq!(upgrade(DEFAULT_CONFIG).unwrap(), None);
        let config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(config.viewer.zoom_step, 10.0);
    }

    #[test]
    fn whole_numbers_are_fine_where_fractions_are_expected() {
        let text = DEFAULT_CONFIG.replace("sharpness = 1.5", "sharpness = 2");
        assert_eq!(upgrade(&text).unwrap(), None);
        let config: Config = toml::from_str(&text).unwrap();
        assert_eq!(config.viewer.sharpness, 2.0);
    }

    #[test]
    fn old_files_move_once_and_never_over_new_ones() {
        let root =
            std::env::temp_dir().join(format!("viewmer-move-{}", std::process::id()));
        let (old, new) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("config.toml"), "old config").unwrap();
        std::fs::write(old.join("theme.toml"), "old theme").unwrap();
        std::fs::write(new.join("theme.toml"), "new theme").unwrap();

        move_old_files(&old, &new);
        let read = |p: PathBuf| std::fs::read_to_string(p).unwrap();
        assert_eq!(read(new.join("config.toml")), "old config");
        assert_eq!(read(new.join("theme.toml")), "new theme");
        assert!(!old.join("config.toml").exists());
        assert!(
            old.join("theme.toml").exists(),
            "a file that wasn't moved stays"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
