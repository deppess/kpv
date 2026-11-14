mod drivers;
use crate::drivers::commands::ClearImages;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, window_size, BeginSynchronizedUpdate, Clear,
    ClearType, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    WindowSize,
};
use crossterm::{execute, queue};
use drivers::commands::{
    DisableMouseCapturePixels, EnableMouseCapturePixels, PointerShape, SetPointerShape,
};
use drivers::graphics::terminal_graphics_test_support;
use keybinds::{KeyInput, Keybinds};

mod threads;
use threads::event::InputEvent;
use threads::renderer::{Renderer, RendererAction, RendererResult};

mod image;
use crate::image::*;

mod labels;

mod outline;

mod jumps;
use crate::jumps::{Jumps, Spot};

mod ruler;
use crate::ruler::{Lines, Ruler};

mod store;
use crate::store::{FileState, Store};

#[cfg(test)]
mod testutil;
use crate::outline::{breadcrumb, current_section, Toc, TocKey};

mod viewer;
use crate::viewer::*;

mod globals;
use crate::globals::*;

mod config;
use crate::config::*;

mod state;
use crate::state::*;

mod ui;
use crate::ui::{help_max_scroll, help_visible, toc_visible, viewport, Segment, TocRow};

use std::collections::BTreeMap;
use std::hash::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

/* Longest wait for kitty to confirm placing a page */
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);
/* How long the view must stay put before its position is written */
const SAVE_DELAY: Duration = Duration::from_secs(2);

/* Tracks the last executed times of signals for throattling */
struct LastExecuted {
    pub alpha: SystemTime,
    pub inverse: SystemTime,
}

