/* Note files: naming a note after its PDF, finding notes in the vault, writing a new
 * one from the template with a link back to the PDF, and keeping that link right
 * when the PDF moves */

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

/* `~` at the start of a path stands for the home folder */
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ if path == "~" => dirs::home_dir().unwrap_or_default(),
        _ => PathBuf::from(path),
    }
}

/* A note's name for a PDF: the title stored in it, or else its file name. Characters
 * that break file paths or [[links]] go, and so does surrounding space. */
pub fn note_name(title: Option<&str>, file_stem: &str) -> String {
    /* A book's title usually ends where its subtitle starts */
    let title = title.map(|t| t.split(':').next().unwrap_or(t));
    let clean = |text: &str| {
        let kept: String = text
            .chars()
            .map(|c| match c {
                '/' | '\\' | ':' | '#' | '|' | '[' | ']' | '^' | '*' | '?' | '"'
                | '<' | '>' => ' ',
                c if c.is_control() => ' ',
                c => c,
            })
            .collect();
        let words: Vec<&str> = kept.split_whitespace().collect();
        let name = words.join(" ");
        let name = name.trim_matches(|c: char| c == '.' || c == '-' || c == ' ');
        shorten(name, 80)
    };
    title
        .map(clean)
        .filter(|t| !t.is_empty())
        .or_else(|| Some(clean(file_stem)).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| "Untitled".to_owned())
}

/* `text` cut to at most `max` characters, at a space when there's one */
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rfind(' ') {
        Some(space) if space > max / 2 => {
            cut[..space].trim_end_matches([',', ';']).to_owned()
        }
        _ => cut,
    }
}

/* Folders a vault keeps for its tools, which hold no notes */
fn hidden(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules"
}

/* Every note in `vault`, the most recently changed first */
pub fn list_notes(vault: &Path) -> Vec<PathBuf> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut folders = vec![vault.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if hidden(&name) {
                continue;
            }
            match entry.file_type() {
                Ok(t) if t.is_dir() => folders.push(path),
                Ok(_) if path.extension().is_some_and(|e| e == "md") => {
                    let changed = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(SystemTime::UNIX_EPOCH);
                    found.push((changed, path));
                }
                _ => (),
            }
        }
    }
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, path)| path).collect()
}

/* The note in `vault` called `name`, wherever it is: links find notes by name, so
 * there should be only one */
pub fn find_note(vault: &Path, name: &str) -> Option<PathBuf> {
    let wanted = format!("{}.md", name).to_lowercase();
    list_notes(vault).into_iter().find(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().to_lowercase() == wanted)
    })
}

/* A Markdown link to the PDF that works whatever its path holds: spaces and brackets
 * are fine inside `<...>`, and the few characters that aren't are escaped */
pub fn pdf_link(title: &str, pdf: &Path) -> String {
    format!("[{}]({})", escape_text(title), link_target(pdf))
}

fn escape_text(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

fn link_target(path: &Path) -> String {
    let path = path
        .to_string_lossy()
        .replace('<', "%3C")
        .replace('>', "%3E")
        .replace(['\n', '\r'], "");
    format!("<{}>", path)
}

/* A new note's text. In the template, {{title}} becomes `name` and {{source}} the link
 * to the PDF. A template without {{source}} gets the link on its `Links:` line, or on
 * a line of its own after the title. Without a template: the title and the link. */
pub fn new_note(template: Option<&str>, name: &str, link: &str) -> String {
    let Some(template) = template else {
        return format!("# {}\n\nSource: {}\n", name, link);
    };
    let text = template.replace("{{title}}", name);
    if text.contains("{{source}}") {
        return text.replace("{{source}}", link);
    }
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    match lines
        .iter()
        .position(|l| l.trim_start().starts_with("Links:"))
    {
        Some(i) => {
            let line = lines[i].trim_end();
            let gap = if line.ends_with(':') { " " } else { ", " };
            lines[i] = format!("{}{}{}", line, gap, link);
        }
        None => lines.insert(lines.len().min(1), format!("Source: {}", link)),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/* Writes a new note at `path`, never over an existing file */
pub fn create_note(path: &Path, text: &str) -> Result<(), String> {
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)
            .map_err(|x| format!("Could not make {}: {}", folder.display(), x))?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .map_err(|x| format!("Could not write {}: {}", path.display(), x))
}

/* The note's text with links to the PDF at `old` pointing to `new`; `None` when it
 * has none */
pub fn move_link(text: &str, old: &Path, new: &Path) -> Option<String> {
    let (old, new) = (
        format!("]({})", link_target(old)),
        format!("]({})", link_target(new)),
    );
    text.contains(&old).then(|| text.replace(&old, &new))
}

/* helix's config for the helix in the panel only: it saves when focus leaves the panel,
 * and a second after typing stops, so a crash or a killed viewer loses next to nothing */
pub fn helix_config(user: Option<&str>) -> String {
    let mut doc = user
        .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        .unwrap_or_default();
    if !doc.get("editor").is_some_and(|e| e.is_table()) {
        doc.insert("editor", toml_edit::table());
    }
    let editor = doc["editor"].as_table_mut().expect("just made a table");
    let mut auto_save = match editor.remove("auto-save") {
        Some(toml_edit::Item::Table(table)) => table,
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(inline))) => {
            inline.into_table()
        }
        _ => toml_edit::Table::new(),
    };
    auto_save.insert("focus-lost", toml_edit::value(true));
    let mut delay = match auto_save.remove("after-delay") {
        Some(toml_edit::Item::Table(table)) => table,
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(inline))) => {
            inline.into_table()
        }
        _ => toml_edit::Table::new(),
    };
    delay.insert("enable", toml_edit::value(true));
    if !delay.contains_key("timeout") {
        delay.insert("timeout", toml_edit::value(1000));
    }
    auto_save.insert("after-delay", toml_edit::Item::Table(delay));
    editor.insert("auto-save", toml_edit::Item::Table(auto_save));
    doc.to_string()
}

