use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    drivers::graphics::*,
    ui::{viewport, Viewport},
    viewer::DisplayRect,
    IMAGE_PADDING,
};

use mupdf::Pixmap;

pub struct Image {
    id: usize,
    /* Stores the dimension of the zoomed in bitmap WITHOUT padding */
    dimensions: (i32, i32),
    data: Vec<u8>,
}

impl Image {
    pub fn new(pixmap: &Pixmap) -> Result<Self, String> {
        static ID: AtomicUsize = AtomicUsize::new(1);

        const PADDING_CLR: u8 = 0u8;
        let padding = *IMAGE_PADDING.get().unwrap();

        let mut data = Vec::with_capacity(
            (2 * padding + pixmap.width() as usize) * padding * 8
                + padding * 8
                + (pixmap.samples().len() / 3) * 4,
        );

        data.extend(std::iter::repeat_n(
            PADDING_CLR,
            (2 * padding + pixmap.width() as usize) * padding * 4,
        ));

        let row_iter = pixmap
            .samples()
            .chunks(pixmap.width() as usize * pixmap.n() as usize);
        for row in row_iter {
            data.extend(std::iter::repeat_n(PADDING_CLR, padding * 4));

            /* If not RGBA extend by adding the alpha channel */
            if pixmap.n() == 3 {
                let pixel_iter = row.chunks(3);
                for pixel in pixel_iter {
                    data.extend_from_slice(pixel);
                    /* Add the alpha channel */
                    data.push(255);
                }
            } else {
                data.extend_from_slice(row);
            }
            data.extend(std::iter::repeat_n(PADDING_CLR, padding * 4));
        }

        data.extend(std::iter::repeat_n(
            PADDING_CLR,
            (2 * padding + pixmap.width() as usize) * padding * 4,
        ));

        let image = Self {
            id: ID.load(Ordering::Acquire),
            dimensions: (pixmap.width() as i32, pixmap.height() as i32),
            data,
        };

        ID.store(ID.load(Ordering::Acquire) + 1, Ordering::Release);
        image.transfer()?;
        Ok(image)
    }

    #[allow(dead_code)]
    pub fn id(&self) -> usize {
        self.id
    }

    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.data.len()
    }

    #[allow(dead_code)]
    pub fn check(&self) -> Result<(), String> {
        /* The first pixels should be invisible and therefore we have an easy if
         * the image still exists */
        terminal_graphics_display_image(self.id, 1, 1, (1, 1, 1, 1), 2, 2)?;
        Ok(())
    }

    pub fn display(&self, rect: DisplayRect) -> Result<bool, String> {
        /* `true` indicates that the image was actually displayed and was not
         * tried to be displayed outside of the viewport */
        let padding = *IMAGE_PADDING.get().unwrap() as f64;
        let Some(p) = placement(rect, self.dimensions, padding, &viewport()) else {
            return Ok(false);
        };

        terminal_graphics_display_image(self.id, p.col, p.row, p.crop, p.cols, p.rows)?;
        Ok(true)
    }

    pub fn transfer(&self) -> Result<(), String> {
        let padding = *IMAGE_PADDING.get().unwrap();

        terminal_graphics_transfer_bitmap(
            self.id,
            self.dimensions.0 as usize + 2 * padding,
            self.dimensions.1 as usize + 2 * padding,
            self.data.as_slice(),
            true,
        )?;

        Ok(())
    }
}

/* Where and how much of a page image to show: a 1-indexed cell position, the crop of
 * the padded bitmap in pixels, and the size in cells */
#[derive(Debug, PartialEq)]
pub struct Placement {
    pub col: usize,
    pub row: usize,
    pub crop: (usize, usize, usize, usize),
    pub cols: usize,
    pub rows: usize,
}

/* `rect` is the page in viewport pixels, `dimensions` the bitmap size without its
 * `padding`. The page is clipped to the viewport on every side and the shown area is
 * widened to whole cells, which the transparent padding absorbs. */
