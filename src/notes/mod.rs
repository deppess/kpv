/* The notes panel: a terminal beside the PDF, running the shell, where any program
 * runs as in any terminal (helix, yazi). viewmer adds a little to it: `hx` there saves
 * when focus leaves the panel, `pdfnote` opens the note named after the PDF, and page
 * tags go in and out of what it shows. */

pub mod keys;
pub mod note;
pub mod pane;
pub mod tags;

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crate::{config::ConfigNotes, globals::CONFIG, store::Store};
use note::{
    create_note, expand_home, find_note, helix_config, new_note, note_name, pdf_link,
};
use pane::Pane;

/* The terminal's screen: cells across and down, and its size in pixels */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorSize {
    pub cols: u16,
    pub rows: u16,
    pub pixels: (u16, u16),
}

pub struct Notes {
    /* The panel is on screen */
    pub open: bool,
    /* Keys go to the panel */
    pub focused: bool,
    /* Runs on after the panel hides, so it comes back as it was */
    pub terminal: Option<Pane>,
    pdf: PathBuf,
    /* The title stored in the PDF */
    pub title: Option<String>,
    /* The note kept with this PDF in its state, whose link to the PDF follows it */
    pub remembered: Option<PathBuf>,
}

fn settings() -> &'static ConfigNotes {
    &CONFIG.get().unwrap().notes
}

pub fn vault() -> PathBuf {
    expand_home(&settings().vault)
}

/* Where new notes go, and where the terminal starts */
pub fn notes_folder() -> PathBuf {
    let vault = vault();
    match settings().folder.trim() {
        "" => vault,
        folder => vault.join(folder),
    }
}

impl Notes {
    pub fn new(pdf: &Path) -> Self {
        Self {
            open: false,
            focused: false,
            terminal: None,
            pdf: std::fs::canonicalize(pdf).unwrap_or_else(|_| pdf.to_path_buf()),
            title: None,
            remembered: None,
        }
    }

    /* Opens the panel and gives it the keys, starting the shell the first time */
    pub fn focus(&mut self, size: EditorSize) -> Result<(), String> {
        self.open = true;
        self.focused = true;
        match &mut self.terminal {
            Some(terminal) => terminal.focus(true),
            None => self.start(size)?,
        }
        Ok(())
    }

    /* Gives the keys back to the PDF; helix in the panel saves on losing focus */
    pub fn unfocus(&mut self) {
        if !self.focused {
            return;
        }
        self.focused = false;
        if let Some(terminal) = &mut self.terminal {
            terminal.focus(false);
        }
    }

    /* Hides the panel; what runs in it goes on, saved */
    pub fn hide(&mut self) {
        self.unfocus();
        self.open = false;
    }

    fn start(&mut self, size: EditorSize) -> Result<(), String> {
        let shell = match settings().shell.trim() {
            "" => login_shell(),
            shell => shell.to_owned(),
        };
        let folder = notes_folder();
        let cwd = if folder.is_dir() {
            folder
        } else {
            dirs::home_dir().unwrap_or_default()
        };
        let mut env = vec![("VIEWMER_PDF".to_owned(), self.pdf.display().to_string())];
        let mut args = Vec::new();
        if let Some(bin) = write_helpers() {
            let path = std::env::var("PATH").unwrap_or_default();
            env.push(("PATH".to_owned(), format!("{}:{}", bin.display(), path)));
            /* fish puts its own paths first while it starts; put the helpers back in front */
            if shell.ends_with("fish") {
                args.push("-C".to_owned());
                args.push(format!(
                    "set -gx PATH {} $PATH",
                    quote(&bin.display().to_string())
                ));
            }
        }
        let mut pane =
            Pane::spawn(&shell, &args, &cwd, size.cols, size.rows, size.pixels, &env)?;
        pane.focus(self.focused);
        self.terminal = Some(pane);
        Ok(())
    }

    /* On quitting: tells what runs in the panel that focus left, so helix saves, gives
     * it a moment, and ends it */
    pub fn close(&mut self) {
        let Some(mut terminal) = self.terminal.take() else {
            return;
        };
        terminal.focus(false);
        let end = Instant::now() + Duration::from_millis(400);
        while !terminal.exited() && Instant::now() < end {
            terminal.take_output();
            thread::sleep(Duration::from_millis(10));
        }
        terminal.kill();
    }

