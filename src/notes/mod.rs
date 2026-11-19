/* The notes panel: an editor beside the PDF, on the note that belongs to it */

pub mod keys;
pub mod note;
pub mod pane;
pub mod picker;
pub mod tags;

use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{config::ConfigNotes, globals::CONFIG, store::Store};
use note::{
    create_note, expand_home, find_note, helix_config, is_helix, last_line, list_notes,
    new_note, note_name, pdf_link,
};
use pane::Pane;
use picker::{Choice, Picker};

/* The editor's screen: cells across and down, and its size in pixels */
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
    pub editor: Option<Pane>,
    /* The note in the editor */
    pub note: Option<PathBuf>,
    /* Shown instead of the editor while a note is being chosen */
    pub picker: Option<Picker>,
    pdf: PathBuf,
    /* The title stored in the PDF */
    pub title: Option<String>,
    /* The note this PDF had last time */
    pub remembered: Option<PathBuf>,
}

fn settings() -> &'static ConfigNotes {
    &CONFIG.get().unwrap().notes
}

pub fn vault() -> PathBuf {
    expand_home(&settings().vault)
}

impl Notes {
    pub fn new(pdf: &Path) -> Self {
        Self {
            open: false,
            focused: false,
            editor: None,
            note: None,
            picker: None,
            pdf: std::fs::canonicalize(pdf).unwrap_or_else(|_| pdf.to_path_buf()),
            title: None,
            remembered: None,
        }
    }

    /* The note's name made from the PDF */
    pub fn suggested_name(&self) -> String {
        let stem = self
            .pdf
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        note_name(self.title.as_deref(), &stem)
    }

    fn picker(&self) -> Picker {
        let vault = vault();
        let name = self.suggested_name();
        Picker::new(&vault, &name, find_note(&vault, &name), list_notes(&vault))
    }

    /* Opens the panel and gives it the keys: the editor if it runs, else the note the
     * PDF had last time, else the picker */
    pub fn focus(&mut self, size: EditorSize) -> Result<(), String> {
        self.open = true;
        self.focused = true;
        if self.picker.is_some() {
            return Ok(());
        }
        if let Some(editor) = &mut self.editor {
            editor.focus(true);
            return Ok(());
        }
        match self.remembered.clone().filter(|p| p.is_file()) {
            Some(path) => self.start(&path, size),
            None => {
                self.picker = Some(self.picker());
                Ok(())
            }
        }
    }

    /* Gives the keys back to the PDF; the editor saves on losing focus */
    pub fn unfocus(&mut self) {
        if !self.focused {
            return;
        }
        self.focused = false;
        if let Some(editor) = &mut self.editor {
            editor.focus(false);
        }
    }

    /* Hides the panel; the editor runs on, saved */
    pub fn hide(&mut self) {
        self.unfocus();
        self.open = false;
        self.picker = None;
    }

    /* Shows the picker to choose another note, or with `naming` to name a new one */
    pub fn choose_another(&mut self, naming: bool) {
        let picker = self.picker();
        self.picker = Some(if naming { picker.naming() } else { picker });
        self.open = true;
        self.focused = true;
    }

    /* Opens what the picker chose, making a new note first if it was one. Returns the
     * note, to remember for this PDF. */
    pub fn chosen(
        &mut self,
        choice: Choice,
        size: EditorSize,
    ) -> Result<PathBuf, String> {
        self.picker = None;
        let path = match choice {
            Choice::Open(path) => path,
            Choice::New(name) => {
                let vault = vault();
                match find_note(&vault, &name) {
                    /* Links find notes by name, so a name is used once */
                    Some(existing) => existing,
                    None => {
                        let path =
                            vault.join(&settings().folder).join(format!("{}.md", name));
                        let template = match settings().template.trim() {
                            "" => None,
                            file => {
                                Some(std::fs::read_to_string(expand_home(file)).map_err(
                                    |x| {
                                        format!(
                                            "Could not read the note template {}: {}",
                                            file, x
                                        )
                                    },
                                )?)
                            }
                        };
                        let link = pdf_link(&self.suggested_name(), &self.pdf);
                        create_note(&path, &new_note(template.as_deref(), &name, &link))?;
                        path
                    }
                }
            }
        };
        if self.note.as_ref() != Some(&path) || self.editor.is_none() {
            self.close_editor();
            self.start(&path, size)?;
        }
        self.remembered = Some(path.clone());
        Ok(path)
    }

    fn start(&mut self, path: &Path, size: EditorSize) -> Result<(), String> {
        let editor = settings().editor.trim();
        let editor = if editor.is_empty() { "hx" } else { editor };
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let args = if is_helix(editor) {
            let mut args = Vec::new();
            if let Some(config) = write_helix_config() {
                args.push("-c".to_owned());
                args.push(config.to_string_lossy().into_owned());
            }
            args.push(format!("{}:{}", path.display(), last_line(&text)));
            args
        } else {
            vec![path.to_string_lossy().into_owned()]
        };
        let vault = vault();
        let cwd = if vault.is_dir() {
            vault
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or_default()
        };
        let mut pane =
            Pane::spawn(editor, &args, &cwd, size.cols, size.rows, size.pixels)?;
        pane.focus(self.focused);
        self.editor = Some(pane);
        self.note = Some(path.to_path_buf());
        Ok(())
    }

    /* Saves and quits the editor, waiting a little for it; one that won't quit is
     * stopped */
    pub fn close_editor(&mut self) {
        let Some(mut editor) = self.editor.take() else {
            return;
        };
        self.note = None;
        if is_helix(&settings().editor) {
            let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
            editor.key(&esc);
            editor.key(&esc);
            editor.send(b":write-quit-all\r");
        }
        let end = Instant::now() + Duration::from_secs(2);
        while !editor.exited() && Instant::now() < end {
            editor.take_output();
            thread::sleep(Duration::from_millis(10));
        }
        if !editor.exited() {
            editor.kill();
        }
    }

    /* After the editor quit by itself (`:q`): the panel closes */
    pub fn editor_ended(&mut self) {
        self.editor = None;
        self.note = None;
        self.open = false;
        self.focused = false;
    }
}

/* The config for the helix in the panel: the user's, saving on losing focus */
fn write_helix_config() -> Option<PathBuf> {
    let user = dirs::home_dir()?.join(".config/helix/config.toml");
    let text = helix_config(std::fs::read_to_string(user).ok().as_deref());
    let dir = Store::default_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("helix.toml");
    std::fs::write(&path, text).ok()?;
    Some(path)
}
