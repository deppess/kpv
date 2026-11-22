mod app;
mod config;
mod drivers;
mod frame;
mod globals;
mod image;
mod input;
mod outline;
mod state;
mod store;
#[cfg(test)]
mod testutil;
mod theme;
mod threads;
mod ui;
mod viewer;

use std::hash::{BuildHasher, Hasher, RandomState};
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, window_size, Clear, ClearType,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use keybinds::Keybinds;

use crate::app::*;
use crate::config::*;
use crate::drivers::commands::{
    DisableMouseCapturePixels, EnableMouseCapturePixels, FreeImages, PointerShape,
};
use crate::drivers::graphics::terminal_graphics_test_support;
use crate::frame::*;
use crate::globals::*;
use crate::input::*;
use crate::outline::Toc;
use crate::state::*;
use crate::theme::{set_theme, Theme};
use crate::threads::event::InputEvent;
use crate::threads::renderer::{Renderer, RendererAction};
use crate::threads::Wake;
use crate::ui::{help_max_scroll, help_visible, toc_visible, viewport};
use crate::viewer::*;

/* The least time between frames: 120 a second */
const FRAME_GAP: Duration = Duration::from_millis(8);

fn main() {
    /* ============================= Check the argument ============================= */
    let arg = std::env::args().nth(1).unwrap_or("".to_owned());
    match arg.as_str() {
        "-h" | "--help" | "" => {
            println!("{}", HELP_MSG);
            return;
        }
        "-v" | "--version" => {
            println!("kpv {}", VERSION);
            return;
        }
        _ => (),
    }
    if !Path::new(&arg).is_file() {
        eprintln!("kpv: no such file: {}", arg);
        std::process::exit(1);
    }
    /* ============================= Uncook the terminal ============================= */
    if let Err(e) = enable_raw_mode().and_then(|_| {
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            Hide,
            Clear(ClearType::All),
            EnableMouseCapturePixels,
            /* Keys like Ctrl-i and Tab, or Esc and Alt, arrive apart, for bindings */
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        )
    }) {
        fail(&format!("Could not set up the terminal: {}", e));
    }

    /* ========================== Cook the terminal on panic ========================= */
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_panic(info);
    }));

    /* ============================= STDIN parser thread ============================= */
    let inbox = threads::inbox();
    RECEIVER_GR.get_or_init(|| Mutex::new(threads::event::spawn()));

    /* ========== Check if the terminal supports the Kitty graphics protocol ========= */
    terminal_graphics_test_support().unwrap_or_else(|e| fail(&e));

    /* ================================= Load config ================================= */
    let mut config = config_load_or_create()
        .unwrap_or_else(|e| fail(&format!("Could not load the config: {}", e)));
    /* The main loop keeps the keys, which change as sequences are typed */
    let mut key_matcher = std::mem::take(&mut config.bindings);
    CONFIG.get_or_init(|| config);
    load_panel_sizes();
    /* A broken theme falls back to the default colors and says so */
    let theme_problems = match config_folder() {
        Ok(folder) => {
            let (theme, problems) = Theme::load(&folder.join("theme.toml"));
            set_theme(theme);
            send_tints();
            problems
        }
        Err(e) => vec![e],
    };

    /* ============================ The window's size ============================ */
    let winsize = window_size()
        .unwrap_or_else(|e| fail(&format!("Could not read the window size: {}", e)));
    TERMINAL_SIZE.get_or_init(|| RwLock::new(winsize));

    /* =========== Generate a random ID which is unique for every instance =========== */
    let random_u64 = RandomState::new().build_hasher().finish();
    SOFTWARE_ID.get_or_init(|| format!("{random_u64:X}"));
    drivers::graphics::terminal_graphics_remove_leftovers();

    /* ====================== Viewer - The core of this program ====================== */
    let mut viewer = Viewer::new();

    let renderer = threads::renderer::Renderer::start(&arg);
    renderer
        .send_action(RendererAction::Load)
        .unwrap_or_else(|e| fail(&e));

    /* ========================= Thread notifying file change ======================== */
    let reload_error = threads::fnotify::spawn(&arg, || threads::wake(Wake::Reload))
        .err()
        .map(|e| format!("Live reload is off: {}", e));
    /* Closing the window or `kill` quits like `q`; without it they quit unsaved */
    let _ = threads::signals::spawn();

    /* ============================== Main program loop ============================== */
    let mut app = App {
        mode: Mode::Normal,
        count: None,
        search: None,
        message: reload_error.or_else(|| match theme_problems.as_slice() {
            [] => None,
            [one] => Some(format!("theme.toml: {}", one)),
            [first, rest @ ..] => {
                Some(format!("theme.toml: {} (and {} more)", first, rest.len()))
            }
        }),
        link: None,
        help: ui::help_lines(&help_entries(&key_matcher)),
        toc: Toc::default(),
        store: None,
        pointer: PointerShape::Default,
        save_due: None,
        inverse_toggled: None,
        fatal: None,
        shown: None,
        title: frame_title(None, None, &arg),
        message_shown: None,
    };

    let mut current_mouse = MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::MAX,
        row: u16::MAX,
        modifiers: KeyModifiers::NONE,
    };

    /* When the last frame went out, and the next one when it had to wait */
    let mut last_frame = Instant::now() - FRAME_GAP;
    let mut frame_due: Option<Instant> = None;

    'main: loop {
        /* Wake up to write the position once the view has stayed put, to take a
         * message away once it has been on show for a while, and to draw a frame
         * that had to wait */
        let due = [
            app.save_due.as_ref().map(|(_, at)| *at),
            message_due(&mut app),
            frame_due,
        ]
        .into_iter()
        .flatten()
        .min();
        let first = match due {
            Some(due) => inbox
                .recv_timeout(due.saturating_duration_since(Instant::now()))
                .ok(),
            None => inbox.recv().ok(),
        };
        let timed_out = first.is_none();
        /* Everything there is, keys first: a busy renderer or a redraw loop must never
         * make the viewer ignore input, `q` above all. Pages arriving together are
         * placed in one frame. */
        let mut wakes: Vec<Wake> = first.into_iter().chain(inbox.try_iter()).collect();
        wakes.sort_by_key(|w| !matches!(w, Wake::Input(_)));
        let only_mouse =
            !wakes.is_empty() && wakes.iter().all(|w| matches!(w, Wake::Mouse(_)));
        let hovered = app.link.take();
        let mut clicked = false;
        let mut resized = false;

        for wake in wakes {
            match wake {
                Wake::Rendered(result) => {
                    handle_result(result, &arg, &mut viewer, &renderer, &mut app);
                }
                Wake::Reload => tell_renderer(&renderer, &mut app, RendererAction::Load),
                Wake::Redraw => (),
                Wake::Input(input) => {
                    if handle_input(
                        input,
                        &mut key_matcher,
                        &mut viewer,
                        &renderer,
                        &mut app,
                    ) {
                        break 'main;
                    }
                }
                Wake::Mouse(mouse) => {
                    current_mouse = mouse;
                    /* Each click counts, even with more of the mouse behind it */
                    clicked |= handle_pointer(&mut current_mouse, &mut viewer, &mut app);
                }
                Wake::Resize => resized = true,
                Wake::Stop => break 'main,
                Wake::Fatal(e) => {
                    app.fatal = Some(e);
                    break 'main;
                }
            }
            if app.fatal.is_some() {
                break 'main;
            }
        }

        if resized {
            /* The resize event only carries cells; ask again for pixels as well */
            if let Ok(size) = window_size() {
                *TERMINAL_SIZE.get().unwrap().write().unwrap() = size;
            }
            viewer.refit();
            app.toc.scroll_by(0, toc_visible(&viewport()));
            if let Mode::Help { scroll } = &mut app.mode {
                let max = help_max_scroll(app.help.len(), help_visible(&viewport()));
                *scroll = (*scroll).min(max);
            }
        }

        let now = Instant::now();
        if app.save_due.as_ref().is_some_and(|(_, at)| *at <= now) {
            save_state(&mut app);
        }
        let shown_long = app
            .message_shown
            .as_ref()
            .is_some_and(|(_, since)| *since + MESSAGE_TIME <= now);
        if shown_long {
            app.message = None;
        } else if timed_out && frame_due.is_none() {
            /* Only the position was written: the screen is as it was */
            app.link = hovered;
            continue 'main;
        }

        schedule_save(&mut app, &viewer);
        clicked |= handle_pointer(&mut current_mouse, &mut viewer, &mut app);

        /* The mouse moves all the time. Unless it clicked or changed the link hint, the
         * screen stays as it is, and redrawing would make kitty place every page again. */
        if only_mouse && !clicked && app.link == hovered {
            continue 'main;
        }

        /* A burst of wakes, such as pages arriving or the wheel spinning, would otherwise
         * have every one drawn, more frames than kitty shows */
        if last_frame.elapsed() < FRAME_GAP {
            frame_due = Some(last_frame + FRAME_GAP);
            continue 'main;
        }
        frame_due = None;
        last_frame = Instant::now();

        /* ================================ Draw a frame ================================ */
        let gr = RECEIVER_GR.get().unwrap().lock().unwrap();
        /* Answers that came too late for the previous frame would be mistaken for
         * this frame's */
        while gr.try_recv().is_ok() {}

        let displayed = match draw_frame(&mut viewer, &renderer, &mut app) {
            Ok(displayed) => displayed,
            Err(e) => {
                app.fatal = Some(e);
                break 'main;
            }
        };

        check_placements(&gr, displayed, &mut viewer, &mut app);
    }

    if app.store.is_some() {
        app.save_due = Some((file_state(&viewer, &app), Instant::now()));
        save_state(&mut app);
    }
    RUNNING.store(false, Ordering::Release);

    /* ========================== Cook the terminal on exit ========================== */
    restore_terminal();
    drivers::graphics::terminal_graphics_remove_leftovers();
    if let Some(e) = app.fatal {
        eprintln!("kpv: {}", e);
        std::process::exit(1);
    }
}

/* Turns off what kpv turned on and gives the terminal back as it was. Errors are
 * ignored: this also runs when the terminal is gone, and from the panic hook. */
fn restore_terminal() {
    let _ = execute!(
        io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableMouseCapturePixels,
        FreeImages,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
}

/* Gives the terminal back and exits with `message` */
fn fail(message: &str) -> ! {
    restore_terminal();
    eprintln!("kpv: {}", message);
    std::process::exit(1);
}

/* A key or the wheel; `true` to quit */
fn handle_input(
    input: InputEvent,
    key_matcher: &mut Keybinds<ConfigAction>,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> bool {
    match input {
        InputEvent::Key(key) if key.kind == KeyEventKind::Release => false,
        InputEvent::Key(key) => handle_key(key, key_matcher, viewer, renderer, app),
        InputEvent::MouseScroll(kind) => {
            handle_mouse_scroll(kind, viewer, app);
            false
        }
    }
}
