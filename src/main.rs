mod app;
mod config;
mod drivers;
mod frame;
mod globals;
mod image;
mod input;
mod jumps;
mod labels;
mod notes;
mod numbers;
mod outline;
mod ruler;
mod state;
mod store;
#[cfg(test)]
mod testutil;
mod theme;
mod threads;
mod ui;
mod viewer;
mod watch;

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, SetCursorStyle, Show};
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, EnableBracketedPaste, EnableFocusChange,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, window_size, Clear, ClearType,
    EnterAlternateScreen, LeaveAlternateScreen,
};
use keybinds::{Keybinds, Mods};

use crate::app::*;
use crate::config::*;
use crate::drivers::commands::{
    DisableMouseCapturePixels, EnableMouseCapturePixels, FreeImages, PointerShape,
};
use crate::drivers::graphics::terminal_graphics_test_support;
use crate::frame::*;
use crate::globals::*;
use crate::input::*;
use crate::jumps::Jumps;
use crate::outline::Toc;
use crate::ruler::Lines;
use crate::state::*;
use crate::theme::{set_theme, Theme};
use crate::threads::event::InputEvent;
use crate::threads::renderer::{Renderer, RendererAction};
use crate::threads::Wake;
use crate::ui::{help_max_scroll, help_visible, inner, toc_visible};
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
            println!("viewmer {}", VERSION);
            return;
        }
        _ => (),
    }
    /* `pdfnote` in the notes terminal: find or make a PDF's note and say where it is */
    if arg == "--note" {
        let args: Vec<String> = std::env::args().skip(2).collect();
        let Some((pdf, name)) = args.split_first() else {
            eprintln!("viewmer: --note needs the PDF");
            std::process::exit(1);
        };
        match note_for(pdf, &name.join(" ")) {
            Ok(path) => println!("{}", path.display()),
            Err(e) => {
                eprintln!("viewmer: {}", e);
                std::process::exit(1);
            }
        }
        return;
    }
    if !Path::new(&arg).is_file() {
        eprintln!("viewmer: no such file: {}", arg);
        std::process::exit(1);
    }
    /* ===================== Run under the crash watcher ===================== */
    if std::env::var_os(watch::WATCHED).is_none() {
        let args: Vec<String> = std::env::args().collect();
        watch::run(&args, reset_screen);
    }

    /* ============================= Uncook the terminal ============================= */
    if let Err(e) = enable_raw_mode().and_then(|_| {
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            Hide,
            Clear(ClearType::All),
            EnableMouseCapturePixels,
            /* Keys like Shift-Enter and Ctrl-h arrive as themselves, for the editor in
             * the notes panel and for bindings */
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            ),
            EnableBracketedPaste,
            EnableFocusChange
        )
    }) {
        fail(&format!("Could not set up the terminal: {}", e));
    }

    /* ========================== Cook the terminal on panic ========================= */
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        watch::log(&format!("viewmer panicked: {}", info));
        restore_terminal();
        default_panic(info);
    }));

    /* ============================= STDIN parser thread ============================= */
    let inbox = threads::inbox();
    RECEIVER_GR.get_or_init(|| Mutex::new(threads::event::spawn()));

    /* ========== Check if the terminal supports the Kitty graphics protocol ========= */
    terminal_graphics_test_support().unwrap_or_else(|e| fail(&e));

    /* ================================= Load config ================================= */
    let mut key_matcher;
    {
        let mut config = config_load_or_create()
            .unwrap_or_else(|e| fail(&format!("Could not load the config: {}", e)));
        let Some(bindings) = config.bindings.take() else {
            fail("The config has no [bindings]");
        };
        key_matcher = bindings;
        CONFIG.get_or_init(|| config);
    }
    /* While the note has the keys, only the notes keys with Ctrl or Alt are taken from
     * the editor */
    let mut global_keys = Keybinds::new(
        key_matcher
            .as_slice()
            .iter()
            .filter(|b| {
                b.action.is_pane()
                    && b.seq
                        .as_slice()
                        .first()
                        .is_some_and(|k| k.mods().intersects(Mods::CTRL | Mods::ALT))
            })
            .cloned()
            .collect(),
    );
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
        message: recovered_message().or(reload_error).or_else(|| {
            match theme_problems.as_slice() {
                [] => None,
                [one] => Some(format!("theme.toml: {}", one)),
                [first, rest @ ..] => {
                    Some(format!("theme.toml: {} (and {} more)", first, rest.len()))
                }
            }
        }),
        link: None,
        help: ui::help_lines(&help_entries(&key_matcher)),
        toc: Toc::default(),
        store: None,
        jumps: Jumps::default(),
        marks: BTreeMap::new(),
        pending_mark: None,
        ruler: None,
        lines: Lines::new(&arg),
        ruler_image: None,
        pointer: PointerShape::Default,
        save_due: None,
        throttle: LastExecuted::default(),
        fatal: None,
        shown: None,
        notes: crate::notes::Notes::new(Path::new(&arg)),
        title: frame_title(None, None, &arg),
        message_shown: None,
        offset_by_hand: false,
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
                        &mut global_keys,
                        &mut viewer,
                        &renderer,
                        &mut app,
                    ) {
                        break 'main;
                    }
                }
                Wake::Mouse(mouse) => {
                    current_mouse = mouse;
                    if notes_mouse(&mut current_mouse, &mut viewer, &mut app) {
                        current_mouse.kind = MouseEventKind::Moved;
                    }
                    /* Each click counts, even with more of the mouse behind it */
                    clicked |= handle_pointer(&mut current_mouse, &mut viewer, &mut app);
                }
                Wake::Resize => resized = true,
                Wake::Terminal => {
                    if let Some(pane) = app.notes.terminal.as_mut() {
                        if pane.take_output() {
                            app.notes.pass_graphics();
                        } else {
                            /* The shell ended (`exit`): the panel closes */
                            app.notes.ended();
                            apply_layout(&mut viewer, &mut app);
                        }
                    }
                }
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
            apply_layout(&mut viewer, &mut app);
            viewer.refit();
            app.toc.scroll_by(0, toc_visible(&ui::viewport()));
            if let Mode::Help { scroll } = &mut app.mode {
                let max = help_max_scroll(app.help.len(), help_visible(&inner()));
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

        /* A program pouring out text in the notes terminal would otherwise have every
         * read drawn, thousands of frames a second that kitty never shows */
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

    /* The note is saved and the editor quits before anything else goes */
    app.notes.close();
    if app.store.is_some() {
        app.save_due = Some((file_state(&viewer, &app), Instant::now()));
        save_state(&mut app);
    }
    RUNNING.store(false, Ordering::Release);

    /* ========================== Cook the terminal on exit ========================== */
    restore_terminal();
    drivers::graphics::terminal_graphics_remove_leftovers();
    if let Some(e) = app.fatal {
        eprintln!("viewmer: {}", e);
        std::process::exit(1);
    }
}

