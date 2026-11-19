use std::io;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
};

use std::sync::atomic::{AtomicU16, Ordering};

use crate::globals::TERMINAL_SIZE;
use crate::notes::pane::Cell;
use crate::state::HelpItem;
use crate::theme::{paint, Slot};

/* The area inside the border where pages are drawn. Positions are 0-indexed cells,
 * sizes in cells and in pixels. */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
    pub pxpercol: f64,
    pub pxperrow: f64,
}

impl Viewport {
    pub fn from_size(columns: u16, rows: u16, width: u16, height: u16) -> Self {
        let pxpercol = width as f64 / columns.max(1) as f64;
        let pxperrow = height as f64 / rows.max(1) as f64;
        Self {
            col: 1,
            row: 1,
            cols: columns.saturating_sub(2).max(1),
            rows: rows.saturating_sub(2).max(1),
            pxpercol,
            pxperrow,
        }
    }

    /* Pixel offset of the viewport inside the terminal */
    pub fn x(&self) -> f32 {
        (self.col as f64 * self.pxpercol) as f32
    }

    pub fn y(&self) -> f32 {
        (self.row as f64 * self.pxperrow) as f32
    }

    pub fn width(&self) -> f32 {
        (self.cols as f64 * self.pxpercol) as f32
    }

    pub fn height(&self) -> f32 {
        (self.rows as f64 * self.pxperrow) as f32
    }
}

/* Columns the notes panel takes on the left; the pages use the rest */
static NOTES_COLS: AtomicU16 = AtomicU16::new(0);

pub fn set_notes_cols(cols: u16) {
    NOTES_COLS.store(cols, Ordering::Release);
}

/* The whole area inside the border, where panels go */
pub fn inner() -> Viewport {
    let size = TERMINAL_SIZE.get().unwrap().read().unwrap();
    Viewport::from_size(size.columns, size.rows, size.width, size.height)
}

/* The whole window, edge to edge */
pub fn window() -> Viewport {
    let size = TERMINAL_SIZE.get().unwrap().read().unwrap();
    let mut vp = Viewport::from_size(size.columns, size.rows, size.width, size.height);
    (vp.col, vp.row, vp.cols, vp.rows) = (0, 0, size.columns.max(1), size.rows.max(1));
    vp
}

/* The PDF's framed part of the window: all of it, or what the notes terminal leaves on
 * the right. Left, top, width and height in cells, the border included. */
pub fn frame_rect() -> (u16, u16, u16, u16) {
    let window = window();
    let notes = NOTES_COLS
        .load(Ordering::Acquire)
        .min(window.cols.saturating_sub(3));
    (notes, 0, window.cols - notes, window.rows)
}

/* Where the pages go: inside the PDF's frame */
pub fn viewport() -> Viewport {
    let mut vp = window();
    let (left, top, width, height) = frame_rect();
    vp.col = left + 1;
    vp.row = top + 1;
    vp.cols = width.saturating_sub(2).max(1);
    vp.rows = height.saturating_sub(2).max(1);
    vp
}

/* How wide the notes panel is, in percent of the window; set from the config, then
 * from what Ctrl-= and Ctrl-- made it last time */
static NOTES_PERCENT: AtomicU16 = AtomicU16::new(45);
pub const NOTES_PERCENT_RANGE: (u16, u16) = (15, 85);

pub fn notes_percent() -> u16 {
    NOTES_PERCENT.load(Ordering::Acquire)
}

pub fn set_notes_percent(percent: u16) {
    let (low, high) = NOTES_PERCENT_RANGE;
    NOTES_PERCENT.store(percent.clamp(low, high), Ordering::Release);
}

/* The notes panel on the left of `inner`, `percent` of its width: left, top, width
 * and height in cells, border included */
pub fn notes_rect(window: &Viewport, percent: u16) -> (u16, u16, u16, u16) {
    let width = (window.cols as u32 * percent.clamp(10, 90) as u32 / 100) as u16;
    let width = width.max(12).min(window.cols.saturating_sub(12)).max(3);
    (window.col, window.row, width, window.rows)
}

