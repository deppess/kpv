use core::f32;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, RwLock},
};

use crossbeam_channel::{unbounded, Receiver, Sender};
use crossterm::event::MouseEvent;
use mupdf::{link::LinkDestination, DestinationKind, Link};

use crate::{jumps::Spot, labels, threads::renderer::*, ui::viewport, Image, CONFIG};

#[derive(Clone, Copy, Debug)]
pub struct DisplayRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub trait Point {
    fn x(&self) -> i32;
    fn y(&self) -> i32;
}

pub trait Rect {
    fn x(&self) -> i32;
    fn y(&self) -> i32;
    fn width(&self) -> i32;
    fn height(&self) -> i32;
}

fn rect_point_intersect<R: Rect, P: Point>(rect: &R, point: &P) -> bool {
    let rect_x = rect.x();
    let rect_y = rect.y();
    let rect_width = rect.width();
    let rect_height = rect.height();

    let point_x = point.x();
    let point_y = point.y();

    let cr1 = point_x >= rect_x && point_x <= (rect_x + rect_width);
    let cr2 = point_y >= rect_y && point_y <= (rect_y + rect_height);

    cr1 && cr2
}

impl Point for MouseEvent {
    fn x(&self) -> i32 {
        self.column as i32
    }

    fn y(&self) -> i32 {
        self.row as i32
    }
}

impl Point for (i32, i32) {
    fn x(&self) -> i32 {
        self.0
    }

    fn y(&self) -> i32 {
        self.1
    }
}

impl Rect for DisplayRect {
    fn x(&self) -> i32 {
        self.x
    }

    fn y(&self) -> i32 {
        self.y
    }

    fn width(&self) -> i32 {
        self.width
    }

    fn height(&self) -> i32 {
        self.height
    }
}

impl Rect for mupdf::rect::Rect {
    fn x(&self) -> i32 {
        self.x0 as i32
    }

    fn y(&self) -> i32 {
        self.y0 as i32
    }

    fn width(&self) -> i32 {
        (self.x1 - self.x0) as i32
    }

    fn height(&self) -> i32 {
        (self.y1 - self.y0) as i32
    }
}

/* A link target lands a little below the top edge, so its heading isn't cut off */
const DEST_AT: f32 = 0.03;
/* How far down the viewport the spot being read is taken to be */
const READING_AT: f32 = 0.25;

/* The top edge a destination names, in page units from the page's top */
pub fn dest_top(kind: DestinationKind) -> Option<f32> {
    let top = match kind {
        DestinationKind::XYZ { top, .. } => top?,
        DestinationKind::FitH { top } | DestinationKind::FitBH { top } => top,
        DestinationKind::FitR { bottom, top, .. } => bottom.min(top),
        _ => return None,
    };
    top.is_finite().then_some(top.max(0.0))
}

pub struct Viewer {
    scale: f32,
    page_first: usize,  /* The first page in the view */
    page_view: usize,   /* The page in the middle */
    offset: (f32, f32), /* Offset is given in page size units ≈ pixels */
    fit_scale: f32,     /* Scale at which the widest page fills the viewport */

    max_width: f32,
    cumulative_heights: Vec<f32>,
    widths: Vec<f32>,
    links: Vec<Vec<Link>>,
    /* The page labels in the PDF, and the ones shown: those, or for a PDF without
     * any, the ones `page_offset` gives */
    pdf_labels: Vec<String>,
    labels: Vec<String>,
    page_offset: i32,

    pub images: HashMap<usize, Arc<RwLock<Image>>>,
    invalidated: HashMap<usize, ()>,
    scheduled4render: HashMap<usize, ()>,
    memory_used: usize,
    last_rendered: VecDeque<usize>,
    /* Failed placements in a row, per page */
    failures: HashMap<usize, usize>,

    sender_rerender: Sender<()>,
}

impl Viewer {
    pub fn new() -> (Self, Receiver<()>) {
        let (sender_rerender, receiver_rerender) = unbounded::<()>();
        (
            Self {
                scale: 1.0,
                page_first: 0,
                page_view: 0,
                offset: (0.0f32, 0.0f32),
                fit_scale: 1.0,
                max_width: -f32::INFINITY,
                cumulative_heights: Vec::new(),
                widths: Vec::new(),
                links: Vec::new(),
                pdf_labels: Vec::new(),
                labels: Vec::new(),
                page_offset: 0,
                images: HashMap::new(),
                invalidated: HashMap::new(),
                scheduled4render: HashMap::new(),
                memory_used: 0,
                last_rendered: VecDeque::new(),
                failures: HashMap::new(),
                sender_rerender,
            },
            receiver_rerender,
        )
    }

