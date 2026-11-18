/* What the main loop keeps besides the viewer, and the remembered position */

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::config::*;
use crate::drivers::commands::PointerShape;
use crate::globals::*;
use crate::image::*;
use crate::input::{reveal_section, show_current_match};
use crate::jumps::{Jumps, Spot};
use crate::outline::Toc;
use crate::ruler::{Lines, Ruler};
use crate::state::*;
use crate::store::{FileState, Store};
use crate::threads::renderer::{Renderer, RendererAction, RendererResult};
use crate::viewer::*;

/* How long the view must stay put before its position is written */
pub const SAVE_DELAY: Duration = Duration::from_secs(2);

/* When the slow toggles last ran, so that holding their key can't flood the renderer */
#[derive(Default)]
pub struct LastExecuted {
    pub alpha: Option<Instant>,
    pub inverse: Option<Instant>,
}

/* Everything the main loop keeps besides the viewer itself */
pub struct App {
    pub mode: Mode,
    pub count: Option<usize>,
    pub search: Option<SearchState>,
    /* One-off status text, cleared by the next key */
    pub message: Option<String>,
    /* Where the hovered link points */
    pub link: Option<String>,
    pub help: Vec<String>,
    /* The contents panel, kept between openings */
    pub toc: Toc,
    /* Where this file's state is remembered; `None` when that's turned off */
    pub store: Option<Store>,
    /* Where jumps started, for Ctrl-o and Ctrl-n */
    pub jumps: Jumps,
    pub marks: BTreeMap<char, Spot>,
    /* `m` or a backtick was typed and waits for its letter */
    pub pending_mark: Option<ConfigAction>,
    /* The reading ruler when it's on, the text rows it steps through, and the image
     * last sent for it */
    pub ruler: Option<Ruler>,
    pub lines: Lines,
    pub ruler_image: Option<RulerPlacement>,
    /* The mouse pointer shape last sent to the terminal */
    pub pointer: PointerShape,
    /* The position waiting to be written, and when */
    pub save_due: Option<(FileState, Instant)>,
    pub throttle: LastExecuted,
    /* Why the viewer has to stop: the terminal or a helper thread is gone */
    pub fatal: Option<String>,
}

/* Sends `action` to the renderer; a renderer that stopped ends the viewer */
pub fn tell_renderer(renderer: &Renderer, app: &mut App, action: RendererAction) {
    if let Err(e) = renderer.send_and_confirm_action(action) {
        app.fatal.get_or_insert(e);
    }
}

/* Whether a slow toggle last ran long enough ago to run again; if so, marks it as run */
pub fn throttle(last: &mut Option<Instant>) -> bool {
    if last.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
        return false;
    }
    *last = Some(Instant::now());
    true
}

/* Opens the remembered state of `file` and goes back to it */
pub fn open_store(
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
pub fn mark_letter(name: &str) -> Option<char> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c),
        _ => None,
    }
}

pub fn file_state(viewer: &Viewer, marks: &BTreeMap<char, Spot>) -> FileState {
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
pub fn schedule_save(app: &mut App, viewer: &Viewer) {
    let Some(store) = &app.store else { return };
    let state = file_state(viewer, &app.marks);
    if !store.is_dirty(&state) {
        app.save_due = None;
    } else if app.save_due.as_ref().is_none_or(|(s, _)| *s != state) {
        app.save_due = Some((state, Instant::now() + SAVE_DELAY));
    }
}

pub fn save_state(app: &mut App) {
    let (Some(store), Some((state, _))) = (app.store.as_mut(), app.save_due.take())
    else {
        return;
    };
    if let Err(e) = store.save(&state) {
        app.message = Some(format!("Could not remember the position: {}", e));
    }
}

/* The ruler's band in viewport pixels, when its page is on screen */
pub fn ruler_band(app: &mut App, viewer: &Viewer) -> Option<(f64, f64, f64, f64)> {
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

/* Takes in what the renderer sends: a (re)loaded document, a page image or search
 * results. `true` means the page images still queued are for an older version and
 * should be dropped. */
pub fn handle_result(
    result: RendererResult,
    file: &str,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> bool {
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
                app.store = open_store(file, id, viewer, &mut app.marks);
            } else if let Some(store) = app.store.as_mut() {
                store.set_id(id);
            }
            /* The text may have moved; keep the ruler only where it fits */
            app.lines.reset();
            if let Some(r) = app.ruler {
                if r.page >= viewer.pages() || r.row >= app.lines.rows(r.page).len() {
                    app.ruler = None;
                }
            }

            app.toc = Toc::new(outline);
            if app.mode == Mode::Toc {
                reveal_section(app, viewer);
            }

            /* A reload drops the highlights; search the new version again */
            if let Some(search) = app.search.as_mut() {
                search.pending = true;
                search.follow = false;
                let action = RendererAction::Search(search.needle.clone());
                tell_renderer(renderer, app, action);
            }
            true
        }
        RendererResult::Image { page, data } => {
            viewer.handle_image(page, data);
            false
        }
        RendererResult::SearchResults { needle, matches } => {
            if let Some(search) = app.search.as_mut().filter(|s| s.needle == needle) {
                search.set_results(matches, viewer.page_view());
                let follow = search.follow;
                show_current_match(app, viewer, renderer, follow);
            }
            false
        }
    }
}