/* Everything the main loop keeps besides the viewer itself */
struct App {
    mode: Mode,
    count: Option<usize>,
    search: Option<SearchState>,
    /* One-off status text, cleared by the next key */
    message: Option<String>,
    /* Where the hovered link points */
    link: Option<String>,
    help: Vec<String>,
    /* The contents panel, kept between openings */
    toc: Toc,
    /* Where this file's state is remembered; `None` when that's turned off */
    store: Option<Store>,
    /* Where jumps started, for Ctrl-o and Ctrl-n */
    jumps: Jumps,
    marks: BTreeMap<char, Spot>,
    /* `m` or a backtick was typed and waits for its letter */
    pending_mark: Option<ConfigAction>,
    /* The reading ruler when it's on, the text rows it steps through, and the image
     * last sent for it */
    ruler: Option<Ruler>,
    lines: Lines,
    ruler_image: Option<RulerPlacement>,
    /* The mouse pointer shape last sent to the terminal */
    pointer: PointerShape,
    /* The position waiting to be written, and when */
    save_due: Option<(FileState, Instant)>,
    throttle: LastExecuted,
}

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
    enable_raw_mode().expect("Could not cook the terminal");
    execute!(io::stdout(), EnterAlternateScreen).expect("Could not enter alt mode");
    execute!(io::stdout(), Hide).expect("Could not hide cursor");
    execute!(io::stdout(), Clear(ClearType::All)).expect("Could not clear terminal");
    execute!(io::stdout(), EnableMouseCapturePixels)
        .expect("Could not enable mouse capture");

    /* ========================== Cook the terminal on panic ========================= */
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        /* Atleast try to cook the terminal on error before printing the message.
         * Do not handle the error to prevent possible infinite loops when panicking. */

        let _ = execute!(io::stdout(), DisableMouseCapturePixels);
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = execute!(io::stdout(), Show);
        let _ = disable_raw_mode();
        default_panic(info);
    }));

    /* ============================= STDIN parser thread ============================= */
    let event_inputs = threads::event::spawn();
    RECEIVER_GR.get_or_init(|| Mutex::new(event_inputs.2));

    /* ========== Check if the terminal supports the Kitty graphics protocol ========= */
    terminal_graphics_test_support()
        .expect("Error when testing terminal support of the Kitty graphics protocol");

    /* ================================= Load config ================================= */
    let mut key_matcher;
    {
        let mut config = config_load_or_create().expect("Could not load config");
        key_matcher = config.bindings.unwrap();
        config.bindings = None;
        CONFIG.get_or_init(|| config);
    }

    /* ======================= Calculate padding for all images ====================== */
    let winsize_tmp = window_size().expect("Could not get win size");
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
    renderer.run(&arg).expect("Couldn't start renderer thread");
    renderer
        .send_and_confirm_action(RendererAction::Load)
        .expect("Cannot send action to renderer thread");

    /* ========================= Thread notifying file change ======================== */
    let file_reload = threads::fnotify::spawn(&arg).expect("Could not init file watcher");

    /* ============================== Main program loop ============================== */
    let mut app = App {
        mode: Mode::Normal,
        count: None,
        search: None,
        message: None,
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
        throttle: LastExecuted {
            alpha: SystemTime::now() - Duration::from_millis(500),
            inverse: SystemTime::now() - Duration::from_millis(500),
        },
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

        /* Keys go first: a busy renderer or a redraw loop must never make the viewer
         * ignore input, `q` above all */
        let index_ready = if !event_inputs.0.is_empty() {
            4
        } else if let Some((_, due)) = &app.save_due {
            /* Wake up to write the position once the view has stayed put */
            sel.ready_timeout(due.saturating_duration_since(Instant::now()))
                .unwrap_or(7)
        } else {
            sel.ready()
        };

        match index_ready {
            0 | 1 => {
                let result = result_receiver
                    .try_recv_priority(index_ready)
                    .expect("Could not receive priority");

                match result {
                    RendererResult::PageMetadata {
                        max_page_width,
                        cumulative_heights,
                        widths,
                        links,
                        labels,
                        outline,
                        id,
                    } => {
                        let uninit = viewer.is_uninit();

                        viewer.update_metadata(
                            max_page_width,
                            &cumulative_heights,
                            &widths,
                            &links,
                            labels,
                        );
                        viewer.invalidate_registry();
                        viewer.center_viewer();
                        if uninit {
                            viewer.scale_page2terminal();
                            app.store = open_store(&arg, id, &mut viewer, &mut app.marks);
                        } else if let Some(store) = app.store.as_mut() {
                            store.set_id(id);
                        }
                        /* The text may have moved; keep the ruler only where it fits */
                        app.lines.reset();
                        if let Some(r) = app.ruler {
                            if r.page >= viewer.pages()
                                || r.row >= app.lines.rows(r.page).len()
                            {
                                app.ruler = None;
                            }
                        }
                        result_receiver.clear_priority(1);

                        app.toc = Toc::new(outline);
                        if app.mode == Mode::Toc {
                            reveal_section(&mut app, &viewer);
                        }

                        /* A reload drops the highlights; search the new version again */
                        if let Some(search) = app.search.as_mut() {
                            search.pending = true;
                            search.follow = false;
                            renderer
                                .send_and_confirm_action(RendererAction::Search(
                                    search.needle.clone(),
                                ))
                                .expect("Could not send action to renderer");
                        }
                    }
                    RendererResult::Image { page, data } => {
                        viewer.handle_image(page, data);
                    }
                    RendererResult::SearchResults { needle, matches } => {
                        if let Some(search) =
                            app.search.as_mut().filter(|s| s.needle == needle)
                        {
                            search.set_results(matches, viewer.page_view());
                            let follow = search.follow;
                            show_current_match(&mut app, &mut viewer, &renderer, follow);
                        }
                    }
                }
            }
            2 => {
                file_reload
                    .try_recv()
                    .expect("Could not receive file reload");
                clear_channel!(file_reload);
                renderer
                    .send_and_confirm_action(RendererAction::Load)
                    .expect("Cannot send action to renderer thread");
            }
            3 => {
                sender_rerender
                    .try_recv()
                    .expect("Could not receive rerender");
            }
            4 => {
                let input = event_inputs.0.try_recv().expect("Could not receive input");
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
            5 => {
                current_mouse =
                    event_inputs.1.try_recv().expect("Could not receive mouse");
            }
            6 => {
                event_inputs
                    .3
                    .try_recv()
                    .expect("Could not receive from win-size");
                clear_channel!(event_inputs.3);

                /* The resize event only carries cells; ask again for pixels as well */
                if let Ok(size) = window_size() {
                    *TERMINAL_SIZE
                        .get()
                        .unwrap()
                        .write()
                        .expect("Could not get win size handle") = size;
                }
                viewer.bound_viewer();
                app.toc.scroll_by(0, toc_visible(&viewport()));
                if let Mode::Help { scroll } = &mut app.mode {
                    let max = help_max_scroll(app.help.len(), help_visible(&viewport()));
                    *scroll = (*scroll).min(max);
                }
            }
            7 => {
                save_state(&mut app);
                continue 'main;
            }
            _ => unreachable!(),
        };

        schedule_save(&mut app, &viewer);

        let hovered = app.link.take();
        let clicked = current_mouse.kind.is_down();
        let over_panel = app.mode == Mode::Toc
            && ui::over_toc(&viewport(), current_mouse.column, current_mouse.row);
        if over_panel {
            /* The panel covers the page: links under it can't be hovered or clicked */
            set_pointer(&mut app, PointerShape::Default);
            if clicked {
                toc_click(current_mouse, &mut viewer, &mut app);
                current_mouse.kind = MouseEventKind::Moved;
            }
        } else if let Some(link) = viewer.intersect_link(current_mouse) {
            set_pointer(&mut app, PointerShape::Pointer);

            app.link = viewer.link_hint(&link);
            if clicked {
                /* URI points to page in this document */
                if link.uri.starts_with('#') {
                    if let Some(destination) = &link.dest {
                        app.jumps.record(viewer.spot());
                        viewer.jump_dest(destination);
                    }
                } else {
                    let _ = open::that_detached(link.uri);
                }

                set_pointer(&mut app, PointerShape::Default);

                /* Since the mouse position is saved but this loop runs on other triggers
                 * such as key press, don't allow the mouse to accidentely click on other
                 * links when the viewer is scrolled down by key presses */
                current_mouse.kind = MouseEventKind::Moved;
            }
        } else {
            set_pointer(&mut app, PointerShape::Default);
        }

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

        queue!(io::stdout(), BeginSynchronizedUpdate).expect("Could not begin frame");
        /* Erase from the top-left corner down, never the whole display: in kitty,
         * erasing the whole display (ED 2) also deletes images that have no placement
         * yet, i.e. pages uploaded since the last frame, so they could never be shown */
        queue!(
            io::stdout(),
            ClearImages,
            MoveTo(0, 0),
            Clear(ClearType::FromCursorDown)
        )
        .expect("Could not clear the screen");

        let displayed = if let Mode::Help { scroll } = app.mode {
            ui::draw_help(&app.help, scroll).expect("Could not draw help");
            Vec::new()
        } else {
            viewer
                .display_pages(&renderer)
                .expect("Could not display pages")
        };

        if !matches!(app.mode, Mode::Help { .. }) {
            draw_ruler(&mut app, &viewer);
        }

        let (page, y) = viewer.reading_spot();
        let section = current_section(&app.toc.entries, page, y);
        if app.mode == Mode::Toc {
            draw_toc(&app, &viewer, section);
        }

        ui::draw_frame(&title, &status_segments(&app, &viewer, section))
            .expect("Could not draw frame");
        queue!(io::stdout(), EndSynchronizedUpdate).expect("Could not end the frame");
        io::stdout().flush().expect("Could not flush stdout");

        for page in displayed {
            /* Never wait on the terminal for good: a missing answer must not
             * freeze the viewer */
            let Ok(res) = gr.recv_timeout(RESPONSE_TIMEOUT) else {
                break;
            };
            if res.payload().contains("OK") {
                viewer.placement_ok(page);
                continue;
            }

            if !viewer.schedule_transfer(page) {
                app.message = Some(format!(
                    "kitty rejects page {}: {}",
                    page + 1,
                    res.payload()
                ));
            }
        }
    }

    if app.store.is_some() {
        app.save_due = Some((file_state(&viewer, &app.marks), Instant::now()));
        save_state(&mut app);
    }
    RUNNING.store(false, Ordering::Release);

    /* ========================== Cook the terminal on exit ========================== */
    execute!(io::stdout(), DisableMouseCapturePixels)
        .expect("Could not disable mouse capture");
    execute!(io::stdout(), LeaveAlternateScreen).expect("Could not leave alt mode");
    execute!(io::stdout(), Show).expect("Could not show cursor");
    disable_raw_mode().expect("Could not uncook the terminal");
    drivers::graphics::terminal_graphics_remove_leftovers();
}

