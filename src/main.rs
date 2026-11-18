mod app;
mod config;
mod drivers;
mod frame;
mod globals;
mod image;
mod input;
mod jumps;
mod labels;
mod outline;
mod ruler;
mod state;
mod store;
#[cfg(test)]
mod testutil;
mod threads;
mod ui;
mod viewer;

use std::collections::BTreeMap;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::io;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, RwLock};
use std::time::Instant;

use crossbeam_channel::{never, Receiver, TryRecvError};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, window_size, Clear, ClearType,
    EnterAlternateScreen, LeaveAlternateScreen, WindowSize,
};

use crate::app::*;
use crate::config::*;
use crate::drivers::commands::{
    DisableMouseCapturePixels, EnableMouseCapturePixels, PointerShape,
};
use crate::drivers::graphics::terminal_graphics_test_support;
use crate::frame::*;
use crate::globals::*;
use crate::input::*;
use crate::jumps::Jumps;
use crate::outline::Toc;
use crate::ruler::Lines;
use crate::state::*;
use crate::threads::event::InputEvent;
use crate::threads::renderer::RendererAction;
use crate::ui::{help_max_scroll, help_visible, toc_visible, viewport};
use crate::viewer::*;

/* The main loop's wakeup when the position is due to be written, after the channels */
const SAVE_DUE: usize = 8;

