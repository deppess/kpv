use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc, Mutex, RwLock,
    },
    thread,
    time::{Duration, Instant},
};

use mupdf::{
    Colorspace, Device, Document, Link, Matrix, MetadataName, Page, Pixmap, Quad,
};

use crate::image::clear_border;
use crate::{
    config::Config,
    globals::{CONFIG, RENDER_SCALE, RUNNING},
    image::{recolor_pixels, Image, Region},
    outline::{self, OutlineEntry},
    store,
    theme::{paint, Slot},
    threads::{wake, Wake},
};

/* A page to render: the part `region` of it at `scale` pixels per page unit, with at
 * most `max_pixels`, coarser if it would take more */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Request {
    pub page: usize,
    pub scale: f32,
    pub region: Region,
    pub max_pixels: f32,
}

#[derive(Clone, PartialEq)]
pub enum RendererAction {
    Load,
    Display(Request),
    ToggleInverse,
    Search(String),
    /* Index into the last search results, drawn in the stronger color */
    SetCurrentMatch(Option<usize>),
    ClearSearch,
}

/* One search hit: its page, and its top-left corner in page units from the page's
 * top-left corner */
#[derive(Clone, Debug, PartialEq)]
pub struct SearchMatch {
    pub page: usize,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone)]
pub enum RendererResult {
    PageMetadata {
        max_page_width: f32,
        cumulative_heights: Vec<f32>,
        widths: Vec<f32>,
        links: Vec<Vec<Link>>,
        outline: Vec<OutlineEntry>,
        /* The permanent half of the PDF's /ID */
        id: Option<String>,
        /* The title and the author stored in the PDF */
        title: Option<String>,
        author: Option<String>,
    },
    Image {
        page: usize,
        data: Option<Arc<RwLock<Image>>>,
    },
    /* Matches found since the last batch, by the search `generation`, with how many
     * of the `total` pages it has searched; it's done when they're all searched */
    SearchProgress {
        generation: u64,
        needle: String,
        matches: Vec<SearchMatch>,
        searched: usize,
        total: usize,
    },
}

struct RendererInnerState<'a> {
    pub config: &'a Config,

    pub file: String,
    pub document: Document,

    pub cache: Vec<Page>,
    /* Search hits as (page, rectangle in page coordinates) */
    /* Shared with the search thread, which adds to them as it finds them */
    pub hits: Arc<Mutex<Vec<(usize, mupdf::Rect)>>>,
    /* Counts searches; the running one stops when it changes */
    search_generation: Arc<AtomicU64>,
    pub current_hit: Option<usize>,

    pub inverse: bool,

    pub cs: Colorspace,
}

impl<'a> RendererInnerState<'a> {
    pub fn new(file: String) -> Result<Self, String> {
        let document = Document::open(&file)
            .map_err(|x| format!("Could not open the given PDF file: {}", x))?;

        let config = CONFIG.get().unwrap();
        let inner_state = Self {
            config,
            file,
            document,
            cache: Vec::new(),
            hits: Arc::new(Mutex::new(Vec::new())),
            search_generation: Arc::new(AtomicU64::new(0)),
            current_hit: None,
            inverse: false,
            cs: Colorspace::device_rgb(),
        };

        Ok(inner_state)
    }

