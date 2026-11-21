use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use mupdf::{pdf::PdfDocument, Document};
use serde::{Deserialize, Serialize};

use crate::globals::CONFIG_FILENAME;

const VERSION: u32 = 1;

/* What is remembered about one document. The position is relative to pages and to
 * the fit-width zoom, so it comes back right in a terminal of another size. The
 * version, the time and the keys are filled in when it is saved. */
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct FileState {
    pub version: u32,
    /* The page at the top of the viewport, 0-indexed */
    pub page: usize,
    /* How far down that page the viewport starts, from 0 to 1 */
    pub within: f32,
    /* 1 fits the page width */
    pub zoom: f32,
    /* Seconds since 1970 */
    #[serde(default)]
    pub opened: u64,
    /* What the file was found by, checked on loading */
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub layout: Layout,
}

/* How the window was laid out, so a file opens as it was left */
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Layout {
    /* The contents panel was open */
    #[serde(default)]
    pub contents: bool,
    /* How far right the view was, from 0 to 1 of the way, for pages wider than it */
    #[serde(default)]
    pub left: f32,
}

/* The state files of one document: one under its permanent PDF ID, one under its path.
 * The ID survives moving and renaming the file; the path covers files without an ID
 * and files that get a new one on every build, as LaTeX output does. */
pub struct Store {
    dir: PathBuf,
    id: Option<String>,
    path: String,
    saved: Option<FileState>,
}

impl Store {
    pub fn new(dir: PathBuf, id: Option<String>, file: &Path) -> Self {
        let path = fs::canonicalize(file)
            .unwrap_or_else(|_| file.to_owned())
            .to_string_lossy()
            .into_owned();
        Self {
            dir,
            id,
            path,
            saved: None,
        }
    }

    /* `~/.local/state/viewmer` on every system, or under $XDG_STATE_HOME when that's
     * set */
    pub fn default_dir() -> Option<PathBuf> {
        let base = match std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            Some(base) => PathBuf::from(base),
            None => std::env::home_dir()?.join(".local").join("state"),
        };
        Some(base.join(CONFIG_FILENAME))
    }

    /* A rebuilt file can come back with a new ID */
    pub fn set_id(&mut self, id: Option<String>) {
        self.id = id;
    }

    fn files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        if let Some(id) = &self.id {
            files.push(self.dir.join(format!("id-{:016x}.toml", fnv1a(id))));
        }
        files.push(
            self.dir
                .join(format!("path-{:016x}.toml", fnv1a(&self.path))),
        );
        files
    }

    /* The remembered state, found by ID first. A missing, unreadable or foreign file
     * is no state. */
    pub fn load(&mut self) -> Option<FileState> {
        let found = self.files().iter().find_map(|file| {
            let text = fs::read_to_string(file).ok()?;
            let state: FileState = toml::from_str(&text).ok()?;
            let ours = (self.id.is_some() && state.id == self.id)
                || state.path.as_deref() == Some(self.path.as_str());
            (state.version == VERSION && ours && state.within.is_finite())
                .then_some(state)
        })?;
        self.saved = Some(found.clone());
        Some(found)
    }

    /* Writes `state` when it differs from what was last written. Each file is written
     * whole to a temporary file and renamed over the old one, so a crash or a second
     * viewer never leaves half a file. */
    pub fn save(&mut self, state: &FileState) -> io::Result<()> {
        let mut state = self.filled(state);
        if self.saved.as_ref().is_some_and(|s| same_place(s, &state)) {
            return Ok(());
        }
        state.opened = now();

        let text = toml::to_string(&state).map_err(io::Error::other)?;
        fs::create_dir_all(&self.dir)?;
        for file in self.files() {
            let tmp = file.with_extension(format!("tmp{}", std::process::id()));
            let mut out = fs::File::create(&tmp)?;
            out.write_all(text.as_bytes())?;
            /* On the disk before it replaces the old one, so a crash or a power cut
             * leaves one or the other whole */
            out.sync_all()?;
            drop(out);
            fs::rename(&tmp, &file)?;
        }
        self.saved = Some(state);
        Ok(())
    }

    /* Whether `state` needs writing */
    pub fn is_dirty(&self, state: &FileState) -> bool {
        let state = self.filled(state);
        !self.saved.as_ref().is_some_and(|s| same_place(s, &state))
    }

    /* `state` as it is written, with the keys */
    fn filled(&self, state: &FileState) -> FileState {
        let mut state = state.clone();
        state.version = VERSION;
        state.id = self.id.clone();
        state.path = Some(self.path.clone());
        state
    }
}

