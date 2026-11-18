/* What keys, the mouse wheel and `:` commands do */

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use keybinds::{KeyInput, Keybinds};

use crate::app::*;
use crate::config::*;
use crate::drivers::commands::PointerShape;
use crate::frame::set_pointer;
use crate::globals::*;
use crate::outline::{current_section, TocKey};
use crate::ruler;
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
            if !throttle(&mut app.throttle.alpha) {
                return false;
            }
            tell_renderer(renderer, app, RendererAction::ToggleAlpha);
            viewer.invalidate_registry();
        }
        ConfigAction::ToggleInverse => {
            if !throttle(&mut app.throttle.inverse) {
                return false;
            }
            tell_renderer(renderer, app, RendererAction::ToggleInverse);
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

pub fn handle_mouse_scroll(
    kind: MouseEventKind,
    viewer: &mut Viewer,
    app: &mut App,
) -> bool {
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
                tell_renderer(renderer, app, RendererAction::ClearSearch);
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
        app.jumps.record(viewer.spot());
        viewer.jump_to(m.page, m.y, 0.25);
    }
    viewer.invalidate_registry();
}

/* Sets mark `c`, or goes to it; going to the backtick or quote mark goes back to
 * where the last jump started, as in vim */
pub fn use_mark(kind: ConfigAction, c: char, viewer: &mut Viewer, app: &mut App) {
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
        Some(page) => {
            app.jumps.record(viewer.spot());
            viewer.jump_target(page, entry.top);
        }
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

/* Scrolls when the ruler's line is off screen or near an edge */
pub fn keep_ruler_in_view(viewer: &mut Viewer, app: &mut App) {
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

pub fn move_ruler(delta: isize, viewer: &mut Viewer, app: &mut App) {
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

/* Hovering and clicking: the pointer shape, the link hint, following links, and clicks
 * in the contents panel. Returns whether the mouse clicked. */
pub fn handle_pointer(
    mouse: &mut MouseEvent,
    viewer: &mut Viewer,
    app: &mut App,
) -> bool {
    let clicked = mouse.kind.is_down();
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
                    app.jumps.record(viewer.spot());
                    viewer.jump_dest(destination);
                }
            } else {
                let _ = open::that_detached(link.uri);
            }

            set_pointer(app, PointerShape::Default);

            /* Since the mouse position is saved but this loop runs on other triggers
             * such as key press, don't allow the mouse to accidentely click on other
             * links when the viewer is scrolled down by key presses */
            mouse.kind = MouseEventKind::Moved;
        }
    } else {
        set_pointer(app, PointerShape::Default);
    }
    clicked
}
