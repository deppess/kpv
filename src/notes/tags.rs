/* Page tags in a note: `(p. 45)`, which viewmer writes, and what people type by hand:
 * `p. 45`, `p.45`, `pp. 45-47` (the first page), `p. xiv`, `p. A-3`. The page is the
 * one printed on it, as the status bar shows it; `p. #30` is the 30th page of the
 * file, for a page with no number printed on it. */

/* A tag in a line: where it starts and ends, in characters, and the page it names */
#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub start: usize,
    pub end: usize,
    pub page: String,
}

/* The tag viewmer writes for page `label` */
pub fn tag(label: &str) -> String {
    format!("(p. {})", label)
}

fn is_roman(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            matches!(
                c.to_ascii_lowercase(),
                'i' | 'v' | 'x' | 'l' | 'c' | 'd' | 'm'
            )
        })
}

/* Every tag in `line` */
pub fn tags(line: &str) -> Vec<Tag> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        /* `p.` or `pp.`, not inside a word such as `shop.` */
        let starts_word = i == 0 || !chars[i - 1].is_alphanumeric();
        let marker = if chars[i] != 'p' || !starts_word {
            None
        } else if chars.get(i + 1) == Some(&'.') {
            Some(2)
        } else if chars.get(i + 1) == Some(&'p') && chars.get(i + 2) == Some(&'.') {
            Some(3)
        } else {
            None
        };
        let Some(marker) = marker else {
            i += 1;
            continue;
        };
        let mut j = i + marker;
        while chars.get(j) == Some(&' ') {
            j += 1;
        }
        let token_start = j;
        /* `#30`: a page by its place in the file */
        if chars.get(j) == Some(&'#') {
            j += 1;
        }
        while chars
            .get(j)
            .is_some_and(|c| c.is_alphanumeric() || *c == '-')
        {
            j += 1;
        }
        let token: String = chars[token_start..j].iter().collect();
        let token = token.trim_end_matches('-');
        /* `45-47` names a span; the tag goes to its first page */
        let page = match token.split_once('-') {
            Some((a, b))
                if a.chars().all(|c| c.is_ascii_digit())
                    && b.chars().all(|c| c.is_ascii_digit())
                    && !a.is_empty() =>
            {
                a
            }
            _ => token,
        };
        let names_a_page = page.chars().any(|c| c.is_ascii_digit()) || is_roman(page);
        if !names_a_page {
            i += 1;
            continue;
        }
        /* Whole parentheses around it belong to the tag */
        let mut start = i;
        let mut end = j;
        if start > 0 && chars[start - 1] == '(' {
            start -= 1;
            while chars.get(end).is_some_and(|c| *c != ')' && *c != '(') && end < j + 12 {
                end += 1;
            }
            if chars.get(end) == Some(&')') {
                end += 1;
            }
        }
        found.push(Tag {
            start,
            end,
            page: page.to_owned(),
        });
        i = j.max(i + 1);
    }
    found
}

/* The tag under column `col` of `line`, if one is */
pub fn tag_at(line: &str, col: usize) -> Option<String> {
    tags(line)
        .into_iter()
        .find(|t| t.start <= col && col < t.end)
        .map(|t| t.page)
}

/* The tag that applies at row `row`, column `col` of `rows`: on that row the last one
 * starting at or before the column, or else the first; otherwise the nearest one on a
 * row above */
pub fn nearest_tag(rows: &[String], row: usize, col: usize) -> Option<String> {
    let here = rows.get(row).map(|r| tags(r)).unwrap_or_default();
    if let Some(tag) = here
        .iter()
        .rev()
        .find(|t| t.start <= col)
        .or_else(|| here.first())
    {
        return Some(tag.page.clone());
    }
    rows.iter()
        .take(row.min(rows.len()))
        .rev()
        .find_map(|r| tags(r).pop())
        .map(|t| t.page)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(line: &str) -> Vec<String> {
        tags(line).into_iter().map(|t| t.page).collect()
    }

    #[test]
    fn the_tags_people_write() {
        assert_eq!(pages("as he says (p. 45), and"), ["45"]);
        assert_eq!(pages("p.45 then p. 46"), ["45", "46"]);
        assert_eq!(pages("see pp. 45-47"), ["45"]);
        assert_eq!(pages("(p. xiv) the foreword"), ["xiv"]);
        assert_eq!(pages("appendix p. A-3"), ["A-3"]);
        assert_eq!(pages("(p. #30) before page one"), ["#30"]);
    }

    #[test]
    fn words_that_look_like_tags_are_not() {
        assert!(pages("the shop. 5 apples").is_empty());
        assert!(pages("p. the end").is_empty());
        assert!(pages("help.45").is_empty());
        assert!(pages("   12  plain line").is_empty());
    }

    #[test]
    fn the_tag_includes_its_parentheses() {
        let t = &tags("x (p. 45) y")[0];
        assert_eq!((t.start, t.end), (2, 9));
        assert_eq!(tag_at("x (p. 45) y", 5), Some("45".to_owned()));
        assert_eq!(tag_at("x (p. 45) y", 10), None);
        assert_eq!(tag("xiv"), "(p. xiv)");
    }

    #[test]
    fn the_nearest_tag_is_on_the_line_or_above() {
        let rows: Vec<String> = ["(p. 3) intro", "text", "(p. 9) a (p. 10) b", "more"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(nearest_tag(&rows, 2, 12), Some("10".to_owned()));
        assert_eq!(nearest_tag(&rows, 2, 0), Some("9".to_owned()));
        assert_eq!(nearest_tag(&rows, 3, 0), Some("10".to_owned()));
        assert_eq!(nearest_tag(&rows, 1, 0), Some("3".to_owned()));
        assert_eq!(nearest_tag(&rows[1..2], 0, 0), None);
    }
}