fn main() {
    /* ============================= Check the argument ============================= */
    let arg = std::env::args().nth(1).unwrap_or("".to_owned());
    match arg.as_str() {
        "-h" | "--help" | "" => {
            println!("{}", HELP_MSG);
            return;
        }
        "-v" | "--version" => {
            println!("viewmer v{} ({})", VERSION, RELEASED);
            return;
        }
        _ => (),
    }
    if !Path::new(&arg).is_file() {
        eprintln!("viewmer: no such file: {}", arg);
        std::process::exit(1);
    }
    let title = Path::new(&arg)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| arg.clone());

    /* ============================= Uncook the terminal ============================= */
    if let Err(e) = enable_raw_mode().and_then(|_| {
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            Hide,
            Clear(ClearType::All),
            EnableMouseCapturePixels
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
    let event_inputs = threads::event::spawn();
    RECEIVER_GR.get_or_init(|| Mutex::new(event_inputs.2));

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

    /* ======================= Calculate padding for all images ====================== */
    let winsize_tmp = window_size()
        .unwrap_or_else(|e| fail(&format!("Could not read the window size: {}", e)));
    let winsize = WindowSize {
        rows: winsize_tmp.rows,
        columns: winsize_tmp.columns,
        width: winsize_tmp.width,
        height: winsize_tmp.height,
    };
    TERMINAL_SIZE.get_or_init(|| RwLock::new(winsize_tmp));

    let config = CONFIG.get().unwrap();

    let pxpercol = winsize.width as f64 / winsize.columns as f64;
    let pxperrow = winsize.height as f64 / winsize.rows as f64;

    let paddingcol = (pxpercol * config.viewer.render_precision
        / config.viewer.scale_min as f64)
        .ceil() as usize;
    let paddingrow = (pxperrow * config.viewer.render_precision
        / config.viewer.scale_min as f64)
        .ceil() as usize;

    IMAGE_PADDING.get_or_init(|| std::cmp::max(paddingcol, paddingrow));

    /* =========== Generate a random ID which is unique for every instance =========== */
    let random_u64 = RandomState::new().build_hasher().finish();
    SOFTWARE_ID.get_or_init(|| format!("{random_u64:X}"));
    drivers::graphics::terminal_graphics_remove_leftovers();

    /* ====================== Viewer - The core of this program ====================== */
    let (mut viewer, sender_rerender) = Viewer::new();

    let (mut renderer, result_receiver) = threads::renderer::Renderer::new();
    renderer
        .run(&arg)
        .and_then(|_| renderer.send_and_confirm_action(RendererAction::Load))
        .unwrap_or_else(|e| fail(&e));

    /* ========================= Thread notifying file change ======================== */
    let (mut file_reload, reload_error) = match threads::fnotify::spawn(&arg) {
        Ok(receiver) => (receiver, None),
        Err(e) => (never(), Some(format!("Live reload is off: {}", e))),
    };
    /* Closing the window or `kill` quits like `q`; without it they quit unsaved */
    let stop = threads::signals::spawn().unwrap_or_else(|_| never());

    /* ============================== Main program loop ============================== */
    let mut app = App {
        mode: Mode::Normal,
        count: None,
        search: None,
        message: reload_error,
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
    };

    let mut current_mouse = MouseEvent {
        kind: MouseEventKind::Moved,
        column: u16::MAX,
        row: u16::MAX,
        modifiers: KeyModifiers::NONE,
    };

    'main: loop {
        /* sel[0..1] are the results from the renderer thread */
        let mut sel = result_receiver.construct_biased_select();
        sel.recv(&file_reload);
        sel.recv(&sender_rerender);
        /* Key event */
        sel.recv(&event_inputs.0);
        /* Mouse event */
        sel.recv(&event_inputs.1);
        /* Window size change input */
        sel.recv(&event_inputs.3);
        /* A signal to stop */
        sel.recv(&stop);

        /* Keys go first: a busy renderer or a redraw loop must never make the viewer
         * ignore input, `q` above all */
        let index_ready = if !event_inputs.0.is_empty() {
            4
        } else if let Some((_, due)) = &app.save_due {
            /* Wake up to write the position once the view has stayed put */
            sel.ready_timeout(due.saturating_duration_since(Instant::now()))
                .unwrap_or(SAVE_DUE)
        } else {
            sel.ready()
        };

        match index_ready {
            0 | 1 => {
                let Ok(result) = result_receiver.try_recv_priority(index_ready) else {
                    app.fatal = Some("The renderer stopped".to_owned());
                    break 'main;
                };

                if handle_result(result, &arg, &mut viewer, &renderer, &mut app) {
                    result_receiver.clear_priority(1);
                }
            }
            2 => match take(&file_reload) {
                Gone => {
                    /* The watcher stopped; keep going without live reload */
                    file_reload = never();
                    app.message = Some("Live reload stopped".to_owned());
                }
                Taken(()) => {
                    clear_channel!(file_reload);
                    tell_renderer(&renderer, &mut app, RendererAction::Load);
                }
                Nothing => continue 'main,
            },
            3 => {
                let _ = sender_rerender.try_recv();
            }
            4 => {
                let input = match take(&event_inputs.0) {
                    Taken(input) => input,
                    Nothing => continue 'main,
                    Gone => {
                        app.fatal = Some("Lost the terminal".to_owned());
                        break 'main;
                    }
                };
                match input {
                    InputEvent::Key(key) => {
                        if key.kind != KeyEventKind::Release
                            && handle_key(
                                key,
                                &mut key_matcher,
                                &mut viewer,
                                &renderer,
                                &mut app,
                            )
                        {
                            break 'main;
                        }
                    }
                    InputEvent::MouseScroll(kind, _modifiers) => {
                        if handle_mouse_scroll(kind, &mut viewer, &mut app) {
                            break 'main;
                        }
                    }
                }
            }
            5 => match take(&event_inputs.1) {
                Taken(mouse) => current_mouse = mouse,
                Nothing => continue 'main,
                Gone => {
                    app.fatal = Some("Lost the terminal".to_owned());
                    break 'main;
                }
            },
            6 => {
                if let Gone = take(&event_inputs.3) {
                    app.fatal = Some("Lost the terminal".to_owned());
                    break 'main;
                }
                clear_channel!(event_inputs.3);

                /* The resize event only carries cells; ask again for pixels as well */
                if let Ok(size) = window_size() {
                    *TERMINAL_SIZE.get().unwrap().write().unwrap() = size;
                }
                viewer.bound_viewer();
                app.toc.scroll_by(0, toc_visible(&viewport()));
                if let Mode::Help { scroll } = &mut app.mode {
                    let max = help_max_scroll(app.help.len(), help_visible(&viewport()));
                    *scroll = (*scroll).min(max);
                }
            }
            7 => break 'main,
            SAVE_DUE => {
                save_state(&mut app);
                continue 'main;
            }
            _ => unreachable!(),
        };
        if app.fatal.is_some() {
            break 'main;
        }

        schedule_save(&mut app, &viewer);

        let hovered = app.link.take();
        let clicked = handle_pointer(&mut current_mouse, &mut viewer, &mut app);

        /* The mouse moves all the time. Unless it clicked or changed the link hint, the
         * screen stays as it is, and redrawing would make kitty place every page again. */
        if index_ready == 5 && !clicked && app.link == hovered {
            continue 'main;
        }

        /* ================================ Draw a frame ================================ */
        let gr = RECEIVER_GR.get().unwrap().lock().unwrap();
        /* Answers that came too late for the previous frame would be mistaken for
         * this frame's */
        while gr.try_recv().is_ok() {}

        let displayed = match draw_frame(&title, &mut viewer, &renderer, &mut app) {
            Ok(displayed) => displayed,
            Err(e) => {
                app.fatal = Some(e);
                break 'main;
            }
        };

        check_placements(&gr, displayed, &mut viewer, &mut app);
    }

    if app.store.is_some() {
        app.save_due = Some((file_state(&viewer, &app.marks), Instant::now()));
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
    let _ = execute!(
        io::stdout(),
        DisableMouseCapturePixels,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
}

/* Gives the terminal back and exits with `message` */
fn fail(message: &str) -> ! {
    restore_terminal();
    eprintln!("viewmer: {}", message);
    std::process::exit(1);
}

/* What a channel the main loop woke up for holds */
enum Take<T> {
    Taken(T),
    /* Another wakeup took it first */
    Nothing,
    /* The other side is gone */
    Gone,
}
use Take::*;

fn take<T>(channel: &Receiver<T>) -> Take<T> {
    match channel.try_recv() {
        Ok(x) => Taken(x),
        Err(TryRecvError::Empty) => Nothing,
        Err(TryRecvError::Disconnected) => Gone,
    }
}