/* Opens the remembered state of `file` and goes back to it */
fn open_store(
    file: &str,
    id: Option<String>,
    viewer: &mut Viewer,
    marks: &mut BTreeMap<char, Spot>,
) -> Option<Store> {
    if !CONFIG.get().unwrap().state.enabled {
        return None;
    }
    let mut store = Store::new(Store::default_dir()?, id, Path::new(file));
    if let Some(state) = store.load() {
        let _ = viewer.set_page_offset(state.page_offset);
        viewer.restore(state.page, state.within, state.zoom);
        *marks = state
            .marks
            .iter()
            .filter_map(|(k, spot)| Some((mark_letter(k)?, *spot)))
            .collect();
    }
    Some(store)
}

/* A mark's letter from its name in the state file */
fn mark_letter(name: &str) -> Option<char> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c),
        _ => None,
    }
}

fn file_state(viewer: &Viewer, marks: &BTreeMap<char, Spot>) -> FileState {
    let (page, within, zoom) = viewer.position();
    FileState {
        page,
        within,
        zoom,
        page_offset: viewer.page_offset(),
        marks: marks.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        ..FileState::default()
    }
}

/* Starts the wait before writing whenever the view has moved, so that scrolling
 * writes once, after it stops */
fn schedule_save(app: &mut App, viewer: &Viewer) {
    let Some(store) = &app.store else { return };
    let state = file_state(viewer, &app.marks);
    if !store.is_dirty(&state) {
        app.save_due = None;
    } else if app.save_due.as_ref().is_none_or(|(s, _)| *s != state) {
        app.save_due = Some((state, Instant::now() + SAVE_DELAY));
    }
}

