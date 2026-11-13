use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mupdf::Outline;

use crate::{
    state::{edit_prompt, PromptKey},
    viewer::dest_top,
};

/* One line of the table of contents, flattened in reading order */
#[derive(Clone, Debug, PartialEq)]
pub struct OutlineEntry {
    pub title: String,
    pub depth: usize,
    /* Where it points: a page and, when known, the spot on it in page units from the
     * page's top. Entries that point outside the document have no page. */
    pub page: Option<usize>,
    pub top: Option<f32>,
}

/* Flattens MuPDF's outline tree, depth first */
pub fn flatten(outlines: &[Outline]) -> Vec<OutlineEntry> {
    fn walk(items: &[Outline], depth: usize, out: &mut Vec<OutlineEntry>) {
        /* A broken file could nest without end */
        const MAX_DEPTH: usize = 32;
        for item in items {
            out.push(OutlineEntry {
                title: clean_title(&item.title),
                depth,
                page: item.dest.map(|d| d.loc.page_number as usize),
                top: item.dest.and_then(|d| dest_top(d.kind)),
            });
            if depth < MAX_DEPTH {
                walk(&item.down, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(outlines, 0, &mut out);
    out
}

/* Titles can carry line breaks and runs of spaces */
fn clean_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        "(untitled)".to_owned()
    } else {
        title
    }
}

/* The entry whose section holds the spot `y` on `page`: the last one that starts at
 * or before it */
pub fn current_section(entries: &[OutlineEntry], page: usize, y: f32) -> Option<usize> {
    let mut best: Option<(usize, (usize, f32))> = None;
    for (i, entry) in entries.iter().enumerate() {
        let Some(p) = entry.page else { continue };
        let at = (p, entry.top.unwrap_or(0.0));
        if at.0 > page || (at.0 == page && at.1 > y) {
            continue;
        }
        if best.is_none_or(|(_, b)| at.0 > b.0 || (at.0 == b.0 && at.1 >= b.1)) {
            best = Some((i, at));
        }
    }
    best.map(|(i, _)| i)
}

fn parent(entries: &[OutlineEntry], i: usize) -> Option<usize> {
    let depth = entries[i].depth;
    (0..i).rev().find(|&j| entries[j].depth < depth)
}

/* `i` and the entries above it, outermost first */
pub fn breadcrumb(entries: &[OutlineEntry], i: usize) -> Vec<&str> {
    let mut chain = vec![i];
    while let Some(p) = parent(entries, *chain.last().unwrap()) {
        chain.push(p);
    }
    chain
        .iter()
        .rev()
        .map(|&j| entries[j].title.as_str())
        .collect()
}

/* What a key in the panel asks the main loop to do */
#[derive(Debug, PartialEq)]
pub enum TocKey {
    Handled,
    Close,
    /* Go to the selected entry, and close the panel when `close` */
    Jump { close: bool },
    /* Not a panel key; the bindings decide */
    Other,
}

/* The panel: which entries are open, which is selected, what's typed in its filter */
#[derive(Debug, Default)]
pub struct Toc {
    pub entries: Vec<OutlineEntry>,
    open: Vec<bool>,
    pub selected: usize,
    /* First visible row */
    pub scroll: usize,
    pub filter: String,
    /* The filter prompt has the keys */
    pub editing: bool,
    /* `z` was typed: the next key folds */
    pending_z: bool,
}

impl Toc {
    pub fn new(entries: Vec<OutlineEntry>) -> Self {
        let open = vec![false; entries.len()];
        Self {
            entries,
            open,
            ..Self::default()
        }
    }

    pub fn has_children(&self, i: usize) -> bool {
        self.entries
            .get(i + 1)
            .is_some_and(|next| next.depth > self.entries[i].depth)
    }

    /* Whether `i` shows its children; every entry does while filtering */
    pub fn is_open(&self, i: usize) -> bool {
        !self.filter.is_empty() || self.open[i]
    }

    /* The entries on screen, in order: everything under an open entry, or with a
     * filter, the matching entries and the entries above them */
    pub fn rows(&self) -> Vec<usize> {
        if !self.filter.is_empty() {
            let needle = self.filter.to_lowercase();
            let mut shown = vec![false; self.entries.len()];
            for (i, entry) in self.entries.iter().enumerate() {
                if entry.title.to_lowercase().contains(&needle) {
                    let mut at = Some(i);
                    while let Some(j) = at.filter(|&j| !shown[j]) {
                        shown[j] = true;
                        at = parent(&self.entries, j);
                    }
                }
            }
            return (0..self.entries.len()).filter(|&i| shown[i]).collect();
        }

        let mut rows = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            rows.push(i);
            if self.open[i] {
                i += 1;
            } else {
                /* Skip the closed entry's children */
                let depth = self.entries[i].depth;
                i += 1;
                while i < self.entries.len() && self.entries[i].depth > depth {
                    i += 1;
                }
            }
        }
        rows
    }

    /* Opens the entries above `current` and selects it, for when the panel opens */
    pub fn reveal(&mut self, current: Option<usize>, height: usize) {
        self.filter.clear();
        self.editing = false;
        self.pending_z = false;
        if self.entries.is_empty() {
            return;
        }
        let target = current.unwrap_or(0).min(self.entries.len() - 1);
        let mut at = parent(&self.entries, target);
        while let Some(p) = at {
            self.open[p] = true;
            at = parent(&self.entries, p);
        }
        self.selected = target;
        let rows = self.rows();
        let row = rows.iter().position(|&r| r == target).unwrap_or(0);
        /* Show it in the middle of the panel */
        self.scroll = row
            .saturating_sub(height / 2)
            .min(rows.len().saturating_sub(height));
    }

    fn matches(&self, i: usize) -> bool {
        self.entries[i]
            .title
            .to_lowercase()
            .contains(&self.filter.to_lowercase())
    }

    /* Keeps the selection on a visible row, then scrolls it into view */
    fn settle(&mut self, height: usize) {
        let rows = self.rows();
        if rows.is_empty() {
            self.scroll = 0;
            return;
        }
        if !rows.contains(&self.selected) {
            let mut at = Some(self.selected);
            while let Some(j) = at.filter(|j| !rows.contains(j)) {
                at = parent(&self.entries, j);
            }
            self.selected = at.unwrap_or(rows[0]);
        }
        let row = rows.iter().position(|&r| r == self.selected).unwrap_or(0);
        let height = height.max(1);
        if row < self.scroll {
            self.scroll = row;
        } else if row >= self.scroll + height {
            self.scroll = row + 1 - height;
        }
        self.scroll = self.scroll.min(rows.len().saturating_sub(height));
    }

    fn move_by(&mut self, delta: isize, height: usize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let row = rows.iter().position(|&r| r == self.selected).unwrap_or(0) as isize;
        let to = (row + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.selected = rows[to];
        self.settle(height);
    }

    /* Scrolls the list without moving the selection, as the mouse wheel does */
    pub fn scroll_by(&mut self, delta: isize, height: usize) {
        let max = self.rows().len().saturating_sub(height);
        self.scroll = (self.scroll as isize + delta).clamp(0, max as isize) as usize;
    }

    pub fn toggle(&mut self, i: usize, height: usize) {
        if self.has_children(i) && self.filter.is_empty() {
            self.open[i] = !self.open[i];
        }
        self.settle(height);
    }

    fn expand(&mut self, height: usize) {
        let i = self.selected;
        if !self.has_children(i) {
            return;
        }
        if self.is_open(i) {
            self.move_by(1, height);
        } else {
            self.open[i] = true;
            self.settle(height);
        }
    }

    fn collapse(&mut self, height: usize) {
        let i = self.selected;
        if self.has_children(i) && self.is_open(i) && self.filter.is_empty() {
            self.open[i] = false;
        } else if let Some(p) = parent(&self.entries, i) {
            self.selected = p;
        }
        self.settle(height);
    }

    fn set_all(&mut self, open: bool, height: usize) {
        self.open.iter_mut().for_each(|o| *o = open);
        self.settle(height);
    }

    /* Applies a key; `height` is how many rows the panel shows */
    pub fn key(&mut self, key: KeyEvent, height: usize) -> TocKey {
        if self.editing {
            match edit_prompt(&mut self.filter, key) {
                /* Select a match, not a parent shown only around one */
                PromptKey::Edited => {
                    if !self.filter.is_empty() && !self.matches(self.selected) {
                        if let Some(i) =
                            (0..self.entries.len()).find(|&i| self.matches(i))
                        {
                            self.selected = i;
                        }
                    }
                }
                PromptKey::Cancel => {
                    self.filter.clear();
                    self.editing = false;
                }
                PromptKey::Submit => self.editing = false,
            }
            self.scroll = 0;
            self.settle(height);
            return TocKey::Handled;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let half = (height / 2).max(1) as isize;

        if std::mem::take(&mut self.pending_z) {
            match key.code {
                KeyCode::Char('M') => self.set_all(false, height),
                KeyCode::Char('R') => self.set_all(true, height),
                KeyCode::Char('a') => self.toggle(self.selected, height),
                KeyCode::Char('c') => self.collapse(height),
                KeyCode::Char('o') => self.expand(height),
                _ => (),
            }
            return TocKey::Handled;
        }

        match key.code {
            KeyCode::Char('d') if ctrl => self.move_by(half, height),
            KeyCode::Char('u') if ctrl => self.move_by(-half, height),
            _ if ctrl => return TocKey::Other,
            KeyCode::Char('j') | KeyCode::Down => self.move_by(1, height),
            KeyCode::Char('k') | KeyCode::Up => self.move_by(-1, height),
            KeyCode::Char('d') | KeyCode::PageDown => self.move_by(half, height),
            KeyCode::Char('u') | KeyCode::PageUp => self.move_by(-half, height),
            KeyCode::Char('g') | KeyCode::Home => self.move_by(isize::MIN / 2, height),
            KeyCode::Char('G') | KeyCode::End => self.move_by(isize::MAX / 2, height),
            KeyCode::Char('l') | KeyCode::Right => self.expand(height),
            KeyCode::Char('h') | KeyCode::Left => self.collapse(height),
            KeyCode::Char('z') => self.pending_z = true,
            KeyCode::Char('/') => {
                self.filter.clear();
                self.editing = true;
            }
            KeyCode::Enter => return TocKey::Jump { close: true },
            KeyCode::Char(' ') => return TocKey::Jump { close: false },
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.settle(height);
            }
            KeyCode::Esc | KeyCode::Tab | KeyCode::Char('q') | KeyCode::Char('o') => {
                return TocKey::Close
            }
            _ => return TocKey::Other,
        }
        TocKey::Handled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(title: &str, depth: usize, page: usize) -> OutlineEntry {
        OutlineEntry {
            title: title.to_owned(),
            depth,
            page: Some(page),
            top: None,
        }
    }

    /* 1 Intro (1.1, 1.2 (1.2.1)), 2 Methods (2.1), 3 Results */
    fn book() -> Toc {
        Toc::new(vec![
            entry("Intro", 0, 0),
            entry("Motivation", 1, 2),
            entry("Prior work", 1, 6),
            entry("Older work", 2, 7),
            entry("Methods", 0, 14),
            entry("Setup", 1, 15),
            entry("Results", 0, 39),
        ])
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn press(toc: &mut Toc, keys: &str) {
        for c in keys.chars() {
            toc.key(key(KeyCode::Char(c)), 10);
        }
    }

    #[test]
    fn closed_entries_hide_their_children() {
        let mut toc = book();
        assert_eq!(toc.rows(), [0, 4, 6]);
        toc.toggle(0, 10);
        assert_eq!(toc.rows(), [0, 1, 2, 4, 6]);
        toc.toggle(2, 10);
        assert_eq!(toc.rows(), [0, 1, 2, 3, 4, 6]);
        toc.toggle(0, 10);
        assert_eq!(toc.rows(), [0, 4, 6]);
    }

    #[test]
    fn opening_on_a_section_reveals_it() {
        let mut toc = book();
        toc.reveal(Some(3), 10);
        assert_eq!(toc.selected, 3);
        assert_eq!(toc.rows(), [0, 1, 2, 3, 4, 6]);
    }

    #[test]
    fn keys_walk_and_fold_the_tree() {
        let mut toc = book();
        press(&mut toc, "j");
        assert_eq!(toc.selected, 4);
        press(&mut toc, "l");
        assert_eq!(toc.rows(), [0, 4, 5, 6]);
        press(&mut toc, "l");
        assert_eq!(toc.selected, 5);
        /* h on a leaf goes to its parent, then closes it */
        press(&mut toc, "h");
        assert_eq!(toc.selected, 4);
        press(&mut toc, "h");
        assert_eq!(toc.rows(), [0, 4, 6]);
        press(&mut toc, "zR");
        assert_eq!(toc.rows().len(), 7);
        press(&mut toc, "G");
        assert_eq!(toc.selected, 6);
        press(&mut toc, "zM");
        assert_eq!(toc.rows(), [0, 4, 6]);
        press(&mut toc, "g");
        assert_eq!(toc.selected, 0);
    }

    #[test]
    fn folding_away_the_selection_selects_its_parent() {
        let mut toc = book();
        toc.reveal(Some(3), 10);
        press(&mut toc, "zM");
        assert_eq!(toc.selected, 0);
    }

    #[test]
    fn filter_keeps_matches_and_their_parents() {
        let mut toc = book();
        press(&mut toc, "/work");
        assert!(toc.editing);
        assert_eq!(toc.rows(), [0, 2, 3]);
        assert_eq!(toc.selected, 2);
        assert_eq!(toc.key(key(KeyCode::Enter), 10), TocKey::Handled);
        assert!(!toc.editing);
        assert_eq!(toc.key(key(KeyCode::Esc), 10), TocKey::Handled);
        assert_eq!(toc.rows(), [0, 4, 6]);
        assert_eq!(toc.key(key(KeyCode::Esc), 10), TocKey::Close);
    }

    #[test]
    fn opening_again_drops_the_filter() {
        let mut toc = book();
        press(&mut toc, "/old");
        toc.key(key(KeyCode::Enter), 10);
        toc.reveal(Some(4), 10);
        assert!(toc.filter.is_empty() && !toc.editing);
        assert_eq!(toc.selected, 4);
        assert_eq!(toc.rows(), [0, 4, 6]);
    }

    #[test]
    fn enter_and_space_jump() {
        let mut toc = book();
        assert_eq!(
            toc.key(key(KeyCode::Enter), 10),
            TocKey::Jump { close: true }
        );
        assert_eq!(
            toc.key(key(KeyCode::Char(' ')), 10),
            TocKey::Jump { close: false }
        );
        assert_eq!(toc.key(key(KeyCode::Char('x')), 10), TocKey::Other);
    }

    #[test]
    fn selection_stays_in_view() {
        let mut toc = book();
        press(&mut toc, "zR");
        press(&mut toc, "jjjj");
        /* Five rows down in a three row panel */
        toc.key(key(KeyCode::Char('j')), 3);
        assert_eq!(toc.selected, 5);
        assert_eq!(toc.scroll, 3);
    }

    #[test]
    fn current_section_is_the_last_one_started() {
        let mut entries = book().entries;
        entries[5].top = Some(300.0);
        assert_eq!(current_section(&entries, 0, 0.0), Some(0));
        assert_eq!(current_section(&entries, 7, 0.0), Some(3));
        assert_eq!(current_section(&entries, 15, 100.0), Some(4));
        assert_eq!(current_section(&entries, 15, 400.0), Some(5));
        assert_eq!(current_section(&entries, 100, 0.0), Some(6));
        assert_eq!(current_section(&[], 3, 0.0), None);
    }

    #[test]
    fn breadcrumb_lists_the_parents() {
        let entries = book().entries;
        assert_eq!(
            breadcrumb(&entries, 3),
            ["Intro", "Prior work", "Older work"]
        );
        assert_eq!(breadcrumb(&entries, 6), ["Results"]);
    }

    #[test]
    fn outline_is_read_from_a_pdf() {
        let bytes = crate::testutil::pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /Outlines 4 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
            "<< /Type /Outlines /First 5 0 R /Last 6 0 R /Count 2 >>",
            "<< /Title (Intro) /Parent 4 0 R /Next 6 0 R /First 7 0 R /Last 7 0 R \
             /Count 1 /Dest [3 0 R /XYZ 0 700 0] >>",
            "<< /Title <FEFF004D0065> /Parent 4 0 R /Prev 5 0 R /Dest [3 0 R /Fit] >>",
            "<< /Title (Set\\nup) /Parent 5 0 R /Dest [3 0 R /FitH 400] >>",
        ]);
        let document = mupdf::Document::from_bytes(&bytes, "application/pdf").unwrap();
        let entries = flatten(&document.outlines().unwrap());
        let titles: Vec<(&str, usize)> = entries
            .iter()
            .map(|e| (e.title.as_str(), e.depth))
            .collect();
        assert_eq!(titles, [("Intro", 0), ("Set up", 1), ("Me", 0)]);
        assert!(entries.iter().all(|e| e.page == Some(0)));
        /* PDF measures up from the bottom; the viewer measures down from the top */
        assert_eq!(entries[0].top, Some(92.0));
        assert_eq!(entries[1].top, Some(392.0));
        assert_eq!(entries[2].top, None);
    }

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(clean_title(" Chapter\n 1\tIntro "), "Chapter 1 Intro");
        assert_eq!(clean_title("  "), "(untitled)");
    }
}
