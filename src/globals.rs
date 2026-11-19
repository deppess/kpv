use crate::config::Config;
use crossterm::terminal::WindowSize;
use std::sync::{
    atomic::{AtomicBool, AtomicU32},
    mpsc::Receiver,
    Mutex, OnceLock, RwLock,
};

pub const HELP_MSG: &str = r#"viewmer: kitty terminal PDF viewer

Usage: viewmer <file>
       viewmer --note <file> [name]   Makes the file's note if needed and prints where

Global options:
-h, --help          Print this usage information.
-v, --version       Print the current version.
"#;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONFIG_FILENAME: &str = "viewmer";
pub const DEFAULT_CONFIG: &str = r#"# viewmer settings. Colors are in theme.toml next to this file.
# These are the defaults: a setting or key left out keeps its default, so this file
# only needs what you change.

[viewer]
# How far one step of j, k, the arrows or the mouse wheel moves
scroll_step = 20
# How much + and - zoom, in percent of the page width
zoom_step = 10
# Pages are drawn at the screen's own resolution, times this. 1 is as sharp as
# the screen; less is faster and lighter
resolution = 1
# Space between pages
page_gap = 4
# Turn the mouse wheel's direction around
invert_wheel = false
# Show where a hovered link points in the status bar
link_hints = true
# Open every file where you left it, with its marks
remember_position = true

[notes]
# The folder your notes are in. The editor starts there, so its own settings for
# that folder (language servers and the like) apply.
vault = "~/Notes"
# Where new notes go, inside the vault; empty for the vault itself
folder = ""
# The shell in the notes panel; empty for the one you log in with
shell = ""
# The editor `pdfnote` opens; `hx` in the panel saves when focus leaves it
editor = "hx"
# A note to start new ones from, where {{title}} becomes the note's name and
# {{source}} a link to the PDF. Empty for a title and a link.
template = ""
# How wide the notes panel is, in percent of the window
width = 45

# Names for keys that start a layer, shown above what can follow them
[layers]
"Ctrl+Space" = "Notes"

# Keys, as "key" = "Action", added to the defaults; "key" = "" unbinds a default.
# The actions are listed in the README.
[bindings]
"Ctrl+a" = "ToggleAlpha"
"Ctrl+r" = "ToggleInverse"
"Ctrl+o" = "JumpBack"
"Ctrl+n" = "JumpForward"
"m" = "SetMark"
"`" = "GoToMark"
"'" = "GoToMark"
"C" = "CenterViewer"
"r" = "ToggleRuler"
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
"Ctrl+d" = "HalfPageDown"
"Ctrl+u" = "HalfPageUp"
"=" = "FitWidth"
"z" = "FitPage"
"q" = "Quit"
"Q" = "Quit"
"?" = "ShowHelp"
":" = "CommandLine"
"/" = "Search"
"n" = "NextMatch"
"N" = "PrevMatch"
"Backspace" = "ClearSearch"
"Esc" = "ClearSearch"
"o" = "ShowOutline"
"Tab" = "ShowOutline"
"Ctrl+h" = "FocusNote"
"Ctrl+l" = "FocusPdf"
"Ctrl+Space x" = "HideNote"
"Ctrl+Space p" = "TagPage"
"Ctrl+Space g" = "GoToTag"
"Ctrl+=" = "WidenNote"
"Ctrl+-" = "NarrowNote"
"J" = "NextPage"
"K" = "PrevPage"
"#;

/* Hate on me for those global singletons as much as you want. */
pub static CONFIG: OnceLock<Config> = OnceLock::new();
/* What kitty answers to image commands: the text after `;`, such as `OK` */
pub static RECEIVER_GR: OnceLock<Mutex<Receiver<String>>> = OnceLock::new();
pub static TERMINAL_SIZE: OnceLock<RwLock<WindowSize>> = OnceLock::new();
/* The scale pages are wanted at now, as f32 bits: the renderer skips requests for
 * another, which a newer zoom has replaced */
pub static RENDER_SCALE: AtomicU32 = AtomicU32::new(0);
pub static SOFTWARE_ID: OnceLock<String> = OnceLock::new();
pub static RUNNING: AtomicBool = AtomicBool::new(true);