pub fn placement(
    rect: DisplayRect,
    dimensions: (i32, i32),
    padding: f64,
    vp: &Viewport,
) -> Option<Placement> {
    if rect.width <= 0 || rect.height <= 0 || dimensions.1 <= 0 {
        return None;
    }
    /* Bitmap pixels per displayed pixel */
    let k = dimensions.1 as f64 / rect.height as f64;
    let (rx, ry) = (rect.x as f64, rect.y as f64);

    let x0 = rx.max(0.0);
    let x1 = (rx + rect.width as f64).min(vp.width() as f64);
    let y0 = ry.max(0.0);
    let y1 = (ry + rect.height as f64).min(vp.height() as f64);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }

    let col0 = (x0 / vp.pxpercol).floor();
    let col1 = (x1 / vp.pxpercol).ceil().min(vp.cols as f64);
    let row0 = (y0 / vp.pxperrow).floor();
    let row1 = (y1 / vp.pxperrow).ceil().min(vp.rows as f64);
    if col1 <= col0 || row1 <= row0 {
        return None;
    }

    let full_w = dimensions.0 as f64 + 2.0 * padding;
    let full_h = dimensions.1 as f64 + 2.0 * padding;
    let sx0 = (padding + (col0 * vp.pxpercol - rx) * k).clamp(0.0, full_w);
    let sx1 = (padding + (col1 * vp.pxpercol - rx) * k).clamp(0.0, full_w);
    let sy0 = (padding + (row0 * vp.pxperrow - ry) * k).clamp(0.0, full_h);
    let sy1 = (padding + (row1 * vp.pxperrow - ry) * k).clamp(0.0, full_h);
    if sx1 <= sx0 || sy1 <= sy0 {
        return None;
    }

    Some(Placement {
        col: 1 + vp.col as usize + col0 as usize,
        row: 1 + vp.row as usize + row0 as usize,
        crop: (
            sx0 as usize,
            sy0 as usize,
            (sx1 - sx0) as usize,
            (sy1 - sy0) as usize,
        ),
        cols: (col1 - col0) as usize,
        rows: (row1 - row0) as usize,
    })
}

/* The reading ruler: a band laid over the page, shipped as an image of whole cells
 * that is transparent around the band, so kitty scales it like the pages */
#[derive(Clone, Debug, PartialEq)]
pub struct RulerPlacement {
    /* 1-indexed cell of the top-left corner, and the size in cells */
    pub col: usize,
    pub row: usize,
    pub cols: usize,
    pub rows: usize,
    /* Bitmap size, and the band inside it, in pixels */
    pub width: usize,
    pub height: usize,
    pub band: (usize, usize, usize, usize),
}

/* `band` is (x, y, width, height) in viewport pixels; the part inside the viewport is
 * shown */
pub fn ruler_placement(
    band: (f64, f64, f64, f64),
    vp: &Viewport,
) -> Option<RulerPlacement> {
    let (x, y, w, h) = band;
    let x0 = x.max(0.0);
    let x1 = (x + w).min(vp.width() as f64);
    let y0 = y.max(0.0);
    let y1 = (y + h).min(vp.height() as f64);
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    let col0 = (x0 / vp.pxpercol).floor();
    let col1 = (x1 / vp.pxpercol).ceil().min(vp.cols as f64);
    let row0 = (y0 / vp.pxperrow).floor();
    let row1 = (y1 / vp.pxperrow).ceil().min(vp.rows as f64);
    if col1 <= col0 || row1 <= row0 {
        return None;
    }
    let width = ((col1 - col0) * vp.pxpercol).round() as usize;
    let height = ((row1 - row0) * vp.pxperrow).round() as usize;
    let bx = ((x0 - col0 * vp.pxpercol).round() as usize).min(width);
    let by = ((y0 - row0 * vp.pxperrow).round() as usize).min(height);
    let bw = ((x1 - x0).round() as usize).min(width - bx);
    let bh = ((y1 - y0).round() as usize).min(height - by);
    Some(RulerPlacement {
        col: 1 + vp.col as usize + col0 as usize,
        row: 1 + vp.row as usize + row0 as usize,
        cols: (col1 - col0) as usize,
        rows: (row1 - row0) as usize,
        width,
        height,
        band: (bx, by, bw, bh),
    })
}

