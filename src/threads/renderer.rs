use core::f32;
use std::{
    sync::{atomic::Ordering, Arc, RwLock},
    thread::{self, JoinHandle},
};

use crossbeam_channel::{unbounded, Receiver, Sender};
use mupdf::{Colorspace, Document, Link, Matrix, Page, Quad};

use crate::{
    config::Config,
    drivers::priority_channel::{unbounded_priority, PriorityReceiver, PrioritySender},
    globals::{CONFIG, RUNNING},
    image::{invert_pixels, Image},
    labels,
    outline::{self, OutlineEntry},
    store,
    theme::{paint, Slot},
};

#[derive(Clone, PartialEq)]
pub enum RendererAction {
    Load,
    Display(usize),
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
    },
    Image {
        page: usize,
        data: Option<Arc<RwLock<Image>>>,
    },
    SearchResults {
        needle: String,
        matches: Vec<SearchMatch>,
    },
}

struct RendererInnerState<'a> {
    pub config: &'a Config,

    pub file: String,
    pub document: Document,

    pub cache: Vec<Page>,
    /* Search hits as (page, rectangle in page coordinates) */
    pub hits: Vec<(usize, mupdf::Rect)>,
    pub current_hit: Option<usize>,

    pub alpha: bool,
    pub inverse: bool,

    pub cs: Colorspace,
    pub ctm: Matrix,
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
            hits: Vec::new(),
            current_hit: None,
            alpha: false,
            inverse: false,
            cs: Colorspace::device_rgb(),
            ctm: Matrix::new_scale(
                config.viewer.render_precision as f32,
                config.viewer.render_precision as f32,
            ),
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
                    + self.config.viewer.margin_bottom,
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

        self.document = document;
        self.cache = cache;
        self.hits.clear();
        self.current_hit = None;

        Ok(RendererResult::PageMetadata {
            max_page_width,
            cumulative_heights,
            widths,
            links,
            labels,
            outline,
            id,
        })
    }
}

impl RendererInnerState<'_> {
    pub fn search(&mut self, needle: &str) -> Vec<SearchMatch> {
        const MAX_HITS_PER_PAGE: u32 = 500;

        self.hits.clear();
        self.current_hit = None;
        let mut matches = Vec::new();
        for (index, page) in self.cache.iter().enumerate() {
            let Ok(bounds) = page.bounds() else { continue };
            let Ok(quads) = page.search(needle, MAX_HITS_PER_PAGE) else {
                continue;
            };
            for quad in quads.iter() {
                let rect = quad_bounds(quad);
                matches.push(SearchMatch {
                    page: index,
                    x: rect.x0 - bounds.x0,
                    y: rect.y0 - bounds.y0,
                });
                self.hits.push((index, rect));
            }
        }
        matches
    }

    /* Paints the search hits of `page` onto its bitmap */
    fn highlight(&self, page: usize, pixmap: &mut mupdf::Pixmap) {
        /* Without its own opacity a color is mixed in at this much, out of 255 */
        const STRENGTH: u8 = 110;
        let (hit, current) = (
            page_color(Slot::PagesMatch, [255, 214, 0], STRENGTH),
            page_color(Slot::PagesCurrentMatch, [255, 120, 0], STRENGTH),
        );

        let scale = self.config.viewer.render_precision as f32;
        let (origin_x, origin_y) = (pixmap.x(), pixmap.y());
        let (width, height) = (pixmap.width() as i32, pixmap.height() as i32);
        let n = pixmap.n() as usize;
        let stride = width as usize * n;

        for (i, (_, rect)) in self.hits.iter().enumerate().filter(|(_, h)| h.0 == page) {
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

        let thread_render =
            thread::spawn(move || {
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
                        RendererAction::Display(_) => (),
                        _ => {
                            general_server_sender.try_send(action.clone()).map_err(
                                |x| format!("Could not send action to client: {}", x),
                            )?;
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
                            let matches = state.search(&needle);
                            priority_server_receiver.clear_priority(1);
                            result_server_sender
                                .try_send_priority(
                                    RendererResult::SearchResults { needle, matches },
                                    0,
                                )
                                .map_err(|x| {
                                    format!("Could not send results to client: {}", x)
                                })?;
                        }
                        RendererAction::SetCurrentMatch(current) => {
                            state.current_hit = current;
                            priority_server_receiver.clear_priority(1);
                        }
                        RendererAction::ClearSearch => {
                            state.hits.clear();
                            state.current_hit = None;
                            priority_server_receiver.clear_priority(1);
                        }
                        RendererAction::Display(page) => {
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
                            let Ok(mut pixmap) = state.cache[page].to_pixmap(
                                &state.ctm,
                                &state.cs,
                                state.alpha,
                                false,
                            ) else {
                                continue;
                            };
                            if state.inverse {
                                let n = pixmap.n() as usize;
                                invert_pixels(pixmap.samples_mut(), n);
                            }
                            state.highlight(page, &mut pixmap);

                            let Ok(image) = Image::new(&pixmap) else {
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
            RendererAction::Display(_) => self
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
            RendererAction::Display(_) => Err("Cannot wait for display".to_string())?,
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
