use std::collections::HashMap;

use mupdf::{Document, TextPageFlags};

/* One line of text across the page, in page units from the page's top */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub top: f32,
    pub bottom: f32,
}

/* Turns the lines MuPDF finds into rows read top to bottom. Lines side by side, as in
 * two columns or a line broken into pieces, become one row. */
pub fn merge_rows(mut lines: Vec<Row>) -> Vec<Row> {
    /* Specks such as dots in a table of contents aren't lines worth stopping on */
    const MIN_HEIGHT: f32 = 2.0;
    lines.retain(|l| l.top.is_finite() && l.bottom - l.top >= MIN_HEIGHT);
    lines.sort_by(|a, b| a.top.total_cmp(&b.top));

    let mut rows: Vec<Row> = Vec::new();
    for line in lines {
        if let Some(last) = rows.last_mut() {
            let overlap = last.bottom.min(line.bottom) - last.top.max(line.top);
            let smaller = (last.bottom - last.top).min(line.bottom - line.top);
            if overlap > smaller * 0.5 {
                last.top = last.top.min(line.top);
                last.bottom = last.bottom.max(line.bottom);
                continue;
            }
        }
        rows.push(line);
    }
    rows
}

/* Text rows per page, read on first use from a copy of the document that the main
 * thread owns, so the ruler never waits on the renderer */
pub struct Lines {
    file: String,
    document: Option<Document>,
    rows: HashMap<usize, Vec<Row>>,
}

impl Lines {
    pub fn new(file: &str) -> Self {
        Self {
            file: file.to_owned(),
            document: None,
            rows: HashMap::new(),
        }
    }

    /* The file changed: read it again when next needed */
    pub fn reset(&mut self) {
        self.document = None;
        self.rows.clear();
    }

    pub fn rows(&mut self, page: usize) -> &[Row] {
        if !self.rows.contains_key(&page) {
            let rows = self.read(page).unwrap_or_default();
            self.rows.insert(page, rows);
        }
        &self.rows[&page]
    }

    fn read(&mut self, page: usize) -> Option<Vec<Row>> {
        if self.document.is_none() {
            self.document = Document::open(&self.file).ok();
        }
        let page = self.document.as_ref()?.load_page(page as i32).ok()?;
        let origin = page.bounds().ok()?;
        let text = page.to_text_page(TextPageFlags::empty()).ok()?;
        let lines = text
            .blocks()
            .flat_map(|block| {
                block
                    .lines()
                    .map(|line| {
                        let b = line.bounds();
                        Row {
                            top: b.y0 - origin.y0,
                            bottom: b.y1 - origin.y0,
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        Some(merge_rows(lines))
    }
}

/* Where the ruler is: a row of a page */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ruler {
    pub page: usize,
    pub row: usize,
}

/* The ruler `delta` rows on from `from`, crossing pages and passing over pages without
 * text. `rows(page)` counts a page's rows. `None` past either end. */
pub fn step(
    from: Ruler,
    delta: isize,
    pages: usize,
    mut rows: impl FnMut(usize) -> usize,
) -> Option<Ruler> {
    let mut at = from;
    for _ in 0..delta.unsigned_abs() {
        at = if delta > 0 {
            if at.row + 1 < rows(at.page) {
                Ruler {
                    page: at.page,
                    row: at.row + 1,
                }
            } else {
                let page = (at.page + 1..pages).find(|&p| rows(p) > 0)?;
                Ruler { page, row: 0 }
            }
        } else if at.row > 0 {
            Ruler {
                page: at.page,
                row: at.row - 1,
            }
        } else {
            let page = (0..at.page).rev().find(|&p| rows(p) > 0)?;
            Ruler {
                page,
                row: rows(page) - 1,
            }
        };
    }
    Some(at)
}

/* The first row at or below `y` on `page`, or the first row of a later page */
pub fn first_from(
    page: usize,
    y: f32,
    pages: usize,
    mut rows_of: impl FnMut(usize) -> Vec<Row>,
) -> Option<Ruler> {
    if let Some(row) = rows_of(page).iter().position(|r| r.bottom > y) {
        return Some(Ruler { page, row });
    }
    let page = (page + 1..pages).find(|&p| !rows_of(p).is_empty())?;
    Some(Ruler { page, row: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(top: f32, bottom: f32) -> Row {
        Row { top, bottom }
    }

    #[test]
    fn lines_side_by_side_become_one_row() {
        let rows = merge_rows(vec![
            row(30.0, 40.0),
            row(10.0, 20.0),
            /* The second column's first line, a little lower */
            row(11.0, 21.0),
            row(50.0, 50.5),
        ]);
        assert_eq!(rows, [row(10.0, 21.0), row(30.0, 40.0)]);
    }

    #[test]
    fn lines_that_only_touch_stay_apart() {
        let rows = merge_rows(vec![row(10.0, 20.0), row(19.0, 29.0)]);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn the_ruler_crosses_pages_and_skips_empty_ones() {
        /* Page 1 is a picture without text */
        let counts = [2, 0, 3];
        let rows = |p: usize| counts[p];
        let at = |page, row| Ruler { page, row };
        assert_eq!(step(at(0, 0), 1, 3, rows), Some(at(0, 1)));
        assert_eq!(step(at(0, 1), 1, 3, rows), Some(at(2, 0)));
        assert_eq!(step(at(2, 0), -1, 3, rows), Some(at(0, 1)));
        assert_eq!(step(at(0, 0), 4, 3, rows), Some(at(2, 2)));
        assert_eq!(step(at(2, 2), 1, 3, rows), None);
        assert_eq!(step(at(0, 0), -1, 3, rows), None);
    }

    #[test]
    fn the_ruler_starts_at_the_first_line_in_view() {
        let pages = [
            vec![row(10.0, 20.0), row(30.0, 40.0)],
            vec![],
            vec![row(5.0, 9.0)],
        ];
        let rows = |p: usize| pages[p].clone();
        assert_eq!(
            first_from(0, 25.0, 3, rows),
            Some(Ruler { page: 0, row: 1 })
        );
        assert_eq!(
            first_from(0, 15.0, 3, rows),
            Some(Ruler { page: 0, row: 0 })
        );
        assert_eq!(
            first_from(0, 45.0, 3, rows),
            Some(Ruler { page: 2, row: 0 })
        );
        assert_eq!(first_from(2, 50.0, 3, rows), None);
    }

    #[test]
    fn rows_are_read_from_a_pdf() {
        let content = "BT /F1 20 Tf 72 700 Td (First line) Tj ET \
                       BT /F1 20 Tf 72 600 Td (Second line) Tj ET \
                       BT /F1 20 Tf 320 600 Td (Next to it) Tj ET";
        let page_content = format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            content
        );
        let bytes = crate::testutil::pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>",
            &page_content,
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        ]);
        let dir =
            std::env::temp_dir().join(format!("viewmer-ruler-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lines.pdf");
        std::fs::write(&file, bytes).unwrap();

        let mut lines = Lines::new(file.to_str().unwrap());
        let rows = lines.rows(0).to_vec();
        assert_eq!(rows.len(), 2);
        /* Measured down from the top: the line at 700 comes first */
        assert!(rows[0].top < 100.0 && rows[1].top > 150.0, "{:?}", rows);
        assert!(lines.rows(5).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
