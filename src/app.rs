/* What the main loop keeps besides the viewer, and the remembered position */

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::*;
use crate::drivers::commands::PointerShape;
use crate::globals::*;
use crate::image::*;
use crate::input::{reveal_section, show_current_match};
use crate::jumps::{Jumps, Spot};
use crate::notes::{EditorSize, Notes};
use crate::outline::Toc;
use crate::ruler::{Lines, Ruler};
use crate::state::*;
use crate::store::{FileState, Layout, Store};
use crate::threads::renderer::{Renderer, RendererAction, RendererResult};
use crate::ui;
use crate::viewer::*;

/* How long the view must stay put before its position is written: soon, so that
 * little is lost if the viewer is killed or crashes */
pub const SAVE_DELAY: Duration = Duration::from_millis(300);

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
    pub help: Vec<ui::HelpLine>,
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
    /* The images placed by the last frame; `None` forces placing them again */
    pub shown: Option<Shown>,
    pub notes: Notes,
    /* The top edge of the frame: the PDF's title and author, or its file name */
    pub title: String,
    /* The message on show and since when, so that it goes after a while */
    pub message_shown: Option<(String, Instant)>,
    /* The page offset was set with `:offset`, so the numbers printed on the pages
     * don't change it */
    pub offset_by_hand: bool,
}

/* How long a message stays in the status bar */
pub const MESSAGE_TIME: Duration = Duration::from_secs(5);

/* When the message on show is due to go; a new message starts its time again */
pub fn message_due(app: &mut App) -> Option<Instant> {
    match (&app.message, &app.message_shown) {
        (None, _) => {
            app.message_shown = None;
            None
        }
        (Some(text), Some((shown, since))) if text == shown => {
            Some(*since + MESSAGE_TIME)
        }
        (Some(text), _) => {
            app.message_shown = Some((text.clone(), Instant::now()));
            Some(Instant::now() + MESSAGE_TIME)
        }
    }
}

/* Whether a title stored in a PDF is a real one, not what an export tool left there:
 * `Microsoft Word - draft.docx`, `untitled`, a file name */
pub fn real_title(title: &str) -> bool {
    let lower = title.trim().to_lowercase();
    let file_like = [
        ".doc", ".docx", ".pdf", ".tex", ".indd", ".dvi", ".ps", ".odt", ".pages",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext));
    !(lower.is_empty()
        || file_like
        || lower.starts_with("microsoft word")
        || matches!(
            lower.as_str(),
            "untitled" | "title" | "document" | "unknown"
        ))
}

/* The frame's title: `Title — Author` when the PDF stores a real title, its file name
 * otherwise */
pub fn frame_title(title: Option<&str>, author: Option<&str>, file: &str) -> String {
    let name = || {
        std::path::Path::new(file)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file.to_owned())
    };
    match title.filter(|t| real_title(t)) {
        Some(title) => match author.map(str::trim).filter(|a| !a.is_empty()) {
            Some(author) => format!("{} — {}", title.trim(), author),
            None => title.trim().to_owned(),
        },
        None => name(),
    }
}

/* Every image placed in a frame. A frame that would place the same ones leaves them
 * alone and redraws only the text. */
#[derive(Clone, Debug, PartialEq)]
pub struct Shown {
    pub pages: Vec<PagePlacement>,
    pub ruler: Option<RulerPlacement>,
    pub tints: Vec<Tint>,
}

/* A one-pixel image `id` stretched over cells under a see-through panel: from a
 * 1-indexed cell, `cols` x `rows` cells, at height `z` */
#[derive(Clone, Debug, PartialEq)]
pub struct Tint {
    pub id: usize,
    pub z: i64,
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
}

impl Shown {
    /* The pages that can be seen, without the ones kept ready out of sight */
    fn in_view(&self) -> Vec<&PagePlacement> {
        self.pages.iter().filter(|p| p.at.is_some()).collect()
    }
}

/* What a frame does to kitty's images to go from showing `old` to `new` */
#[derive(Debug, PartialEq)]
pub struct Changes {
    /* Take every placement down first */
    pub clear: bool,
    /* The page images to place */
    pub pages: Vec<PagePlacement>,
    /* Place or hide the ruler */
    pub ruler: bool,
    /* The tints to place, and the ones to take down */
    pub tints: Vec<Tint>,
    pub hide: Vec<usize>,
}

/* Everything is placed again when what's in view changes. A page kept ready that
 * arrives on its own changes nothing in view and is placed by itself, and the ruler
 * and the tints move by themselves. */
