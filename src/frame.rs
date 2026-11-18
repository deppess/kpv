/* Drawing one frame: pages, ruler, contents panel and the frame with its status bar */

use std::io::{self, Write};
use std::time::Duration;

use crossbeam_channel::Receiver;
use crossterm::cursor::MoveTo;
use crossterm::terminal::{
    BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate,
};
use crossterm::{execute, queue};

use crate::app::*;
use crate::config::*;
use crate::drivers;
use crate::drivers::commands::{ClearImages, PointerShape, SetPointerShape};
use crate::drivers::graphics::GraphicsResponse;
use crate::image::*;
use crate::outline::{breadcrumb, current_section};
use crate::state::*;
use crate::threads::renderer::Renderer;
use crate::ui::{self, toc_visible, viewport, Segment, TocRow};
use crate::viewer::*;

pub fn write_error(e: io::Error) -> String {
    format!("Could not write to the terminal: {}", e)
}

/* Longest wait for kitty to confirm placing a page */
const RESPONSE_TIMEOUT: Duration = Duration::from_millis(500);

/* Draws one frame in a synchronized update and returns the pages placed in it, whose
 * answers from kitty are due. Text is drawn every time; page images and the ruler only
 * when they differ from the last frame's, so that a frame that only changes text
 * (prompts, the contents panel, the status bar) costs kitty no image work. */
pub fn draw_frame(
    title: &str,
    viewer: &mut Viewer,
    renderer: &Renderer,
    app: &mut App,
) -> Result<Vec<usize>, String> {
    let shown = match app.mode {
        Mode::Help { .. } => Shown {
            pages: Vec::new(),
            ruler: None,
        },
        _ => Shown {
            pages: viewer.plan_pages(renderer),
            ruler: ruler_band(app, viewer).and_then(|b| ruler_placement(b, &viewport())),
        },
    };
    let Changes {
        clear,
        pages,
        ruler,
    } = changes(app.shown.as_ref(), &shown);

    let mut out = io::stdout();
    queue!(out, BeginSynchronizedUpdate).map_err(write_error)?;
    if clear {
        queue!(out, ClearImages).map_err(write_error)?;
    }
    /* Erase from the top-left corner down, never the whole display: in kitty,
     * erasing the whole display (ED 2) also deletes images that have no placement
     * yet, i.e. pages uploaded since the last frame, so they could never be shown */
    queue!(out, MoveTo(0, 0), Clear(ClearType::FromCursorDown)).map_err(write_error)?;

    let mut displayed = Vec::new();
    for page in &pages {
        page.place()?;
        displayed.push(page.page);
    }
    if ruler {
        match &shown.ruler {
            Some(ruler) => place_ruler(app, ruler),
            None if !clear => {
                let _ = drivers::graphics::terminal_graphics_hide_ruler();
            }
            None => (),
        }
    }
    app.shown = Some(shown);
    if let Mode::Help { scroll } = app.mode {
        ui::draw_help(&app.help, scroll).map_err(write_error)?;
    }

    let (page, y) = viewer.reading_spot();
    let section = current_section(&app.toc.entries, page, y);
    if app.mode == Mode::Toc {
        draw_toc(app, viewer, section).map_err(write_error)?;
    }

    ui::draw_frame(title, &status_segments(app, viewer, section)).map_err(write_error)?;
    queue!(out, EndSynchronizedUpdate).map_err(write_error)?;
    out.flush().map_err(write_error)?;
    Ok(displayed)
}

/* The pieces of the bottom border, left to right. `section` is the outline entry
 * being read. */
pub fn status_segments(
    app: &App,
    viewer: &Viewer,
    section: Option<usize>,
) -> Vec<Segment> {
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

pub fn draw_toc(app: &App, viewer: &Viewer, section: Option<usize>) -> io::Result<()> {
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
    ui::draw_toc(&visible, &position, &footer)
}

/* Lays the ruler over its line; the image is sent again only when its pixels change,
 * not when it just moves */
fn place_ruler(app: &mut App, placement: &RulerPlacement) {
    const COLOR: [u8; 4] = [90, 160, 255, 70];
    let same_pixels = app.ruler_image.as_ref().is_some_and(|old| {
        (old.width, old.height, old.band)
            == (placement.width, placement.height, placement.band)
    });
    if !same_pixels {
        let data = ruler_bitmap(placement, COLOR);
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

/* Sends the pointer shape when it changes */
pub fn set_pointer(app: &mut App, shape: PointerShape) {
    if app.pointer != shape {
        app.pointer = shape;
        /* Only looks; a failed write shows up when the frame is drawn */
        let _ = execute!(io::stdout(), SetPointerShape(shape));
    }
}

/* Reads kitty's answer for every page placed in the last frame and sends a page again
 * when kitty no longer has it. Never waits on the terminal for good: a missing answer
 * must not freeze the viewer. */
pub fn check_placements(
    answers: &Receiver<GraphicsResponse>,
    displayed: Vec<usize>,
    viewer: &mut Viewer,
    app: &mut App,
) {
    for page in displayed {
        let Ok(res) = answers.recv_timeout(RESPONSE_TIMEOUT) else {
            break;
        };
        if res.payload().contains("OK") {
            viewer.placement_ok(page);
            continue;
        }

        /* Place everything again next frame, once the page is sent again */
        app.shown = None;
        if !viewer.schedule_transfer(page) {
            app.message = Some(format!(
                "kitty rejects page {}: {}",
                page + 1,
                res.payload()
            ));
        }
    }
}