/* Equal apart from when they were written; positions within a hair count as equal */
fn same_place(a: &FileState, b: &FileState) -> bool {
    a.page == b.page
        && (a.within - b.within).abs() < 1e-4
        && (a.zoom - b.zoom).abs() < 1e-4
        && a.layout == b.layout
        && a.id == b.id
        && a.path == b.path
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/* A hash that stays the same across builds, unlike the standard library's */
fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ byte as u64).wrapping_mul(0x100000001b3)
    })
}

/* The first, permanent half of the PDF's /ID, in hex */
pub fn document_id(document: &Document) -> Option<String> {
    let pdf = PdfDocument::try_from(document.clone()).ok()?;
    let id = pdf.trailer().ok()?.get_dict("ID").ok()??;
    let first = id.get_array(0).ok()??;
    let bytes = first.as_bytes().ok()?;
    (!bytes.is_empty()).then(|| bytes.iter().map(|b| format!("{:02x}", b)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /* A fresh directory for one test */
    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "viewmer-store-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn state(page: usize) -> FileState {
        FileState {
            page,
            within: 0.25,
            zoom: 1.5,
            ..FileState::default()
        }
    }

    #[test]
    fn state_comes_back() {
        let dir = dir("back");
        let mut store =
            Store::new(dir.clone(), Some("ab12".to_owned()), Path::new("a.pdf"));
        assert_eq!(store.load(), None);
        store.save(&state(7)).unwrap();

        let mut again =
            Store::new(dir.clone(), Some("ab12".to_owned()), Path::new("a.pdf"));
        let loaded = again.load().unwrap();
        assert_eq!((loaded.page, loaded.within, loaded.zoom), (7, 0.25, 1.5));
        assert!(!again.is_dirty(&state(7)));
        assert!(again.is_dirty(&state(8)));
        /* No temporary files are left behind */
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_moved_file_is_found_by_its_id() {
        let dir = dir("moved");
        let id = Some("ab12".to_owned());
        Store::new(dir.clone(), id.clone(), Path::new("old.pdf"))
            .save(&state(3))
            .unwrap();
        let mut moved = Store::new(dir.clone(), id, Path::new("new.pdf"));
        assert_eq!(moved.load().unwrap().page, 3);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_rebuilt_file_is_found_by_its_path() {
        let dir = dir("rebuilt");
        Store::new(
            dir.clone(),
            Some("first".to_owned()),
            Path::new("thesis.pdf"),
        )
        .save(&state(4))
        .unwrap();
        let mut rebuilt = Store::new(
            dir.clone(),
            Some("second".to_owned()),
            Path::new("thesis.pdf"),
        );
        assert_eq!(rebuilt.load().unwrap().page, 4);
        let mut other = Store::new(
            dir.clone(),
            Some("third".to_owned()),
            Path::new("other.pdf"),
        );
        assert_eq!(other.load(), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn broken_or_newer_files_are_ignored() {
        let dir = dir("broken");
        let mut store = Store::new(dir.clone(), None, Path::new("a.pdf"));
        store.save(&state(2)).unwrap();
        let file = store.files()[0].clone();

        fs::write(&file, "page = [").unwrap();
        assert_eq!(
            Store::new(dir.clone(), None, Path::new("a.pdf")).load(),
            None
        );

        let mut future = store.saved.clone().unwrap();
        future.version = VERSION + 1;
        fs::write(&file, toml::to_string(&future).unwrap()).unwrap();
        assert_eq!(
            Store::new(dir.clone(), None, Path::new("a.pdf")).load(),
            None
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(fnv1a(""), 0xcbf29ce484222325);
        assert_eq!(fnv1a("a"), 0xaf63dc4c8601ec8c);
    }

    #[test]
    fn the_id_is_read_from_the_trailer() {
        let mut bytes = crate::testutil::pdf(&[
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        ]);
        let document = Document::from_bytes(&bytes, "application/pdf").unwrap();
        assert_eq!(document_id(&document), None);

        /* The same file with an /ID in its trailer */
        let text = String::from_utf8(bytes)
            .unwrap()
            .replace("/Root 1 0 R", "/Root 1 0 R /ID [<00ff10> <abcd>]");
        bytes = text.into_bytes();
        let document = Document::from_bytes(&bytes, "application/pdf").unwrap();
        assert_eq!(document_id(&document), Some("00ff10".to_owned()));
    }
}
