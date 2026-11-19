/* Page numbers printed on the pages, for PDFs that don't say how their pages are
 * numbered. The numbers in the top and bottom margins of a sample of pages vote for
 * how far into the file printed page 1 is; when enough of them agree, that offset
 * numbers the pages as the book does, the way `:offset` would. */

use mupdf::{Document, TextPageFlags};

/* How much of the page's height at the top and at the bottom counts as its margins */
const MARGIN: f32 = 0.1;
/* Pages to look at, spread over the file */
const SAMPLES: usize = 40;

/* The whole numbers on their own in the margins of page `index`: `12`, or the `12` in
 * `Chapter 2 | 12` */
pub fn margin_numbers(document: &Document, index: usize) -> Vec<u32> {
    let Ok(page) = document.load_page(index as i32) else {
        return Vec::new();
    };
    let (Ok(bounds), Ok(text)) =
        (page.bounds(), page.to_text_page(TextPageFlags::empty()))
    else {
        return Vec::new();
    };
    let height = (bounds.y1 - bounds.y0).max(1.0);
    let mut numbers = Vec::new();
    for block in text.blocks() {
        for line in block.lines() {
            let b = line.bounds();
            let (top, bottom) =
                ((b.y0 - bounds.y0) / height, (b.y1 - bounds.y0) / height);
            if top > MARGIN && bottom < 1.0 - MARGIN {
                continue;
            }
            let text: String = line.chars().filter_map(|c| c.char()).collect();
            numbers.extend(
                text.split(|c: char| c.is_whitespace() || "|·•—–-/".contains(c))
                    .filter(|t| {
                        (1..=4).contains(&t.len())
                            && t.chars().all(|c| c.is_ascii_digit())
                    })
                    .filter_map(|t| t.parse::<u32>().ok())
                    .filter(|n| *n >= 1),
            );
        }
    }
    numbers
}

/* How far into the file printed page 1 is, from the numbers found on `samples` (each a
 * page's index and its margin numbers), when the pages agree clearly enough */
pub fn agreed_offset(samples: &[(usize, Vec<u32>)]) -> Option<i32> {
    let mut votes: std::collections::HashMap<i32, usize> =
        std::collections::HashMap::new();
    let mut numbered = 0;
    for (index, numbers) in samples {
        let mut offsets: Vec<i32> = numbers
            .iter()
            .map(|n| *index as i32 + 1 - *n as i32)
            .collect();
        offsets.sort_unstable();
        offsets.dedup();
        if !offsets.is_empty() {
            numbered += 1;
        }
        for offset in offsets {
            *votes.entry(offset).or_default() += 1;
        }
    }
    let (offset, count) = votes
        .into_iter()
        .max_by_key(|(offset, count)| (*count, -offset.abs()))?;
    /* Most of the pages that show numbers, and a fair share of all pages looked at */
    let clear = count >= 4 && count * 2 > numbered && count * 4 >= samples.len();
    clear.then_some(offset)
}

/* The offset for a whole document, read from a sample of its pages */
pub fn detect(document: &Document) -> Option<i32> {
    let pages = document.page_count().ok()?.max(0) as usize;
    if pages < 4 {
        return None;
    }
    let step = (pages as f32 / SAMPLES as f32).max(1.0);
    let samples: Vec<(usize, Vec<u32>)> = (0..SAMPLES.min(pages))
        .map(|i| ((i as f32 * step) as usize).min(pages - 1))
        .map(|index| (index, margin_numbers(document, index)))
        .collect();
    agreed_offset(&samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(range: std::ops::Range<usize>, offset: i32) -> Vec<(usize, Vec<u32>)> {
        range
            .map(|i| (i, vec![(i as i32 + 1 - offset) as u32]))
            .collect()
    }

    #[test]
    fn a_book_numbered_from_its_twelfth_page() {
        let mut samples = vec![(0, vec![]), (1, vec![]), (2, vec![2024])];
        samples.extend(pages(11..40, 11));
        assert_eq!(agreed_offset(&samples), Some(11));
    }

    #[test]
    fn pages_numbered_as_their_position_agree_on_nothing_to_change() {
        assert_eq!(agreed_offset(&pages(0..30, 0)), Some(0));
    }

    #[test]
    fn no_numbers_or_scattered_ones_change_nothing() {
        let none: Vec<(usize, Vec<u32>)> = (0..30).map(|i| (i, vec![])).collect();
        assert_eq!(agreed_offset(&none), None);
        /* Years, figure numbers and the like don't line up with the pages */
        let scattered: Vec<(usize, Vec<u32>)> = (0..30)
            .map(|i| (i, vec![(i as u32 * 7) % 50 + 1]))
            .collect();
        assert_eq!(agreed_offset(&scattered), None);
        /* A few pages that happen to agree are not enough */
        let mut few = pages(0..3, 5);
        few.extend((3..30).map(|i| (i, vec![])));
        assert_eq!(agreed_offset(&few), None);
    }

    #[test]
    fn a_book_with_a_gap_still_agrees_on_most_pages() {
        let mut samples = pages(10..30, 9);
        /* Plates without numbers, and a part where numbering jumps */
        samples.extend((30..34).map(|i| (i, vec![])));
        samples.extend(pages(34..38, 40));
        assert_eq!(agreed_offset(&samples), Some(9));
    }

    /* NUMBERS_PDF=<file> cargo test numbers_of_a_real_pdf -- --ignored --nocapture */
    #[test]
    #[ignore]
    fn numbers_of_a_real_pdf() {
        let file = std::env::var("NUMBERS_PDF").unwrap();
        let document = Document::open(&file).unwrap();
        let start = std::time::Instant::now();
        let offset = detect(&document);
        println!("offset {:?} in {:?}", offset, start.elapsed());
    }
}
