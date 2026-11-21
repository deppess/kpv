/* What keys, the mouse wheel and `:` commands do */

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use keybinds::{KeyInput, Keybinds};

use crate::app::*;
use crate::config::*;
use crate::drivers::commands::PointerShape;
use crate::frame::set_pointer;
use crate::globals::*;
use crate::outline::{current_section, TocKey};
use crate::state::*;
use crate::threads::renderer::{Renderer, RendererAction};
use crate::ui::{self, help_max_scroll, help_visible, toc_visible, viewport};
use crate::viewer::*;

/* `true` indicates that the caller should exit *safely* the current process */
pub fn handle_key(
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
                TocKey::Other => match key_matcher.dispatch(KeyInput::from(key)).copied()
                {
                    Some(ConfigAction::ShowOutline) => app.mode = Mode::Normal,
                    /* The zoom keys make the panel wider and narrower */
                    Some(ConfigAction::ZoomIn) => resize_toc(5, app),
                    Some(ConfigAction::ZoomOut) => resize_toc(-5, app),
                    _ => (),
                },
            }
            return false;
        }
        Mode::Normal => (),
    }

    app.message = None;

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
    let speed = config.viewer.scroll_step * times as f32;

    match action {
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
            let _ = viewer.jump(page);
        }
        ConfigAction::Quit => return true,
        ConfigAction::ToggleInverse => {
            if !throttle(&mut app.inverse_toggled) {
                return false;
            }
            tell_renderer(renderer, app, RendererAction::ToggleInverse);
            viewer.invalidate_registry();
        }
        ConfigAction::HalfPageDown => viewer.scroll_screens(0.5 * times as f32),
        ConfigAction::HalfPageUp => viewer.scroll_screens(-0.5 * times as f32),
        ConfigAction::FitWidth => viewer.set_zoom(1.0),
        ConfigAction::FitPage => viewer.fit_page(),
        ConfigAction::ZoomIn => viewer.zoom_by(config.viewer.zoom_step * times as f32),
        ConfigAction::ZoomOut => viewer.zoom_by(-config.viewer.zoom_step * times as f32),
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

/* The wheel scrolls the key list or the contents panel while open, the pages
 * otherwise */
pub fn handle_mouse_scroll(kind: MouseEventKind, viewer: &mut Viewer, app: &mut App) {
    let config = CONFIG.get().unwrap();
    if let Mode::Help { scroll } = &mut app.mode {
        let max = help_max_scroll(app.help.len(), help_visible(&viewport()));
        match kind {
            MouseEventKind::ScrollDown => *scroll = (*scroll + 1).min(max),
            MouseEventKind::ScrollUp => *scroll = scroll.saturating_sub(1),
            _ => (),
        }
        return;
    }
    if app.mode == Mode::Toc {
        let visible = toc_visible(&viewport());
        match kind {
            MouseEventKind::ScrollDown => app.toc.scroll_by(3, visible),
            MouseEventKind::ScrollUp => app.toc.scroll_by(-3, visible),
            _ => (),
        }
        return;
    }

    let inverse_factor = if config.viewer.invert_wheel {
        1.0
    } else {
        -1.0
    };

    let step = config.viewer.scroll_step;
    match kind {
        MouseEventKind::ScrollUp => viewer.scroll((0.0, inverse_factor * step)),
        MouseEventKind::ScrollDown => viewer.scroll((0.0, -inverse_factor * step)),
        MouseEventKind::ScrollLeft => viewer.scroll((-step, 0.0)),
        MouseEventKind::ScrollRight => viewer.scroll((step, 0.0)),
        _ => (),
    }
}