fn save_state(app: &mut App) {
    let (Some(store), Some((state, _))) = (app.store.as_mut(), app.save_due.take())
    else {
        return;
    };
    if let Err(e) = store.save(&state) {
        app.message = Some(format!("Could not remember the position: {}", e));
    }
}

/* The pieces of the bottom border, left to right. `section` is the outline entry
 * being read. */
fn status_segments(app: &App, viewer: &Viewer, section: Option<usize>) -> Vec<Segment> {
    let mut segments = Vec::new();
    match &app.mode {
        Mode::Command(text) => segments.push(ui::mode_segment(&format!(":{}▏", text))),
        Mode::Search(text) => segments.push(ui::mode_segment(&format!("/{}▏", text))),
        Mode::Help { .. } => segments.push(ui::mode_segment("HELP")),
        Mode::Toc => segments.push(ui::mode_segment("TOC")),
        Mode::Normal => segments.push(ui::mode_segment("NOR")),
    }
    if viewer.pages() > 0 {
        segments.push(Segment::new(viewer.page_status(), None));
        segments.push(Segment::new(format!("{}%", viewer.zoom_percent()), None));
    }
    if let Some(count) = app.count {
        segments.push(Segment::new(count.to_string(), None));
    }
    match app.pending_mark {
        Some(ConfigAction::SetMark) => segments.push(Segment::new("m…", None)),
        Some(_) => segments.push(Segment::new("`…", None)),
        None => (),
    }
    if let Some(search) = &app.search {
        segments.push(ui::match_segment(search.status()));
    }
    if let Some(message) = &app.message {
        segments.push(Segment::new(message.clone(), None));
    }
    if let Some(link) = &app.link {
        segments.push(Segment::new(link.clone(), None));
    }
    /* Last, so the border cuts it before anything else */
    if let Some(i) = section {
        let chain = breadcrumb(&app.toc.entries, i);
        let shown = &chain[chain.len().saturating_sub(2)..];
        segments.push(Segment::new(shown.join(" › "), None));
    }
    segments
}

/* Opens the panel's tree down to the section being read and selects it */
fn reveal_section(app: &mut App, viewer: &Viewer) {
    let (page, y) = viewer.reading_spot();
    let section = current_section(&app.toc.entries, page, y);
    app.toc.reveal(section, toc_visible(&viewport()));
}

fn draw_toc(app: &App, viewer: &Viewer, section: Option<usize>) {
    let toc = &app.toc;
    let rows = toc.rows();
    let visible: Vec<TocRow> = rows
        .iter()
        .skip(toc.scroll)
        .take(toc_visible(&viewport()))
        .map(|&i| {
            let entry = &toc.entries[i];
            TocRow {
                depth: entry.depth,
                marker: match (toc.has_children(i), toc.is_open(i)) {
                    (false, _) => ' ',
                    (true, true) => '▾',
                    (true, false) => '▸',
                },
                title: entry.title.clone(),
                label: entry.page.map(|p| viewer.page_label(p)).unwrap_or_default(),
                selected: i == toc.selected,
                current: Some(i) == section,
            }
        })
        .collect();
    let row = rows.iter().position(|&r| r == toc.selected).unwrap_or(0);
    let position = format!("{}/{}", (row + 1).min(rows.len()), rows.len());
    let footer = if toc.editing {
        format!("/{}▏", toc.filter)
    } else if !toc.filter.is_empty() {
        format!("/{} · Esc clears", toc.filter)
    } else {
        "⏎ go · ␣ peek · h/l fold · / find".to_owned()
    };
    ui::draw_toc(&visible, &position, &footer).expect("Could not draw contents");
}

/* Goes to the selected entry of the panel */
fn toc_jump(viewer: &mut Viewer, app: &mut App) {
    let Some(entry) = app.toc.entries.get(app.toc.selected) else {
        return;
    };
    match entry.page {
        Some(page) => {
            app.jumps.record(viewer.spot());
            viewer.jump_target(page, entry.top);
        }
        None => app.message = Some("This entry points outside the document".to_owned()),
    }
}

