use mupdf::{pdf::PdfDocument, pdf::PdfObject, Document};

/* How the numbers of a label range are written */
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Style {
    Decimal,
    RomanUpper,
    RomanLower,
    LettersUpper,
    LettersLower,
    /* Prefix only */
    None,
}

/* Pages from `start` (0-indexed) on are labelled `prefix` + `first`, `first + 1`, … */
#[derive(Clone, Debug, PartialEq)]
pub struct LabelRange {
    pub start: usize,
    pub style: Style,
    pub prefix: String,
    pub first: u32,
}

/* The printed label of every page. Pages before the first range, or every page when
 * there are no ranges, get their 1-indexed position. */
pub fn labels(ranges: &[LabelRange], pages: usize) -> Vec<String> {
    let mut ranges: Vec<&LabelRange> =
        ranges.iter().filter(|r| r.start < pages).collect();
    ranges.sort_by_key(|r| r.start);

    (0..pages)
        .map(|page| match ranges.iter().rev().find(|r| r.start <= page) {
            Some(r) => {
                let n = r.first.saturating_add((page - r.start) as u32);
                format!("{}{}", r.prefix, number(r.style, n))
            }
            None => (page + 1).to_string(),
        })
        .collect()
}

fn number(style: Style, n: u32) -> String {
    match style {
        Style::Decimal => n.to_string(),
        Style::RomanUpper => roman(n),
        Style::RomanLower => roman(n).to_lowercase(),
        Style::LettersUpper => letters(n),
        Style::LettersLower => letters(n).to_lowercase(),
        Style::None => String::new(),
    }
}

