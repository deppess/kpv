use crate::{drivers::graphics::GraphicsResponse, Config};
use crossbeam_channel::Receiver;
use crossterm::terminal::WindowSize;
use std::sync::{atomic::AtomicBool, Mutex, OnceLock, RwLock};

pub const HELP_MSG: &str = r#"viewmer: kitty terminal PDF viewer

Usage: viewmer <file>

Global options:
-h, --help          Print this usage information.
-v, --version       Print the current version.
"#;

pub const VERSION: &str = "0.1.0";
pub const RELEASED: &str = "2026-11-08";
pub const CONFIG_FILENAME: &str = "viewmer";
pub const DEFAULT_CONFIG: &str = r#"
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
# Show where a hovered link points in the status bar
enabled = true

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
"?" = "ShowHelp"
":" = "CommandLine"
"/" = "Search"
"n" = "NextMatch"
"N" = "PrevMatch"
"Backspace" = "ClearSearch"
"Esc" = "ClearSearch"
"o" = "ShowOutline"
"Tab" = "ShowOutline"
"#;

/* Hate on me for those global singletons as much as you want. */
pub static CONFIG: OnceLock<Config> = OnceLock::new();
pub static RECEIVER_GR: OnceLock<Mutex<Receiver<GraphicsResponse>>> = OnceLock::new();
pub static TERMINAL_SIZE: OnceLock<RwLock<WindowSize>> = OnceLock::new();
pub static IMAGE_PADDING: OnceLock<usize> = OnceLock::new();
pub static SOFTWARE_ID: OnceLock<String> = OnceLock::new();
pub static RUNNING: AtomicBool = AtomicBool::new(true);

#[macro_export]
macro_rules! chan_has {
    ($chan:expr) => {
        $chan.peek().is_some()
    };
}

#[macro_export]
macro_rules! clear_channel {
    ($chan:expr) => {
        while let Ok(_) = $chan.try_recv() {}
    };
}

#[macro_export]
macro_rules! has_elapsed {
    ($var:expr) => {{
        let elapsed = $var.elapsed().unwrap().as_millis();
        if elapsed >= 500 {
            $var = SystemTime::now();
        }
        elapsed >= 500
    }};
}