/* Whether `editor` is helix, which gets the config above and is opened at a line */
pub fn is_helix(editor: &str) -> bool {
    matches!(
        Path::new(editor).file_name().and_then(|n| n.to_str()),
        Some("hx" | "helix")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_title_then_the_file() {
        assert_eq!(
            note_name(Some("A System for Writing"), "whatever"),
            "A System for Writing"
        );
        assert_eq!(
            note_name(Some("  "), "Smart Notes -- Ahrens -- ( WeLib.org ).epub"),
            "Smart Notes -- Ahrens -- ( WeLib.org ).epub"
        );
        assert_eq!(
            note_name(
                Some(
                    "A System for Writing: How an Unconventional Approach to Note-Making"
                ),
                "x"
            ),
            "A System for Writing"
        );
        assert_eq!(
            note_name(Some("How/Why [draft] #1"), "x"),
            "How Why draft 1"
        );
        let long = "word ".repeat(30);
        let name = note_name(None, &long);
        assert!(name.len() <= 80 && name.ends_with("word"), "{:?}", name);
        assert_eq!(note_name(None, "..."), "Untitled");
    }

    #[test]
    fn the_link_works_whatever_the_path() {
        let link = pdf_link("Book [1]", Path::new("/Users/me/PDF's/A (b) <c>.pdf"));
        assert_eq!(link, "[Book \\[1\\]](</Users/me/PDF's/A (b) %3Cc%3E.pdf>)");
    }

    #[test]
    fn the_template_is_kept_and_the_link_goes_on_its_links_line() {
        let template = "# {{title}}\n\nLinks: \nTags: \n";
        assert_eq!(
            new_note(Some(template), "Book", "[Book](</a.pdf>)"),
            "# Book\n\nLinks: [Book](</a.pdf>)\nTags: \n"
        );
        assert_eq!(
            new_note(
                Some("# {{title}}\n\nLinks: [[other]]\n"),
                "B",
                "[B](</a.pdf>)"
            ),
            "# B\n\nLinks: [[other]], [B](</a.pdf>)\n"
        );
        assert_eq!(
            new_note(Some("# {{title}}\nfrom {{source}}\n"), "B", "L"),
            "# B\nfrom L\n"
        );
        assert_eq!(
            new_note(Some("# {{title}}\n\nbody\n"), "B", "L"),
            "# B\nSource: L\n\nbody\n"
        );
        assert_eq!(new_note(None, "B", "L"), "# B\n\nSource: L\n");
    }

    #[test]
    fn a_moved_pdf_keeps_its_link() {
        let (old, new) = (Path::new("/old/a b.pdf"), Path::new("/new/a b.pdf"));
        let text = new_note(None, "A", &pdf_link("A", old));
        let moved = move_link(&text, old, new).unwrap();
        assert!(moved.contains("[A](</new/a b.pdf>)"), "{}", moved);
        assert_eq!(move_link(&moved, old, new), None);
    }

    #[test]
    fn the_panel_helix_saves_on_leaving_and_keeps_the_rest() {
        let user = "theme = \"x\"\n\n[editor]\nauto-save = false\nmouse = true\n";
        let config = helix_config(Some(user));
        let parsed: toml::Table = config.parse().unwrap();
        assert_eq!(parsed["theme"].as_str(), Some("x"));
        assert_eq!(parsed["editor"]["mouse"].as_bool(), Some(true));
        assert_eq!(
            parsed["editor"]["auto-save"]["focus-lost"].as_bool(),
            Some(true)
        );

        assert_eq!(
            parsed["editor"]["auto-save"]["after-delay"]["timeout"].as_integer(),
            Some(1000)
        );

        let kept = helix_config(Some(
            "[editor.auto-save]\nafter-delay.enable = false\nafter-delay.timeout = 300\n",
        ));
        let parsed: toml::Table = kept.parse().unwrap();
        assert_eq!(
            parsed["editor"]["auto-save"]["focus-lost"].as_bool(),
            Some(true)
        );
        let delay = &parsed["editor"]["auto-save"]["after-delay"];
        assert_eq!(delay["enable"].as_bool(), Some(true));
        assert_eq!(
            delay["timeout"].as_integer(),
            Some(300),
            "the user's delay stays"
        );

        let none: toml::Table = helix_config(None).parse().unwrap();
        assert_eq!(
            none["editor"]["auto-save"]["focus-lost"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn notes_are_found_by_name_anywhere_in_the_vault() {
        let vault =
            std::env::temp_dir().join(format!("viewmer-vault-{}", std::process::id()));
        fs::create_dir_all(vault.join("100 Writings").join("deep")).unwrap();
        fs::create_dir_all(vault.join(".obsidian")).unwrap();
        fs::write(vault.join("100 Writings").join("deep").join("Book.md"), "x").unwrap();
        fs::write(vault.join(".obsidian").join("Hidden.md"), "x").unwrap();
        fs::write(vault.join("other.txt"), "x").unwrap();

        assert_eq!(
            find_note(&vault, "book"),
            Some(vault.join("100 Writings").join("deep").join("Book.md"))
        );
        assert_eq!(find_note(&vault, "Hidden"), None);
        assert_eq!(list_notes(&vault).len(), 1);

        let new = vault.join("000 Rough Writes").join("New.md");
        create_note(&new, "a").unwrap();
        assert!(
            create_note(&new, "b").is_err(),
            "never over an existing note"
        );
        assert_eq!(fs::read_to_string(&new).unwrap(), "a");
        fs::remove_dir_all(&vault).unwrap();
    }
}
