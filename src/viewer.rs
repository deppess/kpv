use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{atomic::Ordering, Arc, RwLock},
};

use crossterm::event::MouseEvent;
use mupdf::{link::LinkDestination, DestinationKind, Link};

use crate::{
    globals::{CONFIG, RENDER_SCALE},
    image::{covers, Image, PagePlacement, Region},
    jumps::Spot,
    labels,
    threads::renderer::*,
    ui::viewport,
};

/* The smallest zoom, in pixels per page unit; the image padding is worked out for it */
pub const SCALE_MIN: f32 = 0.2;
/* How much memory the page images may take in kitty before the oldest are dropped,
 * below the 320 MB kitty keeps for images */
const MEMORY_LIMIT: usize = 280 << 20;
/* Pages kept ready before and after the ones in view */
const PAGES_PRELOADED: usize = 2;

#[derive(Clone, Copy, Debug)]
pub struct DisplayRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl DisplayRect {
    fn contains(&self, (x, y): (i32, i32)) -> bool {
        (self.x..=self.x + self.width).contains(&x)
            && (self.y..=self.y + self.height).contains(&y)
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
    /* Coarse whole pages kept under a sharp part, so that past its edge the page shows
     * rather than nothing until the next part arrives */
    under: HashMap<usize, Arc<RwLock<Image>>>,
    invalidated: HashSet<usize>,
    /* What was last asked of the renderer for each page */
    scheduled4render: HashMap<usize, Request>,
    /* The pages the last frame wanted, which are never dropped to make room */
    wanted: HashSet<usize>,
    memory_used: usize,
    /* Pages with an image, the one that came longest ago first */
    last_rendered: VecDeque<usize>,
    /* Failed placements in a row, per page */
    failures: HashMap<usize, usize>,
}

impl Viewer {
    pub fn new() -> Self {
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
            under: HashMap::new(),
            invalidated: HashSet::new(),
            scheduled4render: HashMap::new(),
            wanted: HashSet::new(),
            memory_used: 0,
            last_rendered: VecDeque::new(),
            failures: HashMap::new(),
        }
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