/* A click on the panel: on a fold marker it folds, elsewhere on an entry it goes there */
fn toc_click(mouse: MouseEvent, viewer: &mut Viewer, app: &mut App) {
    let vp = viewport();
    let Some((row, col)) = ui::toc_hit(&vp, mouse.column, mouse.row) else {
        return;
    };
    let Some(&i) = app.toc.rows().get(app.toc.scroll + row) else {
        return;
    };
    let marker = ui::toc_marker_col(app.toc.entries[i].depth);
    app.toc.selected = i;
    if app.toc.has_children(i) && (col == marker || col == marker + 1) {
        app.toc.toggle(i, toc_visible(&vp));
    } else {
        toc_jump(viewer, app);
        app.mode = Mode::Normal;
    }
}

/* Sends the pointer shape when it changes */
fn set_pointer(app: &mut App, shape: PointerShape) {
    if app.pointer != shape {
        app.pointer = shape;
        execute!(io::stdout(), SetPointerShape(shape))
            .expect("Could not set pointer shape");
    }
}

/* Tells the renderer which match to draw in the stronger color, and with `scroll`
 * goes to it */
fn show_current_match(
    app: &mut App,
    viewer: &mut Viewer,
    renderer: &Renderer,
    scroll: bool,
) {
    let Some(search) = &app.search else { return };
    renderer
        .send_and_confirm_action(RendererAction::SetCurrentMatch(search.current))
        .expect("Could not send action to renderer");
    if let Some(m) = search.current_match().filter(|_| scroll) {
        app.jumps.record(viewer.spot());
        viewer.jump_to(m.page, m.y, 0.25);
    }
    viewer.invalidate_registry();
}

fn handle_mouse_scroll(kind: MouseEventKind, viewer: &mut Viewer, app: &mut App) -> bool {
    let config = CONFIG.get().unwrap();

    if let Mode::Help { scroll } = &mut app.mode {
        let max = help_max_scroll(app.help.len(), help_visible(&viewport()));
        match kind {
            MouseEventKind::ScrollDown => *scroll = (*scroll + 1).min(max),
            MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(1),
            _ => (),
        }
        return false;
    }
    if app.mode == Mode::Toc {
        let visible = toc_visible(&viewport());
        match kind {
            MouseEventKind::ScrollDown => app.toc.scroll_by(3, visible),
            MouseEventKind::ScrollUp => app.toc.scroll_by(-3, visible),
            _ => (),
        }
        return false;
    }

    let inverse_factor = if config.viewer.inverse_scroll {
        1.0
    } else {
        -1.0
    };

    match kind {
        MouseEventKind::ScrollUp => {
            viewer.scroll((0.0f32, inverse_factor * config.viewer.scroll_speed));
            false
        }
        MouseEventKind::ScrollDown => {
            viewer.scroll((0.0f32, -inverse_factor * config.viewer.scroll_speed));
            false
        }
        MouseEventKind::ScrollLeft => {
            viewer.scroll((-config.viewer.scroll_speed, 0.0f32));
            false
        }
        MouseEventKind::ScrollRight => {
            viewer.scroll((config.viewer.scroll_speed, 0.0f32));
            false
        }
        _ => false,
    }
}