    pub fn is_uninit(&self) -> bool {
        self.cumulative_heights.is_empty() && self.max_width == -f32::INFINITY
    }

    pub fn update_metadata(
        &mut self,
        max_width: f32,
        cumulative_heights: &[f32],
        widths: &[f32],
        links: &[Vec<Link>],
        labels: Vec<String>,
    ) {
        self.max_width = max_width;
        self.cumulative_heights = cumulative_heights.to_owned();
        self.widths = widths.to_owned();
        self.links = links.to_owned();
        self.pdf_labels = labels;
        self.apply_labels();
    }

    fn apply_labels(&mut self) {
        self.labels = if self.page_offset != 0 && labels::is_plain(&self.pdf_labels) {
            labels::with_offset(self.pdf_labels.len(), self.page_offset)
        } else {
            self.pdf_labels.clone()
        };
    }

    /* Numbers a scan whose printed page 1 is `offset` pages into the file. A PDF with
     * its own page labels keeps them. */
    pub fn set_page_offset(&mut self, offset: i32) -> Result<(), String> {
        if offset != 0 && !labels::is_plain(&self.pdf_labels) {
            return Err("This PDF has its own page numbers".to_owned());
        }
        self.page_offset = offset;
        self.apply_labels();
        Ok(())
    }

    pub fn page_offset(&self) -> i32 {
        self.page_offset
    }

    fn page_top(&self, page: usize) -> f32 {
        if page == 0 {
            0.0
        } else {
            self.cumulative_heights[page - 1]
        }
    }

    /* Where the viewport is, in a form that survives other terminal sizes: the page at
     * its top, how far down that page it starts (0 to 1), and the zoom relative to
     * fitting the page width */
    pub fn position(&self) -> (usize, f32, f32) {
        if self.cumulative_heights.is_empty() {
            return (0, 0.0, 1.0);
        }
        let page = self.page_first.min(self.pages() - 1);
        let height = self.page_height(page).unwrap_or(1.0).max(1.0);
        let within = ((self.offset.1 - self.page_top(page)) / height).clamp(0.0, 1.0);
        (page, within, self.scale / self.fit_scale)
    }

    /* Where `page` is drawn, in viewport pixels, when it's on screen */
    pub fn page_rect(&self, page: usize) -> Option<DisplayRect> {
        self.calculate_display_bounds()
            .into_iter()
            .find(|(p, _)| *p == page)
            .map(|(_, rect)| rect)
    }

    /* Viewport pixels per page unit */
    pub fn pixels_per_unit(&self) -> f32 {
        self.scale
    }

    /* The page at the viewport's top edge and the spot on it there */
    pub fn top_of_view(&self) -> (usize, f32) {
        if self.cumulative_heights.is_empty() {
            return (0, 0.0);
        }
        let page = self.page_first.min(self.pages() - 1);
        (page, self.offset.1 - self.page_top(page))
    }

    pub fn spot(&self) -> Spot {
        let (page, within, _) = self.position();
        Spot { page, within }
    }

    /* Goes to a `spot()`, keeping the zoom */
    pub fn go_to_spot(&mut self, spot: Spot) {
        self.restore(spot.page, spot.within, self.scale / self.fit_scale);
    }

    /* Goes back to a `position()` */
    pub fn restore(&mut self, page: usize, within: f32, zoom: f32) {
        if self.cumulative_heights.is_empty() {
            return;
        }
        if zoom.is_finite() && zoom > 0.0 {
            self.scale = self.fit_scale * zoom;
        }
        self.center_viewer();
        let page = page.min(self.pages() - 1);
        let height = self.page_height(page).unwrap_or(0.0);
        self.offset.1 = self.page_top(page) + within.clamp(0.0, 1.0) * height;
        self.bound_viewer();
    }

    /* The printed label of `page` */
    pub fn page_label(&self, page: usize) -> String {
        self.labels
            .get(page)
            .cloned()
            .unwrap_or_else(|| (page + 1).to_string())
    }

    /* The page printed as `label` */
    pub fn find_label(&self, label: &str) -> Option<usize> {
        labels::find(&self.labels, label)
    }

    /* `xiv (14/412)` or `45/412` for the status bar */
    pub fn page_status(&self) -> String {
        labels::status(&self.labels, self.page_view, self.pages())
    }

    pub fn invalidate_registry(&mut self) {
        self.invalidated.clear();
        self.scheduled4render.clear();
        for k in self.images.keys() {
            self.invalidated.insert(*k, ());
        }
    }