pub fn run_command(
    command: Command,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> bool {
    match command {
        Command::Empty => (),
        Command::Quit => return true,
        Command::Page(page) => {
            if viewer.jump(page - 1).is_err() {
                app.message = Some(format!("Only {} pages", viewer.pages()));
            }
        }
        Command::ClearSearch => {
            if app.search.take().is_some() {
                tell_renderer(renderer, app, RendererAction::ClearSearch);
                viewer.invalidate_registry();
            }
        }
        Command::Unknown(text) => {
            app.message = Some(format!("Unknown command: {}", text))
        }
    }
    false
}

pub fn start_search(needle: String, renderer: &Renderer, app: &mut App) {
    if needle.is_empty() {
        return;
    }
    app.search = Some(SearchState::new(needle.clone()));
    tell_renderer(renderer, app, RendererAction::Search(needle));
}

/* Tells the renderer which match to draw in the stronger color, and with `scroll`
 * goes to it */
pub fn show_current_match(
    app: &mut App,
    viewer: &mut Viewer,
    renderer: &Renderer,
    scroll: bool,
) {
    let Some(current) = app.search.as_ref().map(|s| s.current) else {
        return;
    };
    tell_renderer(renderer, app, RendererAction::SetCurrentMatch(current));
    let Some(search) = &app.search else { return };
    if let Some(m) = search.current_match().filter(|_| scroll) {
        viewer.jump_to(m.page, m.y, 0.25);
    }
    viewer.invalidate_registry();
}

/* Opens the panel's tree down to the section being read and selects it */
pub fn reveal_section(app: &mut App, viewer: &Viewer) {
    let (page, y) = viewer.reading_spot();
    let section = current_section(&app.toc.entries, page, y);
    app.toc.reveal(section, toc_visible(&viewport()));
}

/* Goes to the selected entry of the panel */
pub fn toc_jump(viewer: &mut Viewer, app: &mut App) {
    let Some(entry) = app.toc.entries.get(app.toc.selected) else {
        return;
    };
    match entry.page {
        Some(page) => viewer.jump_target(page, entry.top),
        None => app.message = Some("This entry points outside the document".to_owned()),
    }
}

/* A click on the panel: on a fold marker it folds, elsewhere on an entry it goes there */
pub fn toc_click(mouse: MouseEvent, viewer: &mut Viewer, app: &mut App) {
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

/* Hovering and clicking: the pointer shape, the link hint, following links, and clicks
 * in the contents panel. Returns whether the mouse clicked. */
pub fn handle_pointer(
    mouse: &mut MouseEvent,
    viewer: &mut Viewer,
    app: &mut App,
) -> bool {
    let clicked = matches!(mouse.kind, MouseEventKind::Down(_));
    let over_panel =
        app.mode == Mode::Toc && ui::over_toc(&viewport(), mouse.column, mouse.row);
    if over_panel {
        /* The panel covers the page: links under it can't be hovered or clicked */
        set_pointer(app, PointerShape::Default);
        if clicked {
            toc_click(*mouse, viewer, app);
            mouse.kind = MouseEventKind::Moved;
        }
    } else if let Some(link) = viewer.intersect_link(*mouse) {
        set_pointer(app, PointerShape::Pointer);

        app.link = viewer.link_hint(&link);
        if clicked {
            /* URI points to page in this document */
            if link.uri.starts_with('#') {
                if let Some(destination) = &link.dest {
                    viewer.jump_dest(destination);
                }
            } else {
                open_link(&link.uri);
            }

            set_pointer(app, PointerShape::Default);

            /* The mouse is kept between passes of the main loop; a click counts once,
             * not again on a link that a key scrolls under it */
            mouse.kind = MouseEventKind::Moved;
        }
    } else {
        set_pointer(app, PointerShape::Default);
    }
    clicked
}

/* Widens or narrows the contents panel by `step` percent of the pages' width, and
 * keeps the width for next time */
fn resize_toc(step: i16, app: &mut App) {
    let percent = (ui::toc_percent() as i16 + step).max(0) as u16;
    ui::set_toc_percent(percent);
    save_panel_sizes();
    app.message = Some(format!("Contents {}% wide", ui::toc_percent()));
}

/* Opens a web or mail link with the system's opener; other kinds, such as `file:` or
 * an app's own scheme, could run programs, so they stay shut */
fn open_link(uri: &str) {
    let scheme = uri.split(':').next().unwrap_or("").to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "mailto") {
        return;
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let child = std::process::Command::new(opener)
        .arg(uri)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    /* Waited for out of the way, so it leaves no zombie */
    if let Ok(mut child) = child {
        std::thread::spawn(move || child.wait());
    }
}