/* `true` indicates that the caller should exit *safely* the current process */
fn handle_key(
    key: KeyEvent,
    key_matcher: &mut Keybinds<ConfigAction>,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> bool {
    match &mut app.mode {
        Mode::Help { scroll } => {
            let visible = help_visible(&viewport());
            let max = help_max_scroll(app.help.len(), visible);
            let half = (visible / 2).max(1);
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => *scroll = (*scroll + 1).min(max),
                KeyCode::Char('k') | KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Char('d') | KeyCode::Char('f') | KeyCode::PageDown => {
                    *scroll = (*scroll + half).min(max)
                }
                KeyCode::Char('u') | KeyCode::Char('b') | KeyCode::PageUp => {
                    *scroll = scroll.saturating_sub(half)
                }
                KeyCode::Char('g') | KeyCode::Home => *scroll = 0,
                KeyCode::Char('G') | KeyCode::End => *scroll = max,
                KeyCode::Char('q') | KeyCode::Char('?') | KeyCode::Esc => {
                    app.mode = Mode::Normal
                }
                _ => (),
            }
            return false;
        }
        Mode::Command(text) => {
            match edit_prompt(text, key) {
                PromptKey::Edited => (),
                PromptKey::Cancel => app.mode = Mode::Normal,
                PromptKey::Submit => {
                    let command = parse_command(text);
                    app.mode = Mode::Normal;
                    return run_command(command, viewer, renderer, app);
                }
            }
            return false;
        }
        Mode::Search(text) => {
            match edit_prompt(text, key) {
                PromptKey::Edited => (),
                PromptKey::Cancel => app.mode = Mode::Normal,
                PromptKey::Submit => {
                    let needle = std::mem::take(text);
                    app.mode = Mode::Normal;
                    start_search(needle, renderer, app);
                }
            }
            return false;
        }
        Mode::Toc => {
            app.message = None;
            match app.toc.key(key, toc_visible(&viewport())) {
                TocKey::Handled => (),
                TocKey::Close => app.mode = Mode::Normal,
                TocKey::Jump { close } => {
                    toc_jump(viewer, app);
                    if close {
                        app.mode = Mode::Normal;
                    }
                }
                /* The key that opens the panel closes it too, whatever it's bound to */
                TocKey::Other => {
                    if key_matcher.dispatch(KeyInput::from(key))
                        == Some(&ConfigAction::ShowOutline)
                    {
                        app.mode = Mode::Normal;
                    }
                }
            }
            return false;
        }
        Mode::Normal => (),
    }

    app.message = None;

    if let Some(kind) = app.pending_mark.take() {
        app.count = None;
        if let KeyCode::Char(c) = key.code {
            use_mark(kind, c, viewer, app);
        }
        return false;
    }

    if !key_matcher.is_ongoing() {
        if let Some(count) = count_digit(app.count, &key) {
            app.count = Some(count);
            return false;
        }
    }

    let Some(action) = key_matcher.dispatch(KeyInput::from(key)).copied() else {
        if !key_matcher.is_ongoing() {
            app.count = None;
        }
        return false;
    };
    let count = app.count.take();
    let times = count.unwrap_or(1);
    let config = CONFIG.get().unwrap();
    let speed = config.viewer.scroll_speed * times as f32;

    match action {
        ConfigAction::MoveUp | ConfigAction::MoveDown if app.ruler.is_some() => {
            let delta = if action == ConfigAction::MoveDown {
                times as isize
            } else {
                -(times as isize)
            };
            move_ruler(delta, viewer, app);
        }
        ConfigAction::ToggleRuler => {
            if app.ruler.take().is_none() {
                let (page, y) = viewer.top_of_view();
                let pages = viewer.pages();
                let lines = &mut app.lines;
                app.ruler = ruler::first_from(page, y, pages, |p| lines.rows(p).to_vec());
                match app.ruler {
                    Some(_) => keep_ruler_in_view(viewer, app),
                    None => app.message = Some("No text to follow from here".to_owned()),
                }
            }
        }
        ConfigAction::MoveUp => viewer.scroll((0.0f32, -speed)),
        ConfigAction::MoveDown => viewer.scroll((0.0f32, speed)),
        ConfigAction::MoveLeft => viewer.scroll((-speed, 0.0f32)),
        ConfigAction::MoveRight => viewer.scroll((speed, 0.0f32)),
        ConfigAction::PrevPage => {
            let _ = viewer.jump(viewer.page_first().saturating_sub(times));
        }
        ConfigAction::NextPage => {
            let last_page = viewer.pages().saturating_sub(1);
            let _ = viewer.jump((viewer.page_first() + times).min(last_page));
        }
        ConfigAction::CenterViewer => viewer.center_viewer(),
        ConfigAction::JumpFirstPage | ConfigAction::JumpLastPage => {
            let last_page = viewer.pages().saturating_sub(1);
            let page = match (count, action) {
                (Some(page), _) => page.saturating_sub(1).min(last_page),
                (None, ConfigAction::JumpFirstPage) => 0,
                (None, _) => last_page,
            };
            app.jumps.record(viewer.spot());
            let _ = viewer.jump(page);
        }
        ConfigAction::Quit => return true,
        ConfigAction::ToggleAlpha => {
            if app.throttle.alpha.elapsed().unwrap() < Duration::from_millis(500) {
                return false;
            }

            app.throttle.alpha = SystemTime::now();

            renderer
                .send_and_confirm_action(RendererAction::ToggleAlpha)
                .expect("Could not send action to renderer");
            viewer.invalidate_registry();
        }
        ConfigAction::ToggleInverse => {
            if app.throttle.inverse.elapsed().unwrap() < Duration::from_millis(500) {
                return false;
            }

            app.throttle.inverse = SystemTime::now();
            renderer
                .send_and_confirm_action(RendererAction::ToggleInverse)
                .expect("Could not send action to renderer");
            viewer.invalidate_registry();
        }
        ConfigAction::HalfPageDown => viewer.scroll_screens(0.5 * times as f32),
        ConfigAction::HalfPageUp => viewer.scroll_screens(-0.5 * times as f32),
        ConfigAction::FitWidth => viewer.set_zoom(1.0),
        ConfigAction::FitPage => viewer.fit_page(),
        ConfigAction::ZoomIn => viewer.scale(config.viewer.scale_amount * times as f32),
        ConfigAction::ZoomOut => viewer.scale(-config.viewer.scale_amount * times as f32),
        ConfigAction::ShowHelp => {
            key_matcher.reset();
            app.mode = Mode::Help { scroll: 0 };
        }
        ConfigAction::CommandLine => {
            key_matcher.reset();
            app.mode = Mode::Command(String::new());
        }
        ConfigAction::Search => {
            key_matcher.reset();
            app.mode = Mode::Search(String::new());
        }
        ConfigAction::ClearSearch => {
            run_command(Command::ClearSearch, viewer, renderer, app);
        }
        ConfigAction::JumpBack | ConfigAction::JumpForward => {
            for _ in 0..times {
                let to = if action == ConfigAction::JumpBack {
                    app.jumps.back(viewer.spot())
                } else {
                    app.jumps.forward()
                };
                match to {
                    Some(spot) => viewer.go_to_spot(spot),
                    None => {
                        app.message = Some("No further jumps".to_owned());
                        break;
                    }
                }
            }
        }
        ConfigAction::SetMark | ConfigAction::GoToMark => {
            key_matcher.reset();
            app.pending_mark = Some(action);
        }
        ConfigAction::ShowOutline => {
            key_matcher.reset();
            if app.toc.entries.is_empty() {
                app.message = Some("This document has no table of contents".to_owned());
            } else {
                reveal_section(app, viewer);
                app.mode = Mode::Toc;
            }
        }
        ConfigAction::NextMatch | ConfigAction::PrevMatch => {
            let steps = if action == ConfigAction::NextMatch {
                times as isize
            } else {
                -(times as isize)
            };
            match app.search.as_mut() {
                Some(search) if !search.matches.is_empty() => {
                    search.step(steps);
                    show_current_match(app, viewer, renderer, true);
                }
                Some(search) => {
                    app.message = Some(format!("No match for {}", search.needle))
                }
                None => app.message = Some("No search yet: press /".to_owned()),
            }
        }
    }

    false
}