    pub fn scroll(&mut self, amount: (f32, f32)) {
        self.offset.0 += amount.0;
        self.offset.1 += amount.1;
        self.bound_viewer();
    }

    pub fn pages(&self) -> usize {
        self.cumulative_heights.len()
    }

    pub fn jump(&mut self, page: usize) -> Result<(), String> {
        if self.cumulative_heights.is_empty() {
            return Err("The document has no pages".to_owned());
        }
        let res = if page >= self.cumulative_heights.len() {
            Err("Given page number is larger than the number of pages".to_owned())
        } else {
            Ok(())
        };

        self.page_first = usize::min(page, self.cumulative_heights.len() - 1);

        if page == 0 {
            self.offset.1 = 0.0f32;
        } else {
            self.offset.1 = self.cumulative_heights[self.page_first - 1] + 1.0;
        }
        self.bound_viewer();

        res
    }

    pub fn scale(&mut self, scale: f32) {
        self.scale += scale;
        self.bound_viewer();
    }

    /* Zoom relative to fitting the page width, which is 100% */
    pub fn zoom_percent(&self) -> u32 {
        (self.scale / self.fit_scale * 100.0).round() as u32
    }

    /* Scrolls so that `y` (page units from the top of `page`) sits `at` (a fraction
     * of the viewport height) from the top */
    pub fn jump_to(&mut self, page: usize, y: f32, at: f32) {
        if page >= self.cumulative_heights.len() {
            return;
        }
        let page_top = if page == 0 {
            0.0
        } else {
            self.cumulative_heights[page - 1]
        };
        self.offset.1 = page_top + y - viewport().height() * at / self.scale;
        self.bound_viewer();
    }

    /* Goes to where a link or outline entry points: the exact spot when it names one,
     * otherwise the top of its page */
    pub fn jump_dest(&mut self, dest: &LinkDestination) {
        self.jump_target(dest.loc.page_number as usize, dest_top(dest.kind));
    }

    /* Goes to `top` (page units from the page's top) on `page`, or the page's top */
    pub fn jump_target(&mut self, page: usize, top: Option<f32>) {
        match top {
            Some(y) if page < self.pages() => self.jump_to(page, y, DEST_AT),
            _ => {
                let _ = self.jump(page);
            }
        }
    }

    /* Where reading is: the page and the spot on it (page units from its top) a little
     * below the viewport's top edge, past where a jump puts a heading */
    pub fn reading_spot(&self) -> (usize, f32) {
        if self.cumulative_heights.is_empty() {
            return (0, 0.0);
        }
        let at = self.offset.1 + viewport().height() * READING_AT / self.scale;
        let page = self.offset2page(at).min(self.pages() - 1);
        let page_top = if page == 0 {
            0.0
        } else {
            self.cumulative_heights[page - 1]
        };
        (page, at - page_top)
    }

    pub fn page_first(&self) -> usize {
        self.page_first
    }

    pub fn page_view(&self) -> usize {
        self.page_view
    }

    pub fn offset(&self) -> (f32, f32) {
        self.offset
    }

    /* ============================= Calculation methods ============================= */
    fn offset2page(&self, offset: f32) -> usize {
        /* Update page index by performing binary search */
        let res = self.cumulative_heights.binary_search_by(|x: &f32| {
            x.partial_cmp(&offset)
                .expect("NaN value found in cumulative height vector")
        });

        match res {
            Ok(x) => x,
            Err(x) => x,
        }
    }

    pub fn bound_viewer(&mut self) {
        let config = CONFIG.get().unwrap();

        let vp = viewport();
        let (vp_width, vp_height) = (vp.width(), vp.height());

        self.scale = f32::max(self.scale, config.viewer.scale_min);
        self.offset.0 = f32::max(
            self.offset.0,
            f32::min(0.0f32, vp_width - self.max_width * self.scale),
        );
        self.offset.0 = f32::min(
            self.offset.0,
            f32::max(0.0f32, vp_width - self.max_width * self.scale),
        );
        let max_yoffset = f32::max(
            -10.0f32,
            self.cumulative_heights.last().unwrap_or(&0.0f32) - vp_height / self.scale,
        );
        self.offset.1 = f32::max(self.offset.1, -10.0f32);
        self.offset.1 = f32::min(self.offset.1, max_yoffset);

        self.page_first = self.offset2page(self.offset.1);
        self.page_view = self.offset2page(self.offset.1 + vp_height * 0.5 / self.scale);
        let mut min_page = 0;
        if !self.cumulative_heights.is_empty() {
            min_page = self.cumulative_heights.len() - 1;
        }
        self.page_view = usize::min(self.page_view, min_page);
    }