/* RGBA pixels of the ruler image: `color` inside the band, clear around it */
pub fn ruler_bitmap(p: &RulerPlacement, color: [u8; 4]) -> Vec<u8> {
    let mut data = vec![0u8; p.width * p.height * 4];
    let (bx, by, bw, bh) = p.band;
    for y in by..by + bh {
        for x in bx..bx + bw {
            let at = (y * p.width + x) * 4;
            data[at..at + 4].copy_from_slice(&color);
        }
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    /* 100x40 cells of 10x20 px: viewport 98x38 cells at (1, 1), 980x760 px */
    fn vp() -> Viewport {
        Viewport::from_size(100, 40, 1000, 800)
    }

    fn rect(x: i32, y: i32, width: i32, height: i32) -> DisplayRect {
        DisplayRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn ruler_band_sits_inside_whole_cells() {
        /* A band from x 15 to 215, y 45 to 63: cells 1..22 and rows 2..4 */
        let p = ruler_placement((15.0, 45.0, 200.0, 18.0), &vp()).unwrap();
        assert_eq!((p.col, p.row, p.cols, p.rows), (3, 4, 21, 2));
        assert_eq!((p.width, p.height), (210, 40));
        assert_eq!(p.band, (5, 5, 200, 18));
        let data = ruler_bitmap(&p, [1, 2, 3, 4]);
        assert_eq!(data.len(), 210 * 40 * 4);
        assert_eq!(&data[(5 * 210 + 5) * 4..][..4], &[1, 2, 3, 4]);
        assert_eq!(&data[..4], &[0, 0, 0, 0]);
    }

    #[test]
    fn ruler_band_outside_the_viewport_is_cut() {
        let p = ruler_placement((-20.0, -5.0, 100.0, 15.0), &vp()).unwrap();
        assert_eq!((p.col, p.row), (2, 2));
        assert_eq!(p.band, (0, 0, 80, 10));
        assert!(ruler_placement((0.0, 900.0, 100.0, 15.0), &vp()).is_none());
    }

    #[test]
    fn page_fully_inside_starts_after_the_border() {
        /* Bitmap at scale 1: 200x400 px page shown at 200x400 */
        let p = placement(rect(100, 40, 200, 400), (200, 400), 30.0, &vp()).unwrap();
        assert_eq!((p.col, p.row, p.cols, p.rows), (12, 4, 20, 20));
        assert_eq!(p.crop, (30, 30, 200, 400));
    }

    #[test]
    fn page_scrolled_above_the_top_is_cropped() {
        let p = placement(rect(0, -100, 200, 400), (200, 400), 30.0, &vp()).unwrap();
        assert_eq!((p.row, p.rows), (2, 15));
        assert_eq!(p.crop.1, 130);
        assert_eq!(p.crop.3, 300);
    }

    #[test]
    fn page_running_past_the_bottom_stops_at_the_border() {
        let p = placement(rect(0, 600, 200, 400), (200, 400), 30.0, &vp()).unwrap();
        /* Rows 30..38 of the viewport; nothing below row 38 */
        assert_eq!((p.row, p.rows), (32, 8));
        assert_eq!(p.crop.3, 160);
    }

    #[test]
    fn page_wider_than_the_viewport_is_clipped_on_the_right() {
        let p = placement(rect(-50, 0, 2000, 400), (2000, 400), 30.0, &vp()).unwrap();
        assert_eq!((p.col, p.cols), (2, 98));
        assert_eq!(p.crop.0, 80);
        assert_eq!(p.crop.2, 980);
    }

    #[test]
    fn crop_scales_with_render_precision() {
        /* Bitmap rendered at twice the displayed size */
        let p = placement(rect(0, 0, 200, 400), (400, 800), 30.0, &vp()).unwrap();
        assert_eq!(p.crop, (30, 30, 400, 800));
    }

    #[test]
    fn page_outside_the_viewport_is_skipped() {
        assert!(placement(rect(0, 800, 200, 400), (200, 400), 30.0, &vp()).is_none());
        assert!(placement(rect(0, -500, 200, 400), (200, 400), 30.0, &vp()).is_none());
        assert!(placement(rect(990, 0, 200, 400), (200, 400), 30.0, &vp()).is_none());
    }
}