    /* The PDF doesn't say how its pages are numbered */
    pub fn has_plain_labels(&self) -> bool {
        labels::is_plain(&self.pdf_labels)
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

    /* Sets the zoom relative to fitting the page width, keeping the spot at the top of
     * the view */
    pub fn set_zoom(&mut self, zoom: f32) {
        let (page, within, _) = self.position();
        self.restore(page, within, zoom);
    }

    /* Zooms so the page being read fits the viewport whole, and shows all of it */
    pub fn fit_page(&mut self) {
        if self.cumulative_heights.is_empty() {
            return;
        }
        let page = self.page_view;
        let margin = CONFIG.get().unwrap().viewer.page_gap;
        let height = (self.page_height(page).unwrap_or(1.0) - margin).max(1.0);
        let width = self.page_width(page).unwrap_or(1.0).max(1.0);
        let vp = viewport();
        let scale = (vp.height() / height).min(vp.width() / width);
        self.restore(page, 0.0, scale / self.fit_scale);
    }

    /* Scrolls by `pages` viewport heights; negative goes up */
    pub fn scroll_screens(&mut self, pages: f32) {
        let amount = viewport().height() * pages / self.scale;
        self.scroll((0.0, amount));
    }

    /* Viewport pixels per page unit */
    pub fn pixels_per_unit(&self) -> f32 {
        self.scale
    }

    /* The scale pages are rendered at: one image pixel to a screen pixel at this zoom,
     * times the `resolution` setting */
    pub fn render_scale(&self) -> f32 {
        self.scale * CONFIG.get().unwrap().viewer.resolution
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

    /* The images of `pages` are drawn again */
    pub fn invalidate_pages(&mut self, pages: &[usize]) {
        for page in pages {
            if self.images.contains_key(page) {
                self.invalidated.insert(*page);
                self.scheduled4render.remove(page);
            }
        }
    }

    pub fn invalidate_registry(&mut self) {
        self.invalidated.clear();
        self.scheduled4render.clear();
        for k in self.images.keys() {
            self.invalidated.insert(*k);
        }
    }

    /* Moves the view by `amount` page units: right and down for positive values. The
     * horizontal offset is where the page's left edge is, in pixels, so it moves the
     * other way. */
    pub fn scroll(&mut self, amount: (f32, f32)) {
        self.offset.0 -= amount.0 * self.scale;
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

    /* Zooms in (or out, when negative) by `percent` of the page width */
    pub fn zoom_by(&mut self, percent: f32) {
        self.scale += self.fit_scale * percent / 100.0;
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
        self.offset.1 = self.page_top(page) + y - viewport().height() * at / self.scale;
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
        (page, at - self.page_top(page))
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
        /* total_cmp: a page with broken bounds must not stop the viewer */
        let res = self
            .cumulative_heights
            .binary_search_by(|x: &f32| x.total_cmp(&offset));

        match res {
            Ok(x) => x,
            Err(x) => x,
        }
    }

    pub fn bound_viewer(&mut self) {
        let vp = viewport();
        let (vp_width, vp_height) = (vp.width(), vp.height());

        self.scale = f32::max(self.scale, SCALE_MIN);
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

    /* How far right the view is over pages wider than it: 0 at their left edge, 1 at
     * their right */
    pub fn left(&self) -> f32 {
        let (width, view) = (self.max_width * self.scale, viewport().width());
        if width <= view {
            return 0.0;
        }
        (-self.offset.0 / (width - view)).clamp(0.0, 1.0)
    }

    pub fn set_left(&mut self, left: f32) {
        let (width, view) = (self.max_width * self.scale, viewport().width());
        if width > view && left.is_finite() {
            self.offset.0 = -left.clamp(0.0, 1.0) * (width - view);
            self.bound_viewer();
        }
    }

    /* Fits the page width to the viewport again after the viewport changed width,
     * keeping the zoom and the place being read */
    pub fn refit(&mut self) {
        if self.cumulative_heights.is_empty() || self.max_width <= 0.0 {
            return;
        }
        let (page, within, zoom) = self.position();
        self.fit_scale = viewport().width() / self.max_width;
        self.restore(page, within, zoom);
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
        let margin_bottom = CONFIG.get().unwrap().viewer.page_gap;
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
            let (Ok(height), Ok(width)) =
                (self.page_height(page_index), self.page_width(page_index))
            else {
                break;
            };
            let height = (height - margin_bottom) * self.scale;
            let width = width * self.scale;

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
        let (page, rect) = self
            .calculate_display_bounds()
            .into_iter()
            .find(|(_, rect)| rect.contains(mouse))?;

        let (x, y) = (
            (mouse.0 - rect.x) as f32 / self.scale,
            (mouse.1 - rect.y) as f32 / self.scale,
        );
        self.links
            .get(page)?
            .iter()
            .find(|link| {
                let b = &link.bounds;
                (b.x0..=b.x1).contains(&x) && (b.y0..=b.y1).contains(&y)
            })
            .cloned()
    }

    /* What a hovered link points to, for the status bar */
    pub fn link_hint(&self, link: &Link) -> Option<String> {
        if !CONFIG.get().unwrap().viewer.link_hints {
            return None;
        }
        if link.uri.starts_with('#') {
            let page = link.dest.as_ref().map(|d| d.loc.page_number).unwrap_or(0);
            Some(format!("→ page {}", self.page_label(page as usize)))
        } else {
            Some(format!("→ {}", link.uri))
        }
    }

    /* Stores a rendered page, or with `None` forgets it. The oldest pages are dropped
     * while the images take more memory than allowed. */
    pub fn handle_image(&mut self, page: usize, image: Option<Arc<RwLock<Image>>>) {
        let Some(image) = image else {
            self.remove_image(page);
            return;
        };
        /* A sharp part of a page arriving over a whole one keeps the whole one under
         * it; a whole one arriving needs nothing under it */
        let whole = self.whole_page(page);
        let is_whole = |i: &Arc<RwLock<Image>>| covers(i.read().unwrap().region(), whole);
        let stale = self.invalidated.remove(&page);
        let new_whole = is_whole(&image);
        if let Some(old) = self.images.remove(&page) {
            if !new_whole && !stale && is_whole(&old) {
                if let Some(previous) = self.under.insert(page, old) {
                    self.forget(previous);
                }
            } else {
                self.forget(old);
            }
        }
        if new_whole || stale {
            if let Some(under) = self.under.remove(&page) {
                self.forget(under);
            }
        }

        self.memory_used += image.read().unwrap().size();
        self.last_rendered.retain(|p| *p != page);
        self.last_rendered.push_back(page);

        /* An image for an older request stands in, but the newer request stays */
        let (scale, region) = {
            let image = image.read().unwrap();
            (image.scale(), image.region())
        };
        if self.scheduled4render.get(&page).is_some_and(|asked| {
            same_scale(asked.scale, scale) && covers(region, asked.region)
        }) {
            self.scheduled4render.remove(&page);
        }
        self.images.insert(page, image);
        self.failures.remove(&page);

        /* The oldest images go first. Pages on screen or kept ready stay: dropping
         * them would only have them rendered again. */
        while self.memory_used >= MEMORY_LIMIT {
            let Some(i) = self
                .last_rendered
                .iter()
                .position(|p| *p != page && !self.wanted.contains(p))
            else {
                break;
            };
            if let Some(oldest) = self.last_rendered.remove(i) {
                self.remove_image(oldest);
            }
        }
    }

    fn remove_image(&mut self, page: usize) {
        self.last_rendered.retain(|p| *p != page);
        for old in [self.images.remove(&page), self.under.remove(&page)]
            .into_iter()
            .flatten()
        {
            self.forget(old);
        }
        self.invalidated.remove(&page);
    }

    fn forget(&mut self, image: Arc<RwLock<Image>>) {
        self.memory_used = self
            .memory_used
            .saturating_sub(image.read().unwrap().size());
    }

    /* All of a page, in page units */
    fn whole_page(&self, page: usize) -> Region {
        let gap = CONFIG.get().unwrap().viewer.page_gap;
        [
            0.0,
            0.0,
            self.page_width(page).unwrap_or(0.0),
            (self.page_height(page).unwrap_or(0.0) - gap).max(0.0),
        ]
    }

    /* Asks the renderer for the page when its image is missing, out of date, or
     * doesn't cover the part in view sharply, and plans its image: at `rect`, or with
     * `None` kept ready out of sight. Until a new image arrives the old one stands in,
     * so a page never flashes blank. */
    fn plan_page(
        &mut self,
        page: usize,
        rect: Option<DisplayRect>,
        renderer: &Renderer,
    ) -> Vec<PagePlacement> {
        let scale = self.render_scale();
        let size = (
            self.page_width(page).unwrap_or(0.0),
            (self.page_height(page).unwrap_or(0.0)
                - CONFIG.get().unwrap().viewer.page_gap)
                .max(0.0),
        );
        let vp = viewport();
        let want = wanted(size, rect, self.scale, (vp.width(), vp.height()), scale);

        /* An image that covers what would be asked for now is always enough, so a
         * page is never asked for over and over */
        let enough = |region: Region, max_pixels: f32, asked_scale: f32| {
            same_scale(asked_scale, scale)
                && want.need.is_none_or(|need| {
                    max_pixels >= want.max_pixels
                        && (covers(region, need) || covers(region, want.region))
                })
        };
        let stale = match self.images.get(&page) {
            None => true,
            Some(image) => {
                let image = image.read().unwrap();
                self.invalidated.contains(&page)
                    || !enough(image.region(), image.max_pixels(), image.scale())
            }
        };
        let asked = self
            .scheduled4render
            .get(&page)
            .is_some_and(|r| enough(r.region, r.max_pixels, r.scale));
        let request = Request {
            page,
            scale,
            region: want.region,
            max_pixels: want.max_pixels,
        };
        if stale
            && !asked
            && renderer
                .send_action(RendererAction::Display(request))
                .is_ok()
        {
            self.scheduled4render.insert(page, request);
        }

        let mut placed = Vec::new();
        if let (Some(rect), Some(under)) = (rect, self.under.get(&page)) {
            let under = under.read().unwrap();
            if let Some(at) = under.placement(rect, self.scale) {
                placed.push(PagePlacement {
                    page,
                    image: under.id(),
                    at: Some(at),
                    under: true,
                });
            }
        }
        let Some(image) = self.images.get(&page) else {
            return placed;
        };
        let image = image.read().unwrap();
        let at = match rect {
            Some(rect) => image.placement(rect, self.scale),
            None => None,
        };
        if rect.is_none() || at.is_some() {
            placed.push(PagePlacement {
                page,
                image: image.id(),
                at,
                under: false,
            });
        }
        placed
    }

    /* The page images a frame shows: the pages in view, and a few before and after
     * them kept ready so that scrolling doesn't show blank pages */
    pub fn plan_pages(&mut self, renderer: &Renderer) -> Vec<PagePlacement> {
        RENDER_SCALE.store(self.render_scale().to_bits(), Ordering::Release);
        let preloaded = PAGES_PRELOADED;
        let mut plan = Vec::new();

        /* The index of the first rendered page */
        let mut page_index = self.page_first();
        if self.cumulative_heights.len() <= page_index {
            return plan;
        }

        for i in 0..usize::min(preloaded, page_index) {
            plan.extend(self.plan_page(page_index - 1 - i, None, renderer));
        }

        for (page, rect) in self.calculate_display_bounds() {
            plan.extend(self.plan_page(page, Some(rect), renderer));
            page_index += 1;
        }

        for _ in 0..usize::min(preloaded, self.cumulative_heights.len() - page_index) {
            plan.extend(self.plan_page(page_index, None, renderer));
            page_index += 1;
        }

        self.wanted = plan.iter().map(|p| p.page).collect();
        plan
    }

    /* Renders a page again after kitty no longer had its image. Gives up after a few
     * failures in a row so a terminal that keeps refusing can't cause an endless
     * redraw loop; `false` means it gave up. */
    pub fn render_again(&mut self, page: usize) -> bool {
        const MAX_RETRIES: usize = 3;

        let failures = self.failures.entry(page).or_insert(0);
        *failures += 1;
        if *failures > MAX_RETRIES {
            return false;
        }
        let failures = *failures;
        self.remove_image(page);
        self.scheduled4render.remove(&page);
        self.failures.insert(page, failures);
        crate::threads::wake(crate::threads::Wake::Redraw);
        true
    }

    pub fn placement_ok(&mut self, page: usize) {
        self.failures.remove(&page);
    }
}

/* A page with at most this many pixels at the scale in use is rendered whole */
const WHOLE_PIXELS: f32 = 12_000_000.0;
/* Past that, the part in view and a margin around it, up to this many */
const REGION_PIXELS: f32 = 16_000_000.0;
/* and pages out of view whole but coarser, standing in until they come into view */
const STAND_IN_PIXELS: f32 = 4_000_000.0;
/* The margin, as a share of the viewport on each side */
const MARGIN: f32 = 0.5;

/* What to render of a page */
#[derive(Debug, PartialEq)]
pub struct Want {
    pub region: Region,
    pub max_pixels: f32,
    /* The part in view, which an image must cover sharply; `None` out of view */
    pub need: Option<Region>,
}

/* What to render of a page `size` page units big that is drawn at `rect` (`None` when
 * it's out of view) with `view_scale` pixels per unit in a viewport `vp` pixels big,
 * rendering at `scale`. A page that fits a whole image is rendered whole; a bigger one
 * is rendered sharp only around the part in view, and coarse while out of view, which
 * keeps every image within what kitty holds at any zoom. */
pub fn wanted(
    size: (f32, f32),
    rect: Option<DisplayRect>,
    view_scale: f32,
    vp: (f32, f32),
    scale: f32,
) -> Want {
    let (w, h) = size;
    let whole = [0.0, 0.0, w, h];
    /* The part in view, if any of the page is: its edge can sit just outside */
    let need = rect.and_then(|r| {
        let x0 = ((-r.x as f32) / view_scale).clamp(0.0, w);
        let y0 = ((-r.y as f32) / view_scale).clamp(0.0, h);
        let x1 = ((vp.0 - r.x as f32) / view_scale).clamp(x0, w);
        let y1 = ((vp.1 - r.y as f32) / view_scale).clamp(y0, h);
        (x1 - x0 >= 0.5 && y1 - y0 >= 0.5).then_some([x0, y0, x1, y1])
    });
    let pixels = |r: Region| (r[2] - r[0]) * (r[3] - r[1]) * scale * scale;
    if pixels(whole) <= WHOLE_PIXELS {
        return Want {
            region: whole,
            max_pixels: WHOLE_PIXELS,
            need,
        };
    }
    let Some(need) = need else {
        return Want {
            region: whole,
            max_pixels: STAND_IN_PIXELS,
            need: None,
        };
    };
    let (mx, my) = (vp.0 * MARGIN / view_scale, vp.1 * MARGIN / view_scale);
    let around = [
        (need[0] - mx).max(0.0),
        (need[1] - my).max(0.0),
        (need[2] + mx).min(w),
        (need[3] + my).min(h),
    ];
    let region = if pixels(around) <= REGION_PIXELS {
        around
    } else {
        need
    };
    Want {
        region,
        max_pixels: REGION_PIXELS,
        need: Some(need),
    }
}

/* Two scales are the same when they'd give the same pixels */
fn same_scale(a: f32, b: f32) -> bool {
    (a - b).abs() <= b.abs() * 1e-3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i32, y: i32, width: i32, height: i32) -> Option<DisplayRect> {
        Some(DisplayRect {
            x,
            y,
            width,
            height,
        })
    }

    #[test]
    fn a_page_that_fits_is_rendered_whole() {
        /* A letter page at 2 pixels per unit: 1224 x 1584, under the limit */
        let want = wanted(
            (612.0, 792.0),
            at(0, 0, 1224, 1584),
            2.0,
            (1224.0, 800.0),
            2.0,
        );
        assert_eq!(want.region, [0.0, 0.0, 612.0, 792.0]);
        assert_eq!(want.max_pixels, WHOLE_PIXELS);
        assert_eq!(want.need, Some([0.0, 0.0, 612.0, 400.0]));
    }

    #[test]
    fn deep_in_a_page_only_the_part_in_view_is_sharp() {
        /* At 8 pixels per unit the page would be 31 Mpx; the view shows its middle */
        let rect = at(-2000, -3000, 4896, 6336);
        let want = wanted((612.0, 792.0), rect, 8.0, (1600.0, 1000.0), 8.0);
        let need = want.need.unwrap();
        assert_eq!(need, [250.0, 375.0, 450.0, 500.0]);
        assert_eq!(want.max_pixels, REGION_PIXELS);
        /* Half a viewport of margin on each side */
        assert_eq!(want.region, [150.0, 312.5, 550.0, 562.5]);
        let pixels =
            (want.region[2] - want.region[0]) * (want.region[3] - want.region[1]) * 64.0;
        assert!(pixels <= REGION_PIXELS);
    }

    #[test]
    fn a_big_page_out_of_view_stands_in_coarse() {
        let want = wanted((612.0, 792.0), None, 8.0, (1600.0, 1000.0), 8.0);
        assert_eq!(want.region, [0.0, 0.0, 612.0, 792.0]);
        assert_eq!(want.max_pixels, STAND_IN_PIXELS);
        assert_eq!(want.need, None);
    }

    #[test]
    fn a_page_just_above_the_view_has_nothing_in_view() {
        /* Its bottom edge ends where the view starts: only the gap below it shows */
        let rect = at(-1469, -5136, 3919, 5072);
        let scale = 5072.0 / 792.0;
        let want = wanted((612.0, 792.0), rect, scale, (980.0, 760.0), scale);
        assert_eq!(want.need, None);
        assert_eq!(want.max_pixels, STAND_IN_PIXELS);
    }

    #[test]
    fn regions_cover_what_they_should() {
        assert!(covers([0.0, 0.0, 100.0, 100.0], [10.0, 10.0, 90.0, 100.2]));
        assert!(!covers([0.0, 0.0, 100.0, 100.0], [10.0, 10.0, 90.0, 120.0]));
    }

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
