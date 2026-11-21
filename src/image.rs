use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    drivers::graphics::*,
    globals::RUNNING,
    ui::{viewport, Viewport},
    viewer::DisplayRect,
};

use mupdf::Pixmap;

/* A part of a page in page units from its top-left corner: x0, y0, x1, y1 */
pub type Region = [f32; 4];

/* Whether `outer` covers `inner`, give or take half a page unit */
pub fn covers(outer: Region, inner: Region) -> bool {
    const SLACK: f32 = 0.5;
    outer[0] <= inner[0] + SLACK
        && outer[1] <= inner[1] + SLACK
        && outer[2] >= inner[2] - SLACK
        && outer[3] >= inner[3] - SLACK
}

/* A page image that kitty holds: all of a page or a part of it. Only its size is
 * kept here; if kitty loses it, the page is rendered again. */
pub struct Image {
    id: usize,
    /* The bitmap's size without its padding */
    dimensions: (i32, i32),
    /* Clear pixels around it, which let a page be cut at whole cells */
    padding: usize,
    /* The scale it was asked for, in pixels per page unit */
    scale: f32,
    /* The part of the page it shows */
    region: Region,
    /* The most pixels it was allowed; fewer than its region at its scale means it was
     * drawn coarser, to stand in until the page is in view */
    max_pixels: f32,
    /* Bytes kitty keeps for it */
    bytes: usize,
}

impl Image {
    /* Sends `pixmap`, the part `region` of a page rendered for `scale` with at most
     * `max_pixels`, to kitty; it's RGBA, with `padding` clear pixels around */
    pub fn new(
        pixmap: &Pixmap,
        padding: usize,
        scale: f32,
        region: Region,
        max_pixels: f32,
    ) -> Result<Self, String> {
        static ID: AtomicUsize = AtomicUsize::new(1);

        let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
        let data = pixmap.samples();
        let id = ID.fetch_add(1, Ordering::AcqRel);
        terminal_graphics_transfer_bitmap(id, width, height, data)?;
        Ok(Self {
            id,
            dimensions: (
                width.saturating_sub(2 * padding) as i32,
                height.saturating_sub(2 * padding) as i32,
            ),
            padding,
            scale,
            region,
            max_pixels,
            bytes: data.len(),
        })
    }

    pub fn region(&self) -> Region {
        self.region
    }

    pub fn max_pixels(&self) -> f32 {
        self.max_pixels
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn size(&self) -> usize {
        self.bytes
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /* Where the image goes to show a page drawn at `rect` with `view_scale` viewport
     * pixels per page unit; `None` when its part of the page is outside the viewport */
    pub fn placement(&self, rect: DisplayRect, view_scale: f32) -> Option<Placement> {
        let [x0, y0, x1, y1] = self.region;
        let part = DisplayRect {
            x: rect.x + (x0 * view_scale).round() as i32,
            y: rect.y + (y0 * view_scale).round() as i32,
            width: ((x1 - x0) * view_scale).round() as i32,
            height: ((y1 - y0) * view_scale).round() as i32,
        };
        placement(part, self.dimensions, self.padding as f64, &viewport())
    }
}

/* Makes the outer `padding` pixels of an RGBA bitmap `width` wide and `height` high
 * clear */
pub fn clear_border(samples: &mut [u8], width: usize, height: usize, padding: usize) {
    let row = width * 4;
    let padding = padding.min(width / 2).min(height / 2);
    for (y, line) in samples.chunks_exact_mut(row).enumerate() {
        if y < padding || y >= height - padding {
            line.fill(0);
        } else {
            line[..padding * 4].fill(0);
            line[row - padding * 4..].fill(0);
        }
    }
}

/* Dark pages: white paper becomes `background` and black text becomes `text`, the
 * shades between them in proportion, and colors keep their hue around that, so a red
 * mark stays red. For a bitmap with `n` (3 or 4) bytes per pixel; alpha stays. */
pub fn recolor_pixels(samples: &mut [u8], n: usize, background: [u8; 3], text: [u8; 3]) {
    for px in samples.chunks_exact_mut(n) {
        let (r, g, b) = (px[0] as i32, px[1] as i32, px[2] as i32);
        /* Perceived lightness, 0 to 255 */
        let light = (54 * r + 183 * g + 19 * b) >> 8;
        let dark = 255 - light;
        for (c, value) in [r, g, b].into_iter().enumerate() {
            let (bg, fg) = (background[c] as i32, text[c] as i32);
            let base = bg + (fg - bg) * dark / 255;
            px[c] = (base + value - light).clamp(0, 255) as u8;
        }
    }
}

impl Drop for Image {
    /* Frees the copy kitty keeps; not on the way out, when the terminal is being
     * given back */
    fn drop(&mut self) {
        if RUNNING.load(Ordering::Acquire) {
            let _ = terminal_graphics_delete_image(self.id);
        }
    }
}

/* A page image in a frame: placed at `at`, or with `None` kept ready just outside the
 * view, which is placed out of sight to check that kitty still has it */
#[derive(Clone, Debug, PartialEq)]
pub struct PagePlacement {
    pub page: usize,
    pub image: usize,
    pub at: Option<Placement>,
    /* The coarse whole page under the sharp part */
    pub under: bool,
}

impl PagePlacement {
    pub fn place(&self) -> Result<(), String> {
        match &self.at {
            Some(p) => terminal_graphics_display_image(
                self.image, p.col, p.row, p.crop, p.cols, p.rows, self.under,
            ),
            /* The top-left padding pixel, which is clear */
            None => terminal_graphics_display_image(
                self.image,
                1,
                1,
                (1, 1, 1, 1),
                2,
                2,
                self.under,
            ),
        }
    }
}

/* Where and how much of a page image to show: a 1-indexed cell position, the crop of
 * the padded bitmap in pixels, and the size in cells */
#[derive(Clone, Debug, PartialEq)]
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
    fn the_border_is_clear_and_the_page_stays() {
        let mut px = vec![255u8; 4 * 3 * 4];
        clear_border(&mut px, 4, 3, 1);
        let opaque: Vec<usize> = (0..12).filter(|i| px[i * 4 + 3] == 255).collect();
        /* Of 4 x 3 pixels, the middle row's inner two */
        assert_eq!(opaque, [5, 6]);
    }

    #[test]
    fn dark_pages_take_the_theme_colors_and_keep_hues() {
        let (bg, fg) = ([30, 32, 48], [220, 220, 230]);
        /* White paper, black text, a gray in between */
        let mut px = [255, 255, 255, 0, 0, 0, 128, 128, 128];
        recolor_pixels(&mut px, 3, bg, fg);
        assert_eq!(&px[..3], &bg);
        assert_eq!(&px[3..6], &fg);
        assert!(px[6] > bg[0] && px[6] < fg[0]);
        /* A red mark stays reddest in red; alpha is kept */
        let mut red = [220, 40, 40, 77];
        recolor_pixels(&mut red, 4, bg, fg);
        assert!(red[0] > red[1] + 100 && red[0] > red[2] + 100, "{:?}", red);
        assert_eq!(red[3], 77);
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