/* A 0-indexed cell under a mouse position in pixels */
pub fn cell_at(vp: &Viewport, x: u16, y: u16) -> (u16, u16) {
    (
        (x as f64 / vp.pxpercol) as u16,
        (y as f64 / vp.pxperrow) as u16,
    )
}

fn color(slot: Slot) -> Color {
    paint(slot).color()
}

/* A panel background as a cell color. Pages sit below every cell that has a
 * background color, so an opaque panel covers them; a see-through one leaves its
 * cells clear and is tinted by an image laid under them instead. */
pub fn background(slot: Slot) -> Color {
    let p = paint(slot);
    if p.see_through() {
        Color::Reset
    } else {
        p.color()
    }
}

/* One piece of the bottom border, drawn between `│` separators */
pub struct Segment {
    pub text: String,
    pub color: Slot,
}

impl Segment {
    pub fn new(text: impl Into<String>, color: Slot) -> Self {
        Self {
            text: text.into(),
            color,
        }
    }
}

/* Cuts `text` to at most `max` columns, marking the cut with `…` */
pub fn truncate(text: &str, max: usize) -> String {
    if columns(text) <= max {
        return text.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/* How many columns `text` takes in the terminal: two for wide characters such as
 * Chinese or Japanese ones and most emoji */
pub fn columns(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/* Draws the PDF's rounded frame with `title` in the top edge and `segments` in the
 * bottom edge: `╭ title ───╮` … `╰ NOR │ 3/42 │ 110% ───╯`. Beside the notes
 * terminal, it's bright while the PDF has the keys. */
pub fn draw_frame(title: &str, segments: &[Segment], active: bool) -> io::Result<()> {
    let (left, top, width, height) = frame_rect();
    let (cols, rows) = (width as usize, height as usize);
    if cols < 4 || rows < 3 {
        return Ok(());
    }
    let inner = cols - 2;
    let mut out = io::stdout();
    let border = color(if active {
        Slot::FrameBorderActive
    } else {
        Slot::FrameBorder
    });

    /* Top edge with the title */
    let title = truncate(title, inner.saturating_sub(2));
    let title_len = columns(&title) + 2;
    queue!(
        out,
        MoveTo(left, top),
        SetForegroundColor(border),
        Print("╭"),
        SetForegroundColor(color(Slot::FrameTitle)),
        SetAttribute(Attribute::Bold),
        Print(format!(" {} ", title)),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(border),
        Print("─".repeat(inner.saturating_sub(title_len))),
        Print("╮"),
    )?;

    /* Sides */
    for row in 1..rows - 1 {
        queue!(
            out,
            MoveTo(left, top + row as u16),
            Print("│"),
            MoveTo(left + cols as u16 - 1, top + row as u16),
            Print("│"),
        )?;
    }

    /* Bottom edge with the status segments */
    queue!(out, MoveTo(left, top + rows as u16 - 1), Print("╰ "))?;
    let mut used = 1;
    for (i, segment) in segments.iter().enumerate() {
        let sep = if i == 0 { "" } else { " │ " };
        let room = inner.saturating_sub(used + columns(sep) + 1);
        if room == 0 {
            break;
        }
        let text = truncate(&segment.text, room);
        used += columns(sep) + columns(&text);
        queue!(
            out,
            SetForegroundColor(color(Slot::StatusSeparator)),
            Print(sep),
            SetForegroundColor(color(segment.color)),
            Print(&text),
        )?;
    }
    queue!(
        out,
        SetForegroundColor(border),
        Print(" "),
        Print("─".repeat(inner.saturating_sub(used + 1))),
        Print("╯"),
        ResetColor,
    )?;

    Ok(())
}

/* One line of the key list: a group's name, or the keys padded to one column and what
 * they do, or a blank line between groups */
#[derive(Debug, PartialEq)]
pub struct HelpLine {
    pub keys: String,
    pub what: String,
    pub heading: bool,
}

pub fn help_lines(items: &[HelpItem]) -> Vec<HelpLine> {
    let width = items
        .iter()
        .filter_map(|i| match i {
            HelpItem::Keys(keys, _) => Some(columns(keys)),
            HelpItem::Heading(_) => None,
        })
        .max()
        .unwrap_or(0);
    let mut lines = Vec::new();
    for item in items {
        match item {
            HelpItem::Heading(name) => {
                if !lines.is_empty() {
                    lines.push(HelpLine {
                        keys: String::new(),
                        what: String::new(),
                        heading: false,
                    });
                }
                lines.push(HelpLine {
                    keys: String::new(),
                    what: name.to_string(),
                    heading: true,
                });
            }
            HelpItem::Keys(keys, what) => lines.push(HelpLine {
                keys: format!("  {:<width$}   ", keys),
                what: what.clone(),
                heading: false,
            }),
        }
    }
    lines
}

/* Where the help box goes in the viewport for `lines`: left, top, width and height in
 * cells */
pub fn help_rect(vp: &Viewport, lines: &[HelpLine]) -> (u16, u16, u16, u16) {
    let longest = lines
        .iter()
        .map(|l| columns(&l.keys) + columns(&l.what))
        .max()
        .unwrap_or(0);
    let width = (longest + 4).clamp(24, (vp.cols as usize).saturating_sub(2).max(24));
    let height = help_visible(vp).min(lines.len().max(1)) + 2;
    let left = vp.col + (vp.cols.saturating_sub(width as u16)) / 2;
    let top = vp.row + (vp.rows.saturating_sub(height as u16)) / 2;
    (left, top, width as u16, height as u16)
}

/* Largest valid scroll offset for `lines` lines in a box showing `visible` lines */
pub fn help_max_scroll(lines: usize, visible: usize) -> usize {
    lines.saturating_sub(visible)
}

/* Visible lines of the help box for the current viewport */
pub fn help_visible(vp: &Viewport) -> usize {
    (vp.rows as usize).saturating_sub(4).max(1)
}

/* Draws the help box centered in the viewport, starting at line `scroll` */
pub fn draw_help(lines: &[HelpLine], scroll: usize) -> io::Result<()> {
    let vp = inner();
    let (left, top, width, height) = help_rect(&vp, lines);
    let (width, height) = (width as usize, height as usize);
    let visible = height - 2;
    let inner = width - 2;
    let border = color(Slot::HelpBorder);
    let mut out = io::stdout();

    let title = " Keys ";
    let pos = if lines.len() > visible {
        format!(" {}-{}/{} ", scroll + 1, scroll + visible, lines.len())
    } else {
        String::new()
    };
    queue!(
        out,
        MoveTo(left, top),
        SetBackgroundColor(background(Slot::HelpBackground)),
        SetForegroundColor(border),
        Print("╭"),
        Print(title),
        Print("─".repeat(inner.saturating_sub(title.len() + pos.len()))),
        Print(&pos),
        Print("╮"),
    )?;
    for i in 0..visible {
        let line = lines.get(scroll + i);
        if let Some(heading) = line.filter(|l| l.heading) {
            let text = truncate(&heading.what, inner.saturating_sub(2));
            let pad = inner.saturating_sub(2 + columns(&text));
            queue!(
                out,
                MoveTo(left, top + 1 + i as u16),
                SetForegroundColor(border),
                Print("│ "),
                SetForegroundColor(color(Slot::HelpHeading)),
                SetAttribute(Attribute::Bold),
                Print(&text),
                SetAttribute(Attribute::NormalIntensity),
                Print(" ".repeat(pad)),
                SetForegroundColor(border),
                Print(" │"),
            )?;
            continue;
        }
        let (keys, what) = line
            .map(|l| (l.keys.as_str(), l.what.as_str()))
            .unwrap_or(("", ""));
        let text = truncate(&format!("{}{}", keys, what), inner.saturating_sub(2));
        let pad = inner.saturating_sub(2 + columns(&text));
        let split = text
            .char_indices()
            .nth(keys.chars().count())
            .map_or(text.len(), |(i, _)| i);
        queue!(
            out,
            MoveTo(left, top + 1 + i as u16),
            SetForegroundColor(border),
            Print("│ "),
            SetForegroundColor(color(Slot::HelpKeys)),
            Print(&text[..split]),
            SetForegroundColor(color(Slot::HelpText)),
            Print(&text[split..]),
            Print(" ".repeat(pad)),
            SetForegroundColor(border),
            Print(" │"),
        )?;
    }
    let hint = " j k  d u  g G scroll · q close ";
    queue!(
        out,
        MoveTo(left, top + height as u16 - 1),
        Print("╰"),
        Print("─".repeat(inner.saturating_sub(columns(hint)))),
        SetForegroundColor(color(Slot::HelpMuted)),
        Print(hint),
        SetForegroundColor(border),
        Print("╯"),
        ResetColor,
    )
}

/* How wide the contents panel is, in percent of the pages' width; `+` and `-` in the
 * panel change it */
static TOC_PERCENT: AtomicU16 = AtomicU16::new(35);
pub const TOC_PERCENT_RANGE: (u16, u16) = (15, 85);

pub fn toc_percent() -> u16 {
    TOC_PERCENT.load(Ordering::Acquire)
}

pub fn set_toc_percent(percent: u16) {
    let (low, high) = TOC_PERCENT_RANGE;
    TOC_PERCENT.store(percent.clamp(low, high), Ordering::Release);
}

/* Columns the contents panel takes on the left of the viewport */
pub fn toc_width(vp: &Viewport) -> u16 {
    ((vp.cols as u32 * toc_percent() as u32 / 100) as u16)
        .max(20)
        .min(vp.cols)
}

/* Rows of entries the contents panel shows */
pub fn toc_visible(vp: &Viewport) -> usize {
    (vp.rows as usize).saturating_sub(2).max(1)
}

/* The panel row and the column within the panel under a mouse position in pixels */
pub fn toc_hit(vp: &Viewport, x: u16, y: u16) -> Option<(usize, u16)> {
    let col = (x as f64 / vp.pxpercol) as u16;
    let row = (y as f64 / vp.pxperrow) as u16;
    let inside = col >= vp.col && col < vp.col + toc_width(vp);
    let first = vp.row + 1;
    (inside && row >= first && ((row - first) as usize) < toc_visible(vp))
        .then(|| ((row - first) as usize, col - vp.col))
}

/* Whether a mouse position in pixels is over the panel, borders included */
pub fn over_toc(vp: &Viewport, x: u16, y: u16) -> bool {
    let col = (x as f64 / vp.pxpercol) as u16;
    let row = (y as f64 / vp.pxperrow) as u16;
    col >= vp.col
        && col < vp.col + toc_width(vp)
        && row >= vp.row
        && row < vp.row + vp.rows
}

/* Column of an entry's fold marker within the panel */
pub fn toc_marker_col(depth: usize) -> u16 {
    (2 + 2 * depth) as u16
}

/* One line of the contents panel */
pub struct TocRow {
    pub depth: usize,
    /* `▾` open, `▸` closed, ` ` no children */
    pub marker: char,
    pub title: String,
    pub label: String,
    pub selected: bool,
    /* The section being read */
    pub current: bool,
}

/* Draws the contents panel down the left of the viewport. `rows` are the visible ones,
 * `position` goes in the top edge and `footer` in the bottom edge. */
/* The contents panel opens at the left of the pages, beside the note when it's open */
pub fn draw_toc(rows: &[TocRow], position: &str, footer: &str) -> io::Result<()> {
    let vp = viewport();
    let width = toc_width(&vp) as usize;
    if width < 8 || vp.rows < 3 {
        return Ok(());
    }
    let inner = width - 2;
    let (left, top) = (vp.col, vp.row);
    let mut out = io::stdout();

    let title = " Contents ";
    let position = format!(" {} ", position);
    let (bg, border) = (
        background(Slot::ContentsBackground),
        color(Slot::ContentsBorder),
    );
    queue!(
        out,
        MoveTo(left, top),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("╭"),
        SetForegroundColor(color(Slot::ContentsTitle)),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::NormalIntensity),
        SetForegroundColor(border),
        Print("─".repeat(inner.saturating_sub(title.len() + columns(&position)))),
        SetForegroundColor(color(Slot::ContentsMuted)),
        Print(&position),
        SetForegroundColor(border),
        Print("╮"),
    )?;

    for i in 0..toc_visible(&vp) {
        let y = top + 1 + i as u16;
        queue!(
            out,
            MoveTo(left, y),
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print("│"),
        )?;
        match rows.get(i) {
            Some(row) => draw_toc_row(&mut out, row, inner)?,
            None => queue!(out, Print(" ".repeat(inner)))?,
        }
        queue!(
            out,
            SetBackgroundColor(bg),
            SetForegroundColor(border),
            Print("│")
        )?;
    }

    let footer = truncate(footer, inner.saturating_sub(2));
    queue!(
        out,
        MoveTo(left, top + vp.rows - 1),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("╰"),
        SetForegroundColor(color(Slot::ContentsMuted)),
        Print(format!(" {} ", footer)),
        SetForegroundColor(border),
        Print("─".repeat(inner.saturating_sub(columns(&footer) + 2))),
        Print("╯"),
        ResetColor,
    )
}

/* ` ▾ 2.1  Setup            16 `, filling `inner` columns */
fn draw_toc_row(out: &mut io::Stdout, row: &TocRow, inner: usize) -> io::Result<()> {
    let bg = if row.selected {
        background(Slot::ContentsSelected)
    } else {
        background(Slot::ContentsBackground)
    };
    let (muted, current) = (color(Slot::ContentsMuted), color(Slot::ContentsCurrent));
    let indent = 1 + 2 * row.depth.min(inner / 4);
    let label = truncate(&row.label, inner / 3);
    let label_len = columns(&label);
    /* indent, marker and a space, the title, a space, the label, a space */
    let room = inner.saturating_sub(indent + 2 + 1 + label_len + 1);
    let title = truncate(&row.title, room);
    let pad = room - columns(&title);
    let text = if row.current {
        current
    } else {
        color(Slot::ContentsText)
    };

    queue!(out, SetBackgroundColor(bg), Print(" ".repeat(indent)))?;
    queue!(
        out,
        SetForegroundColor(if row.current { current } else { muted }),
        Print(if row.current && row.marker == ' ' {
            '•'
        } else {
            row.marker
        }),
        Print(' '),
        SetForegroundColor(text),
    )?;
    if row.selected {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }
    queue!(
        out,
        Print(&title),
        SetAttribute(Attribute::NormalIntensity),
        Print(" ".repeat(pad + 1)),
        SetForegroundColor(if row.selected {
            color(Slot::ContentsSelectedText)
        } else {
            muted
        }),
        Print(&label),
        Print(' '),
    )
}

/* Draws the notes terminal's screen at `rect`, edge to edge */
pub fn draw_notes(rect: (u16, u16, u16, u16), screen: &[Vec<Cell>]) -> io::Result<()> {
    let (left, top, width, height) = rect;
    let bg = background(Slot::NotesBackground);
    let mut out = io::stdout();
    for row in 0..height {
        queue!(out, MoveTo(left, top + row), ResetColor)?;
        match screen.get(row as usize) {
            Some(cells) => draw_cells(&mut out, cells, width as usize)?,
            None => queue!(
                out,
                SetBackgroundColor(bg),
                Print(" ".repeat(width as usize))
            )?,
        }
    }
    queue!(out, ResetColor)
}

/* One row of the editor's screen, `width` cells */
fn draw_cells(out: &mut io::Stdout, cells: &[Cell], width: usize) -> io::Result<()> {
    let mut last: Option<&Cell> = None;
    /* A wide character's second half has its own cell, so cells are columns */
    let drawn = cells.len().min(width);
    for cell in cells.iter().take(width) {
        if cell.text.is_empty() {
            /* The second half of a wide character, drawn with the first */
            continue;
        }
        let same = last.is_some_and(|l| {
            (
                l.fg,
                l.bg,
                l.bold,
                l.dim,
                l.italic,
                l.underline,
                l.strike,
                l.reverse,
            ) == (
                cell.fg,
                cell.bg,
                cell.bold,
                cell.dim,
                cell.italic,
                cell.underline,
                cell.strike,
                cell.reverse,
            )
        });
        if !same {
            queue!(
                out,
                SetAttribute(Attribute::Reset),
                SetForegroundColor(cell.fg),
                SetBackgroundColor(cell.bg),
            )?;
            for (on, attribute) in [
                (cell.bold, Attribute::Bold),
                (cell.dim, Attribute::Dim),
                (cell.italic, Attribute::Italic),
                (cell.underline, Attribute::Underlined),
                (cell.strike, Attribute::CrossedOut),
                (cell.reverse, Attribute::Reverse),
            ] {
                if on {
                    queue!(out, SetAttribute(attribute))?;
                }
            }
        }
        queue!(out, Print(&cell.text))?;
        last = Some(cell);
    }
    queue!(out, SetAttribute(Attribute::Reset))?;
    if drawn < width {
        queue!(
            out,
            SetBackgroundColor(background(Slot::NotesBackground)),
            Print(" ".repeat(width - drawn)),
        )?;
    }
    Ok(())
}

/* A layer's name and what its keys do */
pub struct Layer {
    pub title: String,
    pub keys: Vec<(String, String)>,
}

/* Where a layer's box goes: in the bottom right corner of `inner` */
pub fn layer_rect(inner: &Viewport, layer: &Layer) -> (u16, u16, u16, u16) {
    let keys = layer
        .keys
        .iter()
        .map(|(k, _)| columns(k))
        .max()
        .unwrap_or(0);
    let what = layer
        .keys
        .iter()
        .map(|(_, w)| columns(w))
        .max()
        .unwrap_or(0);
    let width = (keys + what + 6)
        .max(columns(&layer.title) + 6)
        .min(inner.cols as usize) as u16;
    let height = (layer.keys.len() + 2).min(inner.rows as usize) as u16;
    (
        inner.col + inner.cols - width,
        inner.row + inner.rows - height,
        width,
        height,
    )
}

pub fn draw_layer(layer: &Layer) -> io::Result<()> {
    let (left, top, width, height) = layer_rect(&viewport(), layer);
    if width < 6 || height < 3 {
        return Ok(());
    }
    let inner = width as usize - 2;
    let bg = background(Slot::LayerBackground);
    let border = color(Slot::LayerBorder);
    let keys = layer
        .keys
        .iter()
        .map(|(k, _)| columns(k))
        .max()
        .unwrap_or(0);
    let mut out = io::stdout();
    let title = truncate(&layer.title, inner.saturating_sub(4));
    queue!(
        out,
        MoveTo(left, top),
        SetBackgroundColor(bg),
        SetForegroundColor(border),
        Print("╭ "),
        SetForegroundColor(color(Slot::LayerTitle)),
        SetAttribute(Attribute::Bold),
        Print(&title),
        SetAttribute(Attribute::NormalIntensity),
        SetForegroundColor(border),
        Print(" "),
        Print("─".repeat(inner.saturating_sub(columns(&title) + 2))),
        Print("╮"),
    )?;
    for (i, (key, what)) in layer.keys.iter().take(height as usize - 2).enumerate() {
        let text = truncate(what, inner.saturating_sub(keys + 4));
        let pad = inner.saturating_sub(keys + 4 + columns(&text));
        queue!(
            out,
            MoveTo(left, top + 1 + i as u16),
            SetForegroundColor(border),
            Print("│ "),
            SetForegroundColor(color(Slot::LayerKeys)),
            Print(format!("{:<keys$}", key)),
            Print("  "),
            SetForegroundColor(color(Slot::LayerText)),
            Print(&text),
            Print(" ".repeat(pad)),
            SetForegroundColor(border),
            Print(" │"),
        )?;
    }
    queue!(
        out,
        MoveTo(left, top + height - 1),
        SetForegroundColor(border),
        Print("╰"),
        Print("─".repeat(inner)),
        Print("╯"),
        ResetColor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_leaves_a_one_cell_border() {
        let vp = Viewport::from_size(100, 40, 1000, 800);
        assert_eq!((vp.col, vp.row, vp.cols, vp.rows), (1, 1, 98, 38));
        assert_eq!((vp.pxpercol, vp.pxperrow), (10.0, 20.0));
        assert_eq!(
            (vp.x(), vp.y(), vp.width(), vp.height()),
            (10.0, 20.0, 980.0, 760.0)
        );
    }

    #[test]
    fn viewport_survives_tiny_terminals() {
        let vp = Viewport::from_size(1, 1, 0, 0);
        assert_eq!((vp.cols, vp.rows), (1, 1));
    }

    #[test]
    fn wide_characters_take_two_columns() {
        assert_eq!(columns("ノート.pdf"), 10);
        assert_eq!(truncate("ノートのPDF", 6), "ノー…");
        assert_eq!(columns(&truncate("ノートのPDF", 6)), 5);
    }

    #[test]
    fn truncate_marks_cuts() {
        assert_eq!(truncate("report.pdf", 20), "report.pdf");
        assert_eq!(truncate("report.pdf", 6), "repor…");
        assert_eq!(truncate("report.pdf", 0), "");
    }

    #[test]
    fn help_lines_align_descriptions() {
        let lines = help_lines(&[
            HelpItem::Heading("Moving"),
            HelpItem::Keys("j, Down".to_owned(), "Move down".to_owned()),
            HelpItem::Heading("Other"),
            HelpItem::Keys("?".to_owned(), "Show keys".to_owned()),
        ]);
        let shown: Vec<String> = lines
            .iter()
            .map(|l| format!("{}{}", l.keys, l.what))
            .collect();
        assert_eq!(
            shown,
            vec![
                "Moving",
                "  j, Down   Move down",
                "",
                "Other",
                "  ?         Show keys"
            ]
        );
        assert!(lines[0].heading && !lines[1].heading && !lines[2].heading);
    }

    #[test]
    fn contents_panel_takes_the_left_side() {
        let vp = Viewport::from_size(100, 40, 1000, 800);
        assert_eq!(toc_width(&vp), 34);
        assert_eq!(toc_visible(&vp), 36);
        let narrow = Viewport::from_size(20, 10, 200, 200);
        assert_eq!(toc_width(&narrow), 18);
        let wide = Viewport::from_size(300, 40, 3000, 800);
        assert_eq!(toc_width(&wide), 104);
    }

    #[test]
    fn contents_panel_rows_under_the_mouse() {
        /* Cells are 10 by 20 pixels; entries start at cell row 2 */
        let vp = Viewport::from_size(100, 40, 1000, 800);
        assert_eq!(toc_hit(&vp, 15, 45), Some((0, 0)));
        assert_eq!(toc_hit(&vp, 55, 85), Some((2, 4)));
        assert_eq!(toc_hit(&vp, 15, 25), None);
        assert_eq!(toc_hit(&vp, 400, 85), None);
        assert!(over_toc(&vp, 15, 25));
        assert!(!over_toc(&vp, 400, 85));
    }

    #[test]
    fn the_notes_panel_takes_the_left_and_the_layer_box_the_corner() {
        let vp = Viewport::from_size(100, 40, 1000, 800);
        let edge_to_edge = |cols, rows| {
            let mut window = Viewport::from_size(cols, rows, cols * 10, rows * 20);
            (window.col, window.row, window.cols, window.rows) = (0, 0, cols, rows);
            window
        };
        /* The whole height, from the left edge, with no border */
        assert_eq!(notes_rect(&edge_to_edge(100, 40), 45), (0, 0, 45, 40));
        /* A narrow window keeps 12 columns for the PDF */
        assert_eq!(notes_rect(&edge_to_edge(20, 10), 45).2, 8);
        let layer = Layer {
            title: "Notes".to_owned(),
            keys: vec![
                ("x".to_owned(), "Hide".to_owned()),
                ("s".to_owned(), "Switch".to_owned()),
            ],
        };
        /* `x  Switch` plus the border and padding: 13 wide, 4 high, in the corner */
        assert_eq!(layer_rect(&vp, &layer), (86, 35, 13, 4));
    }

    #[test]
    fn help_scroll_stops_at_the_end() {
        assert_eq!(help_max_scroll(30, 10), 20);
        assert_eq!(help_max_scroll(5, 10), 0);
    }
}