    pub fn load(&mut self) -> Result<RendererResult, String> {
        let mut max_page_width = -f32::INFINITY;
        let mut cumulative_heights = Vec::new();
        let mut widths = Vec::new();
        let mut links = Vec::new();

        /* Build everything first and only then replace the old document, so a file that
         * can't be read (e.g. still being written) leaves the current one in place */
        let document = Document::open(&self.file)
            .map_err(|x| format!("Could not open the given PDF file: {}", x))?;
        if !document.is_pdf() {
            Err("The given PDF file is not a PDF!".to_string())?;
        }
        let mut cache = Vec::new();

        let page_count = document
            .page_count()
            .map_err(|x| format!("Could not extract the number of pages: {}", x))?;

        for i in 0..page_count {
            let page = document
                .load_page(i)
                .map_err(|x| format!("Could not load page {}: {}", i, x))?;

            let bounds = page
                .bounds()
                .map_err(|x| format!("Could not get bounds for page {}: {}", i, x))?;

            let width = bounds.width();
            let height = bounds.height();

            max_page_width = f32::max(max_page_width, width);
            cumulative_heights.push(
                cumulative_heights.last().unwrap_or(&0.0f32)
                    + height
                    + self.config.viewer.page_gap,
            );
            widths.push(width);
            links.push(
                page.links()
                    .map_err(|x| format!("Could not extract links: {}", x))?
                    .collect(),
            );
            cache.push(page);
        }

        /* A document without an outline, or with a broken one, has no contents */
        let outline = document
            .outlines()
            .map(|o| outline::flatten(&o))
            .unwrap_or_default();
        let id = store::document_id(&document);
        let stored = |name| {
            document
                .metadata(name)
                .ok()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
        };
        let (title, author) = (stored(MetadataName::Title), stored(MetadataName::Author));

        self.document = document;
        self.cache = cache;
        /* A search of the old version stops; the viewer searches the new one */
        self.search_generation.fetch_add(1, Ordering::AcqRel);
        self.hits.lock().unwrap().clear();
        self.current_hit = None;

        Ok(RendererResult::PageMetadata {
            max_page_width,
            cumulative_heights,
            widths,
            links,
            outline,
            id,
            title,
            author,
        })
    }
}