    pub fn scale_page2terminal(&mut self) {
        let factor = viewport().width() / (self.max_width * self.scale);
        self.scale *= factor;
        self.fit_scale = self.scale;
        self.bound_viewer();
    }

    pub fn center_viewer(&mut self) {
        self.offset.0 = viewport().width() * 0.5 - self.max_width * self.scale * 0.5;
    }

    pub fn page_height(&self, page: usize) -> Result<f32, String> {
        let page_prev_height = if page > 0 {
            *self.cumulative_heights.get(page - 1).unwrap_or(&0.0f32)
        } else {
            0.0f32
        };

        let page_height = *self.cumulative_heights.get(page).ok_or(format!(
            "Wrong page index provided when retrieving page height, index: {}",
            page
        ))? - page_prev_height;

        Ok(page_height)
    }

    pub fn page_width(&self, page: usize) -> Result<f32, String> {
        self.widths
            .get(page)
            .ok_or(format!("Could not get page {} width", page))
            .copied()
    }

    fn calculate_display_bounds(&self) -> Vec<(usize, DisplayRect)> {
        /* Calculated bounds to display the pages in the temrinal */
        let mut bounds = Vec::new();
        /* Current viewport height */
        let terminal_height = viewport().height();
        /* Bottom margin */
        let margin_bottom = CONFIG.get().unwrap().viewer.margin_bottom;
        /* Number of pages */
        let pages_num = self.cumulative_heights.len();
        /* The index of the first rendered page */
        let mut page_index = self.page_first();

        if pages_num <= page_index {
            return bounds;
        }

        let mut page_offset = self.offset().1;
        page_offset -= if page_index == 0 {
            0.0f32
        } else {
            self.cumulative_heights[page_index - 1]
        };
        let mut displayed_offset = -page_offset * self.scale;

        /* Cumulative displayed page height */
        while displayed_offset < terminal_height && page_index < pages_num {
            let height = ((self
                .page_height(page_index)
                .expect("Could not retrieve page height"))
                - margin_bottom)
                * self.scale;

            let width = self.page_width(page_index).unwrap() * self.scale;

            bounds.push((
                page_index,
                DisplayRect {
                    x: self.offset().0 as i32,
                    y: displayed_offset as i32,
                    width: width as i32,
                    height: height as i32,
                },
            ));

            displayed_offset += height + margin_bottom * self.scale;
            page_index += 1;
        }

        bounds
    }

    /* ================================ Miscellaneous ================================ */
    pub fn intersect_link(&self, mouse: MouseEvent) -> Option<Link> {
        let vp = viewport();
        let mouse = (
            mouse.column as i32 - vp.x() as i32,
            mouse.row as i32 - vp.y() as i32,
        );
        let mut intersected_page = None;
        let mut intersected_rect = None;
        for bound in self.calculate_display_bounds() {
            let (page, rect) = bound;
            if rect_point_intersect(&rect, &mouse) {
                intersected_page = Some(page);
                intersected_rect = Some(rect);
                break;
            }
        }

        intersected_page?;

        let page_point = (
            ((mouse.0 - intersected_rect.unwrap().x) as f32 / self.scale) as i32,
            ((mouse.1 - intersected_rect.unwrap().y) as f32 / self.scale) as i32,
        );
        let mut intersected_link = None;
        let links = &self.links[intersected_page.unwrap()];
        for link in links.iter() {
            if rect_point_intersect(&link.bounds, &page_point) {
                intersected_link = Some(link);
                break;
            }
        }

        intersected_link?;

        Some(intersected_link.unwrap().clone())
    }

    /* What a hovered link points to, for the status bar */
    pub fn link_hint(&self, link: &Link) -> Option<String> {
        if !CONFIG.get().unwrap().viewer.uri_hint.enabled {
            return None;
        }
        if link.uri.starts_with('#') {
            let page = link.dest.as_ref().map(|d| d.loc.page_number).unwrap_or(0);
            Some(format!("→ page {}", self.page_label(page as usize)))
        } else {
            Some(format!("→ {}", link.uri))
        }
    }

