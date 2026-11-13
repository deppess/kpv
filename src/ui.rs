use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
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
    fn help_scroll_stops_at_the_end() {
        assert_eq!(help_max_scroll(30, 10), 20);
        assert_eq!(help_max_scroll(5, 10), 0);
    }
}
