use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
};

use crate::TERMINAL_SIZE;

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

pub fn viewport() -> Viewport {
    let size = TERMINAL_SIZE.get().unwrap().read().unwrap();
    Viewport::from_size(size.columns, size.rows, size.width, size.height)
}

const ACCENT: Color = Color::Cyan;
const DIM: Color = Color::DarkGrey;
const MATCH: Color = Color::Yellow;
/* Pages sit below every cell that has a background color, so the panel covers them */
const PANEL_BG: Color = Color::Black;
const PANEL_FG: Color = Color::White;
const PANEL_SELECTED: Color = Color::DarkGrey;

/* One piece of the bottom border, drawn between `│` separators */
pub struct Segment {
    pub text: String,
    pub color: Option<Color>,
}

impl Segment {
    pub fn new(text: impl Into<String>, color: Option<Color>) -> Self {
        Self {
            text: text.into(),
            color,
        }
    }
}

pub fn mode_segment(mode: &str) -> Segment {
    Segment::new(mode, Some(ACCENT))
}

pub fn match_segment(text: String) -> Segment {
    Segment::new(text, Some(MATCH))
}

/* Cuts `text` to at most `width` characters, marking the cut with `…` */
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

/* Draws the rounded frame with `title` in the top edge and `segments` in the
 * bottom edge: `╭ title ───╮` … `╰ NOR │ 3/42 │ 110% ───╯` */
pub fn draw_frame(title: &str, segments: &[Segment]) -> io::Result<()> {
    let (cols, rows) = {
        let size = TERMINAL_SIZE.get().unwrap().read().unwrap();
        (size.columns as usize, size.rows as usize)
    };
    if cols < 4 || rows < 3 {
        return Ok(());
    }
    let inner = cols - 2;
    let mut out = io::stdout();

    /* Top edge with the title */
    let title = truncate(title, inner.saturating_sub(2));
    let title_len = title.chars().count() + 2;
    queue!(
        out,
        MoveTo(0, 0),
        SetForegroundColor(DIM),
        Print("╭"),
        ResetColor,
        SetAttribute(Attribute::Bold),
        Print(format!(" {} ", title)),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(DIM),
        Print("─".repeat(inner.saturating_sub(title_len))),
        Print("╮"),
    )?;

    /* Sides */
    for row in 1..rows - 1 {
        queue!(
            out,
            MoveTo(0, row as u16),
            Print("│"),
            MoveTo(cols as u16 - 1, row as u16),
            Print("│"),
        )?;
    }

    /* Bottom edge with the status segments */
    queue!(out, MoveTo(0, rows as u16 - 1), Print("╰ "), ResetColor)?;
    let mut used = 1;
    for (i, segment) in segments.iter().enumerate() {
        let sep = if i == 0 { "" } else { " │ " };
        let room = inner.saturating_sub(used + sep.chars().count() + 1);
        if room == 0 {
            break;
        }
        let text = truncate(&segment.text, room);
        used += sep.chars().count() + text.chars().count();
        queue!(out, SetForegroundColor(DIM), Print(sep), ResetColor)?;
        match segment.color {
            Some(color) => {
                queue!(out, SetForegroundColor(color), Print(&text), ResetColor)?
            }
            None => queue!(out, Print(&text))?,
        }
    }
    queue!(
        out,
        SetForegroundColor(DIM),
        Print(" "),
        Print("─".repeat(inner.saturating_sub(used + 1))),
        Print("╯"),
        ResetColor,
    )?;

    Ok(())
}

/* Lines of the help overlay: `keys  description`, keys padded to one column */
pub fn help_lines(entries: &[(String, String)]) -> Vec<String> {
    let width = entries
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    entries
        .iter()
        .map(|(keys, what)| {
            let pad = width - keys.chars().count();
            format!("{}{}   {}", keys, " ".repeat(pad), what)
        })
        .collect()
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
pub fn draw_help(lines: &[String], scroll: usize) -> io::Result<()> {
    let vp = viewport();
    let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let width = (longest + 4).clamp(24, (vp.cols as usize).saturating_sub(2).max(24));
    let visible = help_visible(&vp).min(lines.len().max(1));
    let height = visible + 2;
    let left = vp.col + (vp.cols.saturating_sub(width as u16)) / 2;
    let top = vp.row + (vp.rows.saturating_sub(height as u16)) / 2;
    let inner = width - 2;
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
        SetForegroundColor(ACCENT),
        Print("╭"),
        Print(title),
        Print("─".repeat(inner.saturating_sub(title.len() + pos.len()))),
        Print(&pos),
        Print("╮"),
    )?;
    for i in 0..visible {
        let text = lines.get(scroll + i).map(String::as_str).unwrap_or("");
        let text = truncate(text, inner.saturating_sub(2));
        let pad = inner.saturating_sub(2 + text.chars().count());
        queue!(
            out,
            MoveTo(left, top + 1 + i as u16),
            SetForegroundColor(ACCENT),
            Print("│ "),
            ResetColor,
            Print(&text),
            Print(" ".repeat(pad)),
            SetForegroundColor(ACCENT),
            Print(" │"),
        )?;
    }
    let hint = " j/k scroll · q close ";
    queue!(
        out,
        MoveTo(left, top + height as u16 - 1),
        Print("╰"),
        Print("─".repeat(inner.saturating_sub(hint.chars().count()))),
        SetForegroundColor(DIM),
        Print(hint),
        SetForegroundColor(ACCENT),
        Print("╯"),
        ResetColor,
    )?;
    out.flush()
}