/* The ruler's band in viewport pixels, when its page is on screen */
fn ruler_band(app: &mut App, viewer: &Viewer) -> Option<(f64, f64, f64, f64)> {
    /* A little air above and below the letters */
    const PAD: f64 = 2.0;
    let r = app.ruler?;
    let rect = viewer.page_rect(r.page)?;
    let row = *app.lines.rows(r.page).get(r.row)?;
    let scale = viewer.pixels_per_unit() as f64;
    Some((
        rect.x as f64,
        rect.y as f64 + row.top as f64 * scale - PAD,
        rect.width as f64,
        (row.bottom - row.top) as f64 * scale + 2.0 * PAD,
    ))
}

/* Scrolls when the ruler's line is off screen or near an edge */
fn keep_ruler_in_view(viewer: &mut Viewer, app: &mut App) {
    let Some(r) = app.ruler else { return };
    let height = viewport().height() as f64;
    let shown = ruler_band(app, viewer)
        .is_some_and(|(_, y, _, h)| y >= height * 0.05 && y + h <= height * 0.85);
    if !shown {
        if let Some(row) = app.lines.rows(r.page).get(r.row).copied() {
            viewer.jump_to(r.page, row.top, 0.3);
        }
    }
}

fn move_ruler(delta: isize, viewer: &mut Viewer, app: &mut App) {
    let Some(from) = app.ruler else { return };
    let pages = viewer.pages();
    /* Scrolled away from the ruler: pick it up at the top of the view */
    let from = if ruler_band(app, viewer).is_some() {
        Some(from)
    } else {
        let (page, y) = viewer.top_of_view();
        let lines = &mut app.lines;
        ruler::first_from(page, y, pages, |p| lines.rows(p).to_vec())
    };
    let lines = &mut app.lines;
    match from.and_then(|f| ruler::step(f, delta, pages, |p| lines.rows(p).len())) {
        Some(to) => app.ruler = Some(to),
        None => {
            app.message = Some(
                if delta > 0 {
                    "End of the text"
                } else {
                    "Start of the text"
                }
                .to_owned(),
            )
        }
    }
    keep_ruler_in_view(viewer, app);
}

