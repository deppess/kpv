use core::f32;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, RwLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossbeam_channel::{unbounded, Receiver, Sender};
use mupdf::{
    Colorspace, Device, Document, Link, Matrix, MetadataName, Page, Pixmap, Quad,
};

use crate::{
    config::Config,
    drivers::priority_channel::{unbounded_priority, PriorityReceiver, PrioritySender},
    globals::{CONFIG, RENDER_SCALE, RUNNING},
    image::{recolor_pixels, Image, Region},
    labels,
    outline::{self, OutlineEntry},
    store,
    theme::{paint, Slot},
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
    ToggleAlpha,
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
        /* The printed label of every page */
        labels: Vec<String>,
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

    pub alpha: bool,
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
            alpha: false,
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

        let labels = labels::labels(&labels::read(&document), page_count as usize);
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
            labels,
            outline,
            id,
            title,
            author,
        })
    }
}

impl RendererInnerState<'_> {
    /* Draws the part of a page a request asks for. Returns the bitmap, the scale it was
     * drawn at, and the part of the page it covers, snapped to whole pixels. */
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
        let mut pixmap =
            Pixmap::new(&self.cs, left, top, right - left, bottom - top, self.alpha)
                .ok()?;
        if self.alpha {
            pixmap.clear().ok()?;
        } else {
            pixmap.clear_with(255).ok()?;
        }
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
    fn start_search(
        &mut self,
        needle: String,
        results: PrioritySender<RendererResult, 2>,
    ) {
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
                let _ = results.try_send_priority(progress, 0);
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
                    if n == 4 {
                        samples[at + 3] = samples[at + 3].max(alpha as u8);
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
    let vp = crate::ui::inner();
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

pub struct Renderer {
    thread_render: Option<JoinHandle<Result<(), String>>>,

    priority_client_sender: PrioritySender<RendererAction, 2>,
    priority_server_receiver: PriorityReceiver<RendererAction, 2>,

    general_client_receiver: Receiver<RendererAction>,
    general_server_sender: Sender<RendererAction>,

    result_server_sender: PrioritySender<RendererResult, 2>,
}

impl Renderer {
    pub fn new() -> (Self, PriorityReceiver<RendererResult, 2>) {
        let (priority_client_sender, priority_server_receiver) =
            unbounded_priority::<RendererAction, 2>();

        let (general_server_sender, general_client_receiver) =
            unbounded::<RendererAction>();

        let (result_server_sender, result_client_receiver) =
            unbounded_priority::<RendererResult, 2>();

        (
            Self {
                thread_render: None,

                priority_client_sender,
                priority_server_receiver,

                general_client_receiver,
                general_server_sender,

                result_server_sender,
            },
            result_client_receiver,
        )
    }

    pub fn run(&mut self, file_input: &str) -> Result<(), String> {
        let file_string = file_input.to_owned();
        let priority_server_receiver = self.priority_server_receiver.clone();
        let general_server_sender = self.general_server_sender.clone();
        let result_server_sender = self.result_server_sender.clone();

        let thread_render = thread::spawn(move || {
            let priority_server_receiver = priority_server_receiver;
            let general_server_sender = general_server_sender;
            let result_server_sender = result_server_sender;

            let mut state = RendererInnerState::new(file_string)?;

            let mut sel = priority_server_receiver.construct_biased_select();

            while RUNNING.load(Ordering::Acquire) {
                let action = priority_server_receiver
                    .recv_priority(sel.ready())
                    .map_err(|x| format!("Could not receive from client: {}", x))?;

                match action {
                    RendererAction::Display(..) => (),
                    _ => {
                        general_server_sender
                            .try_send(action.clone())
                            .map_err(|x| {
                                format!("Could not send action to client: {}", x)
                            })?;
                    }
                }

                match action {
                    RendererAction::Load => {
                        priority_server_receiver.clear_priority(0);
                        /* A failed reload keeps showing the last good version */
                        let Ok(result) = state.load() else { continue };

                        // Clear the scheduled pages for rendering
                        priority_server_receiver.clear_priority(1);

                        result_server_sender.try_send_priority(result, 0).map_err(
                            |x| format!("Could not send results to client: {}", x),
                        )?;
                    }
                    RendererAction::ToggleAlpha => {
                        state.alpha = !state.alpha;

                        // Clear the scheduled pages for rendering
                        priority_server_receiver.clear_priority(1);
                    }
                    RendererAction::ToggleInverse => {
                        state.inverse = !state.inverse;

                        // Clear the scheduled pages for rendering
                        priority_server_receiver.clear_priority(1);
                    }
                    RendererAction::Search(needle) => {
                        state.start_search(needle, result_server_sender.clone());
                        priority_server_receiver.clear_priority(1);
                    }
                    RendererAction::SetCurrentMatch(current) => {
                        state.current_hit = current;
                        priority_server_receiver.clear_priority(1);
                    }
                    RendererAction::ClearSearch => {
                        state.search_generation.fetch_add(1, Ordering::AcqRel);
                        state.hits.lock().unwrap().clear();
                        state.current_hit = None;
                        priority_server_receiver.clear_priority(1);
                    }
                    RendererAction::Display(request) => {
                        let page = request.page;
                        /* A newer zoom asked for this page at another scale */
                        let wanted = f32::from_bits(RENDER_SCALE.load(Ordering::Acquire));
                        if (request.scale - wanted).abs() > wanted * 1e-3 {
                            continue;
                        }
                        if state.cache.get(page).is_none() {
                            // Sending `None` as data signals that it should be
                            // removed from the registry
                            result_server_sender
                                .try_send_priority(
                                    RendererResult::Image { page, data: None },
                                    1,
                                )
                                .map_err(|x| {
                                    format!("Could not send result to client: {}", x)
                                })?;
                            continue;
                        }

                        /* A page that can't be drawn stays blank */
                        let Some((mut pixmap, render, region)) = state.render(&request)
                        else {
                            continue;
                        };
                        if state.inverse {
                            let n = pixmap.n() as usize;
                            let (background, _) =
                                page_color(Slot::PagesDarkBackground, [30, 30, 36], 255);
                            let (text, _) =
                                page_color(Slot::PagesDarkText, [220, 220, 224], 255);
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
                        result_server_sender
                            .try_send_priority(
                                RendererResult::Image {
                                    page,
                                    data: Some(Arc::new(RwLock::new(image))),
                                },
                                1,
                            )
                            .map_err(|x| {
                                format!("Could not send results to client: {}", x)
                            })?;
                    }
                };
            }

            Ok(())
        });

        /* ======================== Check and move the threads ======================= */
        if thread_render.is_finished() {
            thread_render
                .join()
                .map_err(|_| "The renderer stopped".to_owned())??;
        } else {
            self.thread_render = Some(thread_render);
        }

        Ok(())
    }

    pub fn send_action(&self, action: RendererAction) -> Result<(), String> {
        match action {
            RendererAction::Display(..) => self
                .priority_client_sender
                .try_send_priority(action, 0)
                .map_err(|x| format!("Could not send action to renderer: {}", x))?,
            _ => Err("Only Display can be sent without confirmation".to_string())?,
        }

        Ok(())
    }

    pub fn send_and_confirm_action(&self, action: RendererAction) -> Result<(), String> {
        /* Sends an action to the renderer thread and waits until the thread confirms
         * that the action has been accepted and scheduled */
        match action {
            RendererAction::Display(..) => Err("Cannot wait for display".to_string())?,
            _ => self
                .priority_client_sender
                .try_send_priority(action.clone(), 0)
                .map_err(|x| format!("Could not send action to renderer: {}", x))?,
        }

        let result = self
            .general_client_receiver
            .recv()
            .map_err(|x| format!("Could not receive action from renderer: {}", x))?;

        if result != action {
            return Err("Sent and received action from renderer don't match!".to_owned());
        }

        Ok(())
    }
}