/* Columns the contents panel takes on the left of the viewport */
pub fn toc_width(vp: &Viewport) -> u16 {
    ((vp.cols as u32 * 35 / 100) as u16)
        .clamp(28, 60)
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
    queue!(
        out,
        MoveTo(left, top),
        SetBackgroundColor(PANEL_BG),
        SetForegroundColor(ACCENT),
        Print("╭"),
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::NormalIntensity),
        Print("─".repeat(inner.saturating_sub(title.len() + position.chars().count()))),
        SetForegroundColor(DIM),
        Print(&position),
        SetForegroundColor(ACCENT),
        Print("╮"),
    )?;

    for i in 0..toc_visible(&vp) {
        let y = top + 1 + i as u16;
        queue!(
            out,
            MoveTo(left, y),
            SetBackgroundColor(PANEL_BG),
            SetForegroundColor(ACCENT),
            Print("│"),
        )?;
        match rows.get(i) {
            Some(row) => draw_toc_row(&mut out, row, inner)?,
            None => queue!(out, Print(" ".repeat(inner)))?,
        }
        queue!(
            out,
            SetBackgroundColor(PANEL_BG),
            SetForegroundColor(ACCENT),
            Print("│"),
        )?;
    }

    let footer = truncate(footer, inner.saturating_sub(2));
    queue!(
        out,
        MoveTo(left, top + vp.rows - 1),
        SetBackgroundColor(PANEL_BG),
        SetForegroundColor(ACCENT),
        Print("╰"),
        SetForegroundColor(DIM),
        Print(format!(" {} ", footer)),
        SetForegroundColor(ACCENT),
        Print("─".repeat(inner.saturating_sub(footer.chars().count() + 2))),
        Print("╯"),
        ResetColor,
    )
}

/* ` ▾ 2.1  Setup            16 `, filling `inner` columns */
fn draw_toc_row(out: &mut io::Stdout, row: &TocRow, inner: usize) -> io::Result<()> {
    let bg = if row.selected {
        PANEL_SELECTED
    } else {
        PANEL_BG
    };
    let indent = 1 + 2 * row.depth.min(inner / 4);
    let label = truncate(&row.label, inner / 3);
    let label_len = label.chars().count();
    /* indent, marker and a space, the title, a space, the label, a space */
    let room = inner.saturating_sub(indent + 2 + 1 + label_len + 1);
    let title = truncate(&row.title, room);
    let pad = room - title.chars().count();
    let text = if row.current { ACCENT } else { PANEL_FG };

    queue!(out, SetBackgroundColor(bg), Print(" ".repeat(indent)))?;
    queue!(
        out,
        SetForegroundColor(if row.current { ACCENT } else { DIM }),
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
        SetForegroundColor(if row.selected { PANEL_FG } else { DIM }),
        Print(&label),
        Print(' '),
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
    fn truncate_marks_cuts() {
        assert_eq!(truncate("report.pdf", 20), "report.pdf");
        assert_eq!(truncate("report.pdf", 6), "repor…");
        assert_eq!(truncate("report.pdf", 0), "");
    }

    #[test]
    fn help_lines_align_descriptions() {
        let lines = help_lines(&[
            ("j, Down".to_owned(), "Move down".to_owned()),
            ("?".to_owned(), "Show keys".to_owned()),
        ]);
        assert_eq!(lines, vec!["j, Down   Move down", "?         Show keys"]);
    }

    #[test]
    fn contents_panel_takes_the_left_side() {
        let vp = Viewport::from_size(100, 40, 1000, 800);
        assert_eq!(toc_width(&vp), 34);
        assert_eq!(toc_visible(&vp), 36);
        let narrow = Viewport::from_size(20, 10, 200, 200);
        assert_eq!(toc_width(&narrow), 18);
        let wide = Viewport::from_size(300, 40, 3000, 800);
        assert_eq!(toc_width(&wide), 60);
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
    fn help_scroll_stops_at_the_end() {
        assert_eq!(help_max_scroll(30, 10), 20);
        assert_eq!(help_max_scroll(5, 10), 0);
    }
}