    /* The shell ended (`exit`): the panel closes */
    pub fn ended(&mut self) {
        self.terminal = None;
        self.open = false;
        self.focused = false;
    }
}

/* The shell the account logs in with, then $SHELL, then sh */
fn login_shell() -> String {
    let from_account = unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if entry.is_null() || (*entry).pw_shell.is_null() {
            None
        } else {
            std::ffi::CStr::from_ptr((*entry).pw_shell)
                .to_str()
                .ok()
                .map(str::to_owned)
        }
    };
    from_account
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".to_owned())
}

/* The note's name for a PDF: the title stored in it, or else its file name */
pub fn suggested_name(pdf: &Path, title: Option<&str>) -> String {
    let stem = pdf
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    note_name(title, &stem)
}

/* The note named `name` (or after the PDF): the one the vault has, wherever it is,
 * since links find notes by name, or else a new one in the notes folder from the
 * template with a link back to the PDF. Returns where it is. */
pub fn note_for(
    pdf: &Path,
    title: Option<&str>,
    name: Option<&str>,
) -> Result<PathBuf, String> {
    let suggested = suggested_name(pdf, title);
    let name = name
        .map(|n| n.trim().trim_end_matches(".md").trim().to_owned())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| suggested.clone());
    if let Some(existing) = find_note(&vault(), &name) {
        return Ok(existing);
    }
    let path = notes_folder().join(format!("{}.md", name));
    let template =
        match settings().template.trim() {
            "" => None,
            file => Some(std::fs::read_to_string(expand_home(file)).map_err(|x| {
                format!("Could not read the note template {}: {}", file, x)
            })?),
        };
    let link = pdf_link(&suggested, pdf);
    create_note(&path, &new_note(template.as_deref(), &name, &link))?;
    Ok(path)
}

/* The commands the panel's shell finds first: `hx` with saving on losing focus, and
 * `pdfnote`. Returns their folder. */
fn write_helpers() -> Option<PathBuf> {
    let dir = Store::default_dir()?;
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).ok()?;

    let editor = match settings().editor.trim() {
        "" => "hx".to_owned(),
        editor => editor.to_owned(),
    };
    let mut scripts = Vec::new();
    /* helix itself, past this folder in the PATH */
    if note::is_helix(&editor) {
        if let Some(real) = find_program(&editor, &bin) {
            let user = dirs::home_dir()?.join(".config/helix/config.toml");
            let config = dir.join("helix.toml");
            std::fs::write(
                &config,
                helix_config(std::fs::read_to_string(user).ok().as_deref()),
            )
            .ok()?;
            scripts.push((
                "hx".to_owned(),
                format!(
                    "#!/bin/sh\n# helix in viewmer's notes panel: it saves when focus leaves the panel\nexec {} -c {} \"$@\"\n",
                    quote(&real.display().to_string()),
                    quote(&config.display().to_string())
                ),
            ));
        }
    }
    let viewmer = std::env::current_exe().ok()?;
    scripts.push((
        "pdfnote".to_owned(),
        format!(
            "#!/bin/sh\n# Opens the note named after the PDF in viewmer, or the one named \"$*\",\n# making it from the template first, at its end\nnote=$({} --note \"$VIEWMER_PDF\" \"$@\") || exit 1\n{}\n",
            quote(&viewmer.display().to_string()),
            if note::is_helix(&editor) {
                format!(
                    "exec {} \"$note:$(($(wc -l < \"$note\") + 1))\"",
                    quote(&editor)
                )
            } else {
                format!("exec {} \"$note\"", quote(&editor))
            }
        ),
    ));
    for (name, text) in scripts {
        let path = bin.join(name);
        std::fs::write(&path, text).ok()?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(bin)
}

/* Where `program` is in the PATH, not counting `skip` */
fn find_program(program: &str, skip: &Path) -> Option<PathBuf> {
    if program.contains('/') {
        return Some(PathBuf::from(program));
    }
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(PathBuf::from)
        .filter(|dir| dir != skip)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}

/* `text` in single quotes for the shell */
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quoting_survives_any_path() {
        assert_eq!(quote("/a/PDF's/x"), "'/a/PDF'\\''s/x'");
    }
}