impl RendererInnerState<'_> {
    /* Draws the part of a page a request asks for into an RGBA bitmap with `padding()`
     * clear pixels around it, which kitty takes as it is. Returns the bitmap, the
     * scale it was drawn at, and the part of the page it covers, snapped to whole
     * pixels. */
    fn render(&self, request: &Request) -> Option<(Pixmap, f32, Region)> {
        let page = self.cache.get(request.page)?;
        let bounds = page.bounds().ok()?;
        let [x0, y0, x1, y1] = request.region;
        let area = ((x1 - x0) * (y1 - y0)).max(1.0);
        let scale = request
            .scale
            .min((request.max_pixels / area).sqrt())
            .max(1e-3);

        /* The part in device pixels, where the page's own origin can be anywhere */
        let px = |v: f32, origin: f32| (origin + v) * scale;
        let left = px(x0, bounds.x0).floor() as i32;
        let top = px(y0, bounds.y0).floor() as i32;
        let right = (px(x1, bounds.x0).ceil() as i32).max(left + 1);
        let bottom = (px(y1, bounds.y0).ceil() as i32).max(top + 1);
        let pad = padding() as i32;
        let mut pixmap = Pixmap::new(
            &self.cs,
            left - pad,
            top - pad,
            right - left + 2 * pad,
            bottom - top + 2 * pad,
            true,
        )
        .ok()?;
        pixmap.clear_with(255).ok()?;
        let (width, height) = (pixmap.width() as usize, pixmap.height() as usize);
        clear_border(pixmap.samples_mut(), width, height, pad as usize);
        {
            let device = Device::from_pixmap(&pixmap).ok()?;
            page.run(&device, &Matrix::new_scale(scale, scale)).ok()?;
        }
        let unit = |v: i32, origin: f32| v as f32 / scale - origin;
        let region = [
            unit(left, bounds.x0),
            unit(top, bounds.y0),
            unit(right, bounds.x0),
            unit(bottom, bounds.y0),
        ];
        Some((pixmap, scale, region))
    }

    /* Searches the document on a thread of its own, with its own copy of it, so that
     * pages go on rendering. Matches go to the viewer a batch at a time and into the
     * hits painted on pages as they're found; a newer search or a reload stops it. */
    fn start_search(&mut self, needle: String) {
        const MAX_HITS_PER_PAGE: u32 = 500;
        const BATCH: Duration = Duration::from_millis(100);

        let generation = self.search_generation.fetch_add(1, Ordering::AcqRel) + 1;
        self.current_hit = None;
        self.hits.lock().unwrap().clear();
        let (file, hits, live) = (
            self.file.clone(),
            self.hits.clone(),
            self.search_generation.clone(),
        );
        thread::spawn(move || {
            let is_live = || live.load(Ordering::Acquire) == generation;
            let send = |matches: Vec<SearchMatch>, searched: usize, total: usize| {
                let progress = RendererResult::SearchProgress {
                    generation,
                    needle: needle.clone(),
                    matches,
                    searched,
                    total,
                };
                wake(Wake::Rendered(progress));
            };
            let Ok(document) = Document::open(&file) else {
                send(Vec::new(), 0, 0);
                return;
            };
            let total = document.page_count().unwrap_or(0).max(0) as usize;
            let mut batch = Vec::new();
            let mut sent = Instant::now();
            for index in 0..total {
                let found = document.load_page(index as i32).ok().and_then(|page| {
                    let bounds = page.bounds().ok()?;
                    let quads = page.search(&needle, MAX_HITS_PER_PAGE).ok()?;
                    Some((bounds, quads.iter().map(quad_bounds).collect::<Vec<_>>()))
                });
                if let Some((bounds, rects)) = found {
                    let mut shared = hits.lock().unwrap();
                    if !is_live() {
                        return;
                    }
                    for rect in rects {
                        batch.push(SearchMatch {
                            page: index,
                            x: rect.x0 - bounds.x0,
                            y: rect.y0 - bounds.y0,
                        });
                        shared.push((index, rect));
                    }
                }
                if !is_live() {
                    return;
                }
                if index + 1 == total || sent.elapsed() >= BATCH {
                    send(std::mem::take(&mut batch), index + 1, total);
                    sent = Instant::now();
                }
            }
            if total == 0 {
                send(Vec::new(), 0, 0);
            }
        });
    }

    /* Paints the search hits of `page` onto its bitmap */
    fn highlight(&self, page: usize, pixmap: &mut mupdf::Pixmap, scale: f32) {
        /* Without its own opacity a color is mixed in at this much, out of 255 */
        const STRENGTH: u8 = 110;
        let (hit, current) = (
            page_color(Slot::PagesMatch, [255, 214, 0], STRENGTH),
            page_color(Slot::PagesCurrentMatch, [255, 120, 0], STRENGTH),
        );

        let (origin_x, origin_y) = (pixmap.x(), pixmap.y());
        let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
        let n = pixmap.n() as usize;
        let stride = width as usize * n;

        let hits = self.hits.lock().unwrap();
        for (i, (_, rect)) in hits.iter().enumerate().filter(|(_, h)| h.0 == page) {
            let ([r, g, b], alpha) = if self.current_hit == Some(i) {
                current
            } else {
                hit
            };
            let (color, alpha) = ([r, g, b], alpha as u32);
            let x0 = ((rect.x0 * scale) as i32 - origin_x).clamp(0, width);
            let x1 = ((rect.x1 * scale).ceil() as i32 - origin_x).clamp(0, width);
            let y0 = ((rect.y0 * scale) as i32 - origin_y).clamp(0, height);
            let y1 = ((rect.y1 * scale).ceil() as i32 - origin_y).clamp(0, height);
            let samples = pixmap.samples_mut();
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = y as usize * stride + x as usize * n;
                    for c in 0..3 {
                        let base = samples[at + c] as u32;
                        samples[at + c] = ((base * (255 - alpha)
                            + color[c] as u32 * alpha)
                            / 255) as u8;
                    }
                }
            }
        }
    }
}

/* A color painted onto pages and how strongly, from the theme; `fallback` when the
 * theme says the terminal's own color, which has no value to paint with */
pub fn page_color(slot: Slot, fallback: [u8; 3], strength: u8) -> ([u8; 3], u8) {
    let p = paint(slot);
    (p.rgb().unwrap_or(fallback), p.alpha().unwrap_or(strength))
}

/* Clear pixels around a page image: two cells, so that while a page waits for its
 * image at a new zoom, the old one can still be cut at whole cells */