    pub fn handle_image(&mut self, page: usize, image: Option<Arc<RwLock<Image>>>) {
        macro_rules! remove_image {
            ($page:expr) => {
                self.memory_used -= self.images[&$page].read().unwrap().size();
                self.images.remove(&$page);
                self.invalidated.remove(&$page);
            };
        }

        let config = CONFIG.get().unwrap();

        if image.is_none() {
            remove_image!(page);
            return;
        }

        if self.invalidated.contains_key(&page) {
            remove_image!(page);
        }

        let image_unwrapped = image.unwrap();
        self.memory_used += image_unwrapped.read().unwrap().size();
        self.last_rendered.push_back(page);

        self.images.insert(page, image_unwrapped);
        self.failures.remove(&page);
        self.scheduled4render.remove(&page);

        while self.memory_used >= config.viewer.memory_limit {
            let page2remove = self.last_rendered.pop_front().unwrap();
            if !self.images.contains_key(&page2remove) {
                continue;
            }
            remove_image!(page2remove);
        }
    }

    fn load_or_display(
        &mut self,
        page: usize,
        rect: DisplayRect,
        preload: bool,
        renderer: &Renderer,
    ) -> Option<usize> {
        if (!self.images.contains_key(&page) || self.invalidated.contains_key(&page))
            && !self.scheduled4render.contains_key(&page)
        {
            let res = renderer.send_action(RendererAction::Display(page));
            if res.is_ok() {
                self.scheduled4render.insert(page, ());
            }

            return None;
        }

        if self.images.contains_key(&page) {
            let image = self.images[&page].read().unwrap();
            if preload {
                image.check().unwrap();
                return Some(page);
            } else {
                let has_displayed = image.display(rect).unwrap();

                if has_displayed {
                    return Some(page);
                }
            }
        }

        None
    }

    /* Displays the pages based on the internal state of the offset.
     * Calculates how many pages should be rendered based on the terminal size */
    pub fn display_pages(&mut self, renderer: &Renderer) -> Result<Vec<usize>, String> {
        let config = CONFIG.get().unwrap();
        let preloaded = config.viewer.pages_preloaded;

        /* Track what images have been actually displayed on the screen to
         * later check if there occured errors during the display */
        let mut displayed = Vec::new();
        let none_rect = DisplayRect {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        };

        /* The index of the first rendered page */
        let mut page_index = self.page_first();
        if self.cumulative_heights.len() <= page_index {
            return Ok(displayed);
        }

        /* Preload N pages before the first displayed page to avoid flickering pages */
        for i in 0..usize::min(preloaded, page_index) {
            let r = self.load_or_display(page_index - 1 - i, none_rect, true, renderer);
            if let Some(page) = r {
                displayed.push(page);
            }
        }

        for (page, rect) in self.calculate_display_bounds() {
            let r = self.load_or_display(page, rect, false, renderer);
            if let Some(page) = r {
                displayed.push(page);
            }
            page_index += 1;
        }

        /* Preload N pages after the last displayed page */
        for _ in 0..usize::min(preloaded, self.cumulative_heights.len() - page_index) {
            let r = self.load_or_display(page_index, none_rect, true, renderer);
            if let Some(page) = r {
                displayed.push(page);
            }
            page_index += 1;
        }

        Ok(displayed)
    }

    /* Sends a page image again after kitty failed to place it. Gives up after a few
     * failures in a row so a terminal that keeps refusing can't cause an endless
     * redraw loop; `false` means it gave up. */
    pub fn schedule_transfer(&mut self, page: usize) -> bool {
        const MAX_RETRIES: usize = 3;

        let failures = self.failures.entry(page).or_insert(0);
        *failures += 1;
        if *failures > MAX_RETRIES {
            return false;
        }
        let Some(image) = self.images.get(&page).cloned() else {
            return true;
        };
        let _ = image.read().unwrap().transfer();
        let _ = self.sender_rerender.send(());
        true
    }

    pub fn placement_ok(&mut self, page: usize) {
        self.failures.remove(&page);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_name_their_top_edge() {
        let xyz = DestinationKind::XYZ {
            left: Some(10.0),
            top: Some(120.0),
            zoom: None,
        };
        assert_eq!(dest_top(xyz), Some(120.0));
        assert_eq!(dest_top(DestinationKind::FitH { top: 40.0 }), Some(40.0));
        let rect = DestinationKind::FitR {
            left: 0.0,
            bottom: 300.0,
            right: 10.0,
            top: 200.0,
        };
        assert_eq!(dest_top(rect), Some(200.0));
        assert_eq!(dest_top(DestinationKind::Fit), None);
        let unset = DestinationKind::XYZ {
            left: None,
            top: Some(f32::NAN),
            zoom: None,
        };
        assert_eq!(dest_top(unset), None);
    }
}