pub fn roman(mut n: u32) -> String {
    const NUMERALS: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, numeral) in NUMERALS {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/* A to Z, then AA to ZZ, then AAA and so on, as the PDF spec defines it */
pub fn letters(n: u32) -> String {
    if n == 0 {
        return String::new();
    }
    let letter = (b'A' + ((n - 1) % 26) as u8) as char;
    letter.to_string().repeat(((n - 1) / 26 + 1) as usize)
}

/* Decodes a PDF text string: UTF-16 or UTF-8 with a byte order mark, otherwise
 * PDFDocEncoding, read as Latin-1 */
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |bytes: &[u8], big: bool| {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| {
                if big {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => bytes.iter().map(|&b| b as char).collect(),
    }
}

/* The page that carries `wanted` as its label. An exact match wins over one that only
 * differs in case, and the first such page wins over later ones. */
pub fn find(labels: &[String], wanted: &str) -> Option<usize> {
    labels
        .iter()
        .position(|l| l == wanted)
        .or_else(|| labels.iter().position(|l| l.eq_ignore_ascii_case(wanted)))
}

/* Whether `labels` are just the pages' positions, as when the PDF has none */
pub fn is_plain(labels: &[String]) -> bool {
    labels
        .iter()
        .enumerate()
        .all(|(i, l)| *l == (i + 1).to_string())
}

/* Labels for a scan whose printed page 1 is `offset` pages into the file. The pages
 * before it are named by position, as `:#3` names them. */
pub fn with_offset(pages: usize, offset: i32) -> Vec<String> {
    (0..pages)
        .map(|p| {
            let printed = p as i64 + 1 - offset as i64;
            if printed >= 1 {
                printed.to_string()
            } else {
                format!("#{}", p + 1)
            }
        })
        .collect()
}

/* The page for the status bar: `xiv (14/412)`, or just `45/412` when the label is the
 * page's position anyway */
pub fn status(labels: &[String], page: usize, pages: usize) -> String {
    let position = format!("{}/{}", page + 1, pages);
    match labels.get(page) {
        Some(label) if *label != (page + 1).to_string() => {
            format!("{} ({})", label, position)
        }
        _ => position,
    }
}

/* Reads the label ranges from the document's /PageLabels number tree. A document
 * without one, or one that can't be read, has no ranges. */
pub fn read(document: &Document) -> Vec<LabelRange> {
    let mut ranges = Vec::new();
    let Ok(pdf) = PdfDocument::try_from(document.clone()) else {
        return ranges;
    };
    if let Ok(Some(tree)) = pdf.catalog().and_then(|c| c.get_dict("PageLabels")) {
        collect(&tree, &mut ranges, 0);
    }
    ranges
}

fn collect(node: &PdfObject, ranges: &mut Vec<LabelRange>, depth: usize) {
    /* A broken file could loop its tree back on itself */
    const MAX_DEPTH: usize = 32;
    if depth > MAX_DEPTH {
        return;
    }

    if let Ok(Some(nums)) = node.get_dict("Nums") {
        let len = nums.len().unwrap_or(0);
        for i in (0..len.saturating_sub(1)).step_by(2) {
            let Ok(Some(key)) = nums.get_array(i as i32) else {
                continue;
            };
            let Ok(Some(dict)) = nums.get_array(i as i32 + 1) else {
                continue;
            };
            let Ok(start) = key.as_int() else { continue };
            if start < 0 {
                continue;
            }
            ranges.push(range(start as usize, &dict));
        }
    }

    if let Ok(Some(kids)) = node.get_dict("Kids") {
        for i in 0..kids.len().unwrap_or(0) {
            if let Ok(Some(kid)) = kids.get_array(i as i32) {
                collect(&kid, ranges, depth + 1);
            }
        }
    }
}

fn range(start: usize, dict: &PdfObject) -> LabelRange {
    let style = match dict.get_dict("S") {
        Ok(Some(s)) => match s.as_name() {
            Ok(b"D") => Style::Decimal,
            Ok(b"R") => Style::RomanUpper,
            Ok(b"r") => Style::RomanLower,
            Ok(b"A") => Style::LettersUpper,
            Ok(b"a") => Style::LettersLower,
            _ => Style::None,
        },
        _ => Style::None,
    };
    let prefix = match dict.get_dict("P") {
        Ok(Some(p)) => p.as_bytes().map(decode_text).unwrap_or_default(),
        _ => String::new(),
    };
    let first = match dict.get_dict("St") {
        Ok(Some(st)) => st.as_int().ok().filter(|&n| n >= 1).unwrap_or(1) as u32,
        _ => 1,
    };
    LabelRange {
        start,
        style,
        prefix,
        first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::pdf;

    fn range(start: usize, style: Style, prefix: &str, first: u32) -> LabelRange {
        LabelRange {
            start,
            style,
            prefix: prefix.to_owned(),
            first,
        }
    }

    #[test]
    fn labels_are_read_from_a_nested_number_tree() {
        let page = "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>";
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R /PageLabels 6 0 R >>",
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R 7 0 R 8 0 R] /Count 5 >>",
            page,
            page,
            page,
            "<< /Kids [9 0 R 10 0 R] >>",
            page,
            page,
            "<< /Limits [0 0] /Nums [0 << /S /r >>] >>",
            "<< /Limits [2 4] /Nums [2 << /S /D /St 5 >> 4 << /S /D /P <FEFF0041002D> >>] >>",
        ]);
        let document = Document::from_bytes(&bytes, "application/pdf").unwrap();
        let pages = document.page_count().unwrap() as usize;
        assert_eq!(pages, 5);
        assert_eq!(
            labels(&read(&document), pages),
            ["i", "ii", "5", "6", "A-1"]
        );
    }

    #[test]
    fn documents_without_labels_count_pages() {
        let bytes = pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        ]);
        let document = Document::from_bytes(&bytes, "application/pdf").unwrap();
        assert!(read(&document).is_empty());
        assert_eq!(labels(&read(&document), 1), ["1"]);
    }

    #[test]
    fn roman_numerals() {
        assert_eq!(roman(1), "I");
        assert_eq!(roman(4), "IV");
        assert_eq!(roman(14), "XIV");
        assert_eq!(roman(1994), "MCMXCIV");
        assert_eq!(roman(0), "");
    }

    #[test]
    fn letters_repeat_after_z() {
        assert_eq!(letters(1), "A");
        assert_eq!(letters(26), "Z");
        assert_eq!(letters(27), "AA");
        assert_eq!(letters(53), "AAA");
    }

    #[test]
    fn book_with_front_matter() {
        let ranges = [
            range(0, Style::RomanLower, "", 1),
            range(4, Style::Decimal, "", 1),
            range(7, Style::Decimal, "A-", 1),
        ];
        assert_eq!(
            labels(&ranges, 9),
            ["i", "ii", "iii", "iv", "1", "2", "3", "A-1", "A-2"]
        );
    }

    #[test]
    fn start_value_and_prefix_only() {
        let ranges = [
            range(0, Style::None, "Cover", 1),
            range(1, Style::Decimal, "", 10),
        ];
        assert_eq!(labels(&ranges, 3), ["Cover", "10", "11"]);
    }

    #[test]
    fn unlabelled_pages_get_their_position() {
        assert_eq!(labels(&[], 3), ["1", "2", "3"]);
        /* Ranges given out of order or past the end */
        let ranges = [
            range(2, Style::RomanUpper, "", 1),
            range(50, Style::Decimal, "", 1),
        ];
        assert_eq!(labels(&ranges, 4), ["1", "2", "I", "II"]);
    }

    #[test]
    fn text_strings_decode() {
        assert_eq!(decode_text(b"A-"), "A-");
        assert_eq!(decode_text(&[0xFE, 0xFF, 0x00, 0x41, 0x00, 0xE9]), "Aé");
        assert_eq!(decode_text(&[0xFF, 0xFE, 0x41, 0x00]), "A");
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, 0xC3, 0xA9]), "é");
        assert_eq!(decode_text(&[0xE9]), "é");
    }

    #[test]
    fn labels_are_found_by_text() {
        let labels: Vec<String> = ["i", "ii", "1", "2", "1"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(find(&labels, "1"), Some(2));
        assert_eq!(find(&labels, "II"), Some(1));
        assert_eq!(find(&labels, "9"), None);
    }

    #[test]
    fn an_offset_numbers_a_scan() {
        assert_eq!(with_offset(4, 2), ["#1", "#2", "1", "2"]);
        assert_eq!(with_offset(2, 0), ["1", "2"]);
        assert_eq!(with_offset(2, -3), ["4", "5"]);
        assert!(is_plain(&with_offset(3, 0)));
        assert!(!is_plain(&with_offset(3, 1)));
    }

    #[test]
    fn status_shows_the_label_when_it_differs() {
        let labels: Vec<String> = ["xiv", "15"].iter().map(|s| s.to_string()).collect();
        assert_eq!(status(&labels, 0, 412), "xiv (1/412)");
        assert_eq!(status(&labels, 1, 412), "15 (2/412)");
        let plain: Vec<String> = ["1", "2"].iter().map(|s| s.to_string()).collect();
        assert_eq!(status(&plain, 1, 2), "2/2");
    }
}