/* Lays the ruler over its line; the image is sent again only when it changes */
fn draw_ruler(app: &mut App, viewer: &Viewer) {
    const COLOR: [u8; 4] = [90, 160, 255, 70];
    let Some(placement) =
        ruler_band(app, viewer).and_then(|b| ruler_placement(b, &viewport()))
    else {
        return;
    };
    if app.ruler_image.as_ref() != Some(&placement) {
        let data = ruler_bitmap(&placement, COLOR);
        if drivers::graphics::terminal_graphics_transfer_bitmap(
            drivers::graphics::RULER_ID,
            placement.width,
            placement.height,
            &data,
            true,
        )
        .is_err()
        {
            return;
        }
        app.ruler_image = Some(placement.clone());
    }
    let _ = drivers::graphics::terminal_graphics_place_ruler(
        placement.col,
        placement.row,
        placement.cols,
        placement.rows,
    );
}

/* Sets mark `c`, or goes to it; going to the backtick or quote mark goes back to
 * where the last jump started, as in vim */
fn use_mark(kind: ConfigAction, c: char, viewer: &mut Viewer, app: &mut App) {
    if kind == ConfigAction::SetMark {
        if c.is_ascii_lowercase() {
            app.marks.insert(c, viewer.spot());
            app.message = Some(format!("Mark {} set", c));
        } else {
            app.message = Some("Marks are the letters a to z".to_owned());
        }
        return;
    }
    if c == '`' || c == '\'' {
        match app.jumps.back(viewer.spot()) {
            Some(spot) => viewer.go_to_spot(spot),
            None => app.message = Some("No jump to go back to".to_owned()),
        }
        return;
    }
    match app.marks.get(&c).copied() {
        Some(spot) => {
            app.jumps.record(viewer.spot());
            viewer.go_to_spot(spot);
        }
        None => app.message = Some(format!("No mark {}", c)),
    }
}

fn start_search(needle: String, renderer: &Renderer, app: &mut App) {
    if needle.is_empty() {
        return;
    }
    app.search = Some(SearchState::new(needle.clone()));
    renderer
        .send_and_confirm_action(RendererAction::Search(needle))
        .expect("Could not send action to renderer");
}

fn run_command(
    command: Command,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> bool {
    match command {
        Command::Empty => (),
        Command::Quit => return true,
        Command::Page(page) => {
            app.jumps.record(viewer.spot());
            if viewer.jump(page - 1).is_err() {
                app.message = Some(format!("Only {} pages", viewer.pages()));
            }
        }
        Command::Goto(label) => {
            if let Some(page) = viewer.find_label(&label) {
                app.jumps.record(viewer.spot());
                let _ = viewer.jump(page);
            } else {
                match label.parse::<usize>() {
                    /* No page is printed with that number; take it as a position */
                    Ok(page) if page >= 1 && page <= viewer.pages() => {
                        app.jumps.record(viewer.spot());
                        let _ = viewer.jump(page - 1);
                        app.message = Some(format!(
                            "No printed page {}, went to page #{}",
                            label, page
                        ));
                    }
                    _ => app.message = Some(format!("No page or command: {}", label)),
                }
            }
        }
        Command::ClearSearch => {
            if app.search.take().is_some() {
                renderer
                    .send_and_confirm_action(RendererAction::ClearSearch)
                    .expect("Could not send action to renderer");
                viewer.invalidate_registry();
            }
        }
        Command::Marks => {
            app.message = Some(if app.marks.is_empty() {
                "No marks: m then a letter sets one".to_owned()
            } else {
                app.marks
                    .iter()
                    .map(|(c, spot)| format!("{} {}", c, viewer.page_label(spot.page)))
                    .collect::<Vec<_>>()
                    .join(" · ")
            })
        }
        Command::DeleteMarks(None) => app.marks.clear(),
        Command::DeleteMarks(Some(letters)) => {
            for c in letters.chars() {
                app.marks.remove(&c);
            }
        }
        Command::Offset(None) => {
            app.message = Some(match viewer.page_offset() {
                0 => "No page offset: :offset 12 makes the 13th page printed page 1"
                    .to_owned(),
                n => format!("Printed page 1 is page #{}", n + 1),
            })
        }
        Command::Offset(Some(offset)) => {
            if let Err(e) = viewer.set_page_offset(offset) {
                app.message = Some(e);
            }
        }
        Command::Unknown(text) => {
            app.message = Some(format!("Unknown command: {}", text))
        }
    }
    false
}