pub fn changes(old: Option<&Shown>, new: &Shown) -> Changes {
    /* Only page images go when everything is placed again; tints that aren't wanted any
     * more are taken down either way */
    let hide = old
        .map(|old| {
            old.tints
                .iter()
                .map(|t| t.id)
                .filter(|id| !new.tints.iter().any(|t| t.id == *id))
                .collect()
        })
        .unwrap_or_default();
    match old {
        Some(old) if old.in_view() == new.in_view() => Changes {
            clear: false,
            pages: new
                .pages
                .iter()
                .filter(|p| !old.pages.contains(p))
                .cloned()
                .collect(),
            ruler: old.ruler != new.ruler,
            tints: new
                .tints
                .iter()
                .filter(|t| !old.tints.contains(t))
                .cloned()
                .collect(),
            hide,
        },
        _ => Changes {
            clear: true,
            pages: new.pages.clone(),
            ruler: true,
            tints: new.tints.clone(),
            hide,
        },
    }
}

/* Sends `action` to the renderer; a renderer that stopped ends the viewer */
pub fn tell_renderer(renderer: &Renderer, app: &mut App, action: RendererAction) {
    if let Err(e) = renderer.send_action(action) {
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

/* Opens the remembered state of `file` and goes back to it: the place, the marks and
 * the note. A file that moved since gets the link in its note moved along. */
pub fn open_store(file: &str, id: Option<String>, viewer: &mut Viewer, app: &mut App) {
    if !CONFIG.get().unwrap().viewer.remember_position {
        return;
    }
    let Some(dir) = Store::default_dir() else {
        return;
    };
    let mut store = Store::new(dir, id, Path::new(file));
    if let Some(state) = store.load() {
        let _ = viewer.set_page_offset(state.page_offset);
        app.offset_by_hand = state.offset_by_hand;
        viewer.restore(state.page, state.within, state.zoom);
        app.marks = state
            .marks
            .iter()
            .filter_map(|(k, spot)| Some((mark_letter(k)?, *spot)))
            .collect();
        app.notes.remembered = state.note.as_deref().map(PathBuf::from);
        if let (Some(old), Some(note)) = (&state.path, &app.notes.remembered) {
            if old != store.path() {
                move_note_link(note, Path::new(old), Path::new(store.path()));
            }
        }
        restore_layout(&state.layout, viewer, app);
    }
    app.store = Some(store);
}

/* Lays the window out as it was: the note open first, since the zoom is kept relative
 * to the width the pages had beside it */
fn restore_layout(layout: &Layout, viewer: &mut Viewer, app: &mut App) {
    if layout.notes {
        if app.notes.focus(editor_size()).is_ok() && !layout.notes_focused {
            app.notes.unfocus();
        }
        apply_layout(viewer, app);
    }
    viewer.set_left(layout.left);
    if layout.contents {
        app.mode = Mode::Toc;
    }
    app.ruler = layout.ruler.map(|[page, row]| Ruler { page, row });
}

/* The layout to remember */
fn layout(viewer: &Viewer, app: &App) -> Layout {
    Layout {
        notes: app.notes.open,
        notes_focused: app.notes.open && app.notes.focused,
        contents: app.mode == Mode::Toc,
        ruler: app.ruler.map(|r| [r.page, r.row]),
        /* Rounded, so that tiny moves don't count as changes */
        left: ((viewer.left() * 1000.0).round() / 1000.0).abs(),
    }
}

/* Points the note's link to the PDF at its new place */
fn move_note_link(note: &Path, old: &Path, new: &Path) {
    let Ok(text) = std::fs::read_to_string(note) else {
        return;
    };
    if let Some(moved) = crate::notes::note::move_link(&text, old, new) {
        let _ = std::fs::write(note, moved);
    }
}

/* A mark's letter from its name in the state file */
pub fn mark_letter(name: &str) -> Option<char> {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_lowercase() => Some(c),
        _ => None,
    }
}

pub fn file_state(viewer: &Viewer, app: &App) -> FileState {
    let (page, within, zoom) = viewer.position();
    FileState {
        page,
        within,
        zoom,
        page_offset: viewer.page_offset(),
        offset_by_hand: app.offset_by_hand,
        marks: app.marks.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        note: app
            .notes
            .remembered
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        layout: layout(viewer, app),
        ..FileState::default()
    }
}

/* Starts the wait before writing whenever the view has moved, so that scrolling
 * writes once, after it stops */
pub fn schedule_save(app: &mut App, viewer: &Viewer) {
    let Some(store) = &app.store else { return };
    let state = file_state(viewer, app);
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

/* Where the panel sizes are kept between runs */
fn panel_sizes_file() -> Option<PathBuf> {
    Some(Store::default_dir()?.join("panels.toml"))
}

/* Reads the panels' widths from last time; the notes panel starts from the config's
 * width when it was never resized */
pub fn load_panel_sizes() {
    ui::set_notes_percent(CONFIG.get().unwrap().notes.width);
    let Some(text) = panel_sizes_file().and_then(|f| std::fs::read_to_string(f).ok())
    else {
        return;
    };
    let Ok(table) = text.parse::<toml::Table>() else {
        return;
    };
    let width = |key: &str| {
        table
            .get(key)
            .and_then(|v| v.as_integer())
            .map(|w| w.clamp(0, 100) as u16)
    };
    if let Some(width) = width("contents_width") {
        ui::set_toc_percent(width);
    }
    if let Some(width) = width("notes_width") {
        ui::set_notes_percent(width);
    }
}

pub fn save_panel_sizes() {
    let Some(file) = panel_sizes_file() else {
        return;
    };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(
        file,
        format!(
            "contents_width = {}\nnotes_width = {}\n",
            ui::toc_percent(),
            ui::notes_percent()
        ),
    );
}

/* The size of the editor in the notes panel: the panel inside its border */
pub fn editor_size() -> EditorSize {
    let window = ui::window();
    let (_, _, cols, rows) = ui::notes_rect(&window, ui::notes_percent());
    EditorSize {
        cols,
        rows,
        pixels: (
            (cols as f64 * window.pxpercol) as u16,
            (rows as f64 * window.pxperrow) as u16,
        ),
    }
}

/* Lays the pages out beside the notes panel while it's open, and over the whole width
 * when it isn't, keeping the zoom and the place; the editor follows the panel's size */
pub fn apply_layout(viewer: &mut Viewer, app: &mut App) {
    let cols = if app.notes.open {
        ui::notes_rect(&ui::window(), ui::notes_percent()).2
    } else {
        0
    };
    let before = ui::viewport();
    ui::set_notes_cols(cols);
    if ui::viewport() != before {
        viewer.refit();
    }
    let size = editor_size();
    if let Some(terminal) = &mut app.notes.terminal {
        if terminal.size() != (size.cols, size.rows) {
            /* The program draws its images again at the new size */
            crate::notes::send_to_kitty(&crate::notes::graphics::clear(false));
        }
        terminal.resize(size.cols, size.rows, size.pixels);
    }
}

/* Numbers the pages as printed on them, unless the PDF says how it numbers them or the
 * offset was set by hand */
pub fn apply_printed_numbers(offset: Option<i32>, viewer: &mut Viewer, app: &mut App) {
    let Some(offset) = offset else { return };
    if app.offset_by_hand || !viewer.has_plain_labels() || offset == viewer.page_offset()
    {
        return;
    }
    if viewer.set_page_offset(offset).is_ok() && offset > 0 {
        app.message = Some(format!(
            "Numbered as printed: page #{} is page 1",
            offset + 1
        ));
    }
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
            title,
            author,
        } => {
            app.title = frame_title(title.as_deref(), author.as_deref(), file);
            app.notes.title = title.filter(|t| real_title(t));
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
                open_store(file, id, viewer, app);
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
        RendererResult::PrintedNumbers { offset } => {
            apply_printed_numbers(offset, viewer, app);
            false
        }
        RendererResult::SearchProgress {
            generation,
            needle,
            matches,
            searched,
            total,
        } => {
            let Some(search) = app.search.as_mut().filter(|s| s.needle == needle) else {
                return false;
            };
            let pages: Vec<usize> = matches.iter().map(|m| m.page).collect();
            if !search.add(generation, matches) {
                return false;
            }
            search.searched = (searched, total);
            /* Pages with new matches are drawn again with them highlighted */
            viewer.invalidate_pages(&pages);
            if searched >= total {
                search.finish(viewer.page_view());
                let follow = search.follow;
                show_current_match(app, viewer, renderer, follow);
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(page: usize, image: usize, row: Option<usize>) -> PagePlacement {
        PagePlacement {
            page,
            image,
            at: row.map(|row| Placement {
                col: 2,
                row,
                crop: (0, 0, 100, 100),
                cols: 10,
                rows: 5,
            }),
            under: false,
        }
    }

    fn ruler(row: usize) -> RulerPlacement {
        RulerPlacement {
            col: 2,
            row,
            cols: 10,
            rows: 1,
            width: 100,
            height: 20,
            band: (0, 2, 100, 16),
        }
    }

    fn shown(pages: Vec<PagePlacement>, ruler: Option<RulerPlacement>) -> Shown {
        Shown {
            pages,
            ruler,
            tints: Vec::new(),
        }
    }

    fn tint(id: usize, row: usize) -> Tint {
        Tint {
            id,
            z: -1,
            col: 2,
            row,
            cols: 30,
            rows: 1,
        }
    }

    #[test]
    fn the_frame_shows_the_title_the_pdf_stores() {
        assert_eq!(
            frame_title(
                Some("The Art of Mac Malware"),
                Some("Patrick Wardle"),
                "/a/x.pdf"
            ),
            "The Art of Mac Malware — Patrick Wardle"
        );
        assert_eq!(frame_title(Some("Notes"), None, "/a/x.pdf"), "Notes");
        assert_eq!(
            frame_title(Some("Microsoft Word - draft3.docx"), Some("me"), "/a/x.pdf"),
            "x.pdf"
        );
        assert_eq!(frame_title(Some("untitled"), None, "/a/x.pdf"), "x.pdf");
        assert_eq!(frame_title(None, Some("me"), "/a/x.pdf"), "x.pdf");
    }

    #[test]
    fn the_first_frame_places_everything() {
        let new = shown(vec![page(0, 1, Some(2)), page(1, 2, None)], Some(ruler(3)));
        let c = changes(None, &new);
        assert!(c.clear && c.ruler);
        assert_eq!(c.pages, new.pages);
    }

    #[test]
    fn the_same_view_places_nothing() {
        let old = shown(vec![page(0, 1, Some(2)), page(1, 2, None)], Some(ruler(3)));
        let c = changes(Some(&old), &old.clone());
        assert_eq!(
            c,
            Changes {
                clear: false,
                pages: vec![],
                ruler: false,
                tints: vec![],
                hide: vec![],
            }
        );
    }

    #[test]
    fn a_page_kept_ready_is_placed_by_itself() {
        let old = shown(vec![page(0, 1, Some(2))], None);
        let new = shown(vec![page(0, 1, Some(2)), page(1, 2, None)], None);
        let c = changes(Some(&old), &new);
        assert!(!c.clear && !c.ruler);
        assert_eq!(c.pages, vec![page(1, 2, None)]);
    }

    #[test]
    fn moving_the_ruler_leaves_the_pages() {
        let old = shown(vec![page(0, 1, Some(2))], Some(ruler(3)));
        let new = shown(vec![page(0, 1, Some(2))], Some(ruler(4)));
        let c = changes(Some(&old), &new);
        assert!(!c.clear && c.ruler);
        assert!(c.pages.is_empty());
        let off = changes(Some(&old), &shown(vec![page(0, 1, Some(2))], None));
        assert!(!off.clear && off.ruler);
    }

    #[test]
    fn scrolling_or_a_new_image_in_view_places_everything_again() {
        let old = shown(vec![page(0, 1, Some(2)), page(1, 2, None)], None);
        let scrolled = shown(vec![page(0, 1, Some(1)), page(1, 2, None)], None);
        assert!(changes(Some(&old), &scrolled).clear);
        let rendered = shown(vec![page(0, 3, Some(2)), page(1, 2, None)], None);
        let c = changes(Some(&old), &rendered);
        assert!(c.clear);
        assert_eq!(c.pages.len(), 2);
    }

    #[test]
    fn tints_move_appear_and_go_by_themselves() {
        let pages = vec![page(0, 1, Some(2))];
        let mut old = shown(pages.clone(), None);
        old.tints = vec![tint(10, 2), tint(11, 4)];
        let mut new = shown(pages.clone(), None);
        new.tints = vec![tint(10, 2), tint(11, 5)];
        let c = changes(Some(&old), &new);
        assert!(!c.clear && c.pages.is_empty());
        assert_eq!(c.tints, vec![tint(11, 5)]);
        assert!(c.hide.is_empty());

        let closed = changes(Some(&old), &shown(pages, None));
        assert!(!closed.clear && closed.tints.is_empty());
        assert_eq!(closed.hide, vec![10, 11]);
    }
}