/* Gives the terminal back as it was. Errors are ignored: this also runs when the
 * terminal is gone, and from the panic hook. */
fn restore_terminal() {
    reset_screen();
    let _ = disable_raw_mode();
}

/* Turns off what viewmer turned on in the terminal and leaves its screen; the crash
 * watcher does this too, for a viewer that couldn't */
fn reset_screen() {
    let _ = execute!(
        io::stdout(),
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
        DisableFocusChange,
        DisableMouseCapturePixels,
        FreeImages,
        LeaveAlternateScreen,
        SetCursorStyle::DefaultUserShape,
        Show
    );
}

/* The note for `pdf`: the one named `name`, or else after the PDF's title or file name */
fn note_for(pdf: &str, name: &str) -> Result<std::path::PathBuf, String> {
    let mut config = config_load_or_create()?;
    config.bindings = None;
    CONFIG.get_or_init(|| config);
    let title = mupdf::Document::open(pdf)
        .ok()
        .and_then(|d| d.metadata(mupdf::MetadataName::Title).ok())
        .map(|t| t.trim().to_owned())
        .filter(|t| real_title(t));
    let name = Some(name).filter(|n| !n.trim().is_empty());
    crate::notes::note_for(Path::new(pdf), title.as_deref(), name)
}

/* What the viewer says when the watcher started it again after a crash */
fn recovered_message() -> Option<String> {
    std::env::var_os(watch::RECOVERED)?;
    let log = watch::crash_log()
        .map(|f| f.display().to_string())
        .unwrap_or_default();
    Some(format!(
        "Started again after a crash; what happened is in {}",
        log
    ))
}

/* Gives the terminal back and exits with `message` */
fn fail(message: &str) -> ! {
    restore_terminal();
    eprintln!("viewmer: {}", message);
    std::process::exit(1);
}

/* A key, the wheel, a paste or focus moving; `true` to quit */
fn handle_input(
    input: InputEvent,
    key_matcher: &mut Keybinds<ConfigAction>,
    global_keys: &mut Keybinds<ConfigAction>,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> bool {
    let notes_keys = app.notes.open && app.notes.focused;
    match input {
        InputEvent::Key(key) if key.kind == KeyEventKind::Release => (),
        InputEvent::Key(key) if notes_keys => {
            handle_notes_key(key, global_keys, viewer, app);
        }
        InputEvent::Key(key) => {
            if handle_key(key, key_matcher, viewer, renderer, app) {
                return true;
            }
        }
        InputEvent::MouseScroll(kind, mods, x, y) => {
            if handle_mouse_scroll(kind, mods, (x, y), viewer, app) {
                return true;
            }
        }
        InputEvent::Paste(text) if notes_keys => {
            if let Some(editor) = &mut app.notes.terminal {
                editor.paste(&text);
            }
        }
        InputEvent::Paste(_) => (),
        /* Leaving the terminal window saves the note */
        InputEvent::Focus(gained) => {
            if let (true, Some(editor)) = (notes_keys, &mut app.notes.terminal) {
                editor.focus(gained);
            }
        }
    }
    false
}