fn padding() -> usize {
    let vp = crate::ui::viewport();
    (2.0 * vp.pxpercol.max(vp.pxperrow)).ceil() as usize
}

fn quad_bounds(quad: &Quad) -> mupdf::Rect {
    let xs = [quad.ul.x, quad.ur.x, quad.ll.x, quad.lr.x];
    let ys = [quad.ul.y, quad.ur.y, quad.ll.y, quad.lr.y];
    mupdf::Rect {
        x0: xs.iter().cloned().fold(f32::INFINITY, f32::min),
        y0: ys.iter().cloned().fold(f32::INFINITY, f32::min),
        x1: xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
        y1: ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
    }
}

/* The viewer's side of the renderer thread, which draws pages and sends them, with
 * everything else it finds, to the main loop */
pub struct Renderer {
    /* What the viewer asks for, in order */
    actions: Sender<RendererAction>,
}

impl Renderer {
    pub fn start(file_input: &str) -> Self {
        let file_string = file_input.to_owned();
        let (actions, inbox) = channel::<RendererAction>();
        thread::spawn(move || {
            let error = match render_loop(file_string, inbox) {
                Ok(()) => "The renderer stopped".to_owned(),
                Err(e) => e,
            };
            wake(Wake::Fatal(error));
        });
        Self { actions }
    }

    /* Queues an action for the renderer, which takes them in order */
    pub fn send_action(&self, action: RendererAction) -> Result<(), String> {
        self.actions
            .send(action)
            .map_err(|_| "The renderer stopped".to_owned())
    }
}

fn render_loop(
    file_string: String,
    inbox: Receiver<RendererAction>,
) -> Result<(), String> {
    let mut state = RendererInnerState::new(file_string)?;

    while RUNNING.load(Ordering::Acquire) {
        let action = inbox
            .recv()
            .map_err(|x| format!("Could not receive from client: {}", x))?;

        match action {
            RendererAction::Load => {
                /* What was asked of the old version goes */
                while inbox.try_recv().is_ok() {}
                /* A failed reload keeps showing the last good version */
                let Ok(result) = state.load() else { continue };
                wake(Wake::Rendered(result));
            }
            RendererAction::ToggleInverse => {
                state.inverse = !state.inverse;
            }
            RendererAction::Search(needle) => {
                state.start_search(needle);
            }
            RendererAction::SetCurrentMatch(current) => {
                state.current_hit = current;
            }
            RendererAction::ClearSearch => {
                state.search_generation.fetch_add(1, Ordering::AcqRel);
                state.hits.lock().unwrap().clear();
                state.current_hit = None;
            }
            RendererAction::Display(request) => {
                let page = request.page;
                /* A newer zoom asked for this page at another scale */
                let wanted = f32::from_bits(RENDER_SCALE.load(Ordering::Acquire));
                if (request.scale - wanted).abs() > wanted * 1e-3 {
                    continue;
                }
                if state.cache.get(page).is_none() {
                    /* A page the new version doesn't have is forgotten */
                    wake(Wake::Rendered(RendererResult::Image { page, data: None }));
                    continue;
                }

                /* A page that can't be drawn stays blank */
                let Some((mut pixmap, render, region)) = state.render(&request) else {
                    continue;
                };
                if state.inverse {
                    let n = pixmap.n() as usize;
                    let (background, _) =
                        page_color(Slot::PagesDarkBackground, [30, 30, 36], 255);
                    let (text, _) = page_color(Slot::PagesDarkText, [220, 220, 224], 255);
                    recolor_pixels(pixmap.samples_mut(), n, background, text);
                }
                state.highlight(page, &mut pixmap, render);

                let Ok(image) = Image::new(
                    &pixmap,
                    padding(),
                    request.scale,
                    region,
                    request.max_pixels,
                ) else {
                    continue;
                };
                wake(Wake::Rendered(RendererResult::Image {
                    page,
                    data: Some(Arc::new(RwLock::new(image))),
                }));
            }
        };
    }

    Ok(())
}
