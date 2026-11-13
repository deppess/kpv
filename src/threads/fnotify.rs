use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
    time::Duration,
};

use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

static WATCHER_FILE: OnceLock<RecommendedWatcher> = OnceLock::new();

/* Quiet time after the last change before reloading, so that a file written in
 * several steps is only loaded once it's complete */
const SETTLE: Duration = Duration::from_millis(200);

/* Watches the file's folder rather than the file itself: editors and PDF tools often
 * save by writing a new file and renaming it over the old one, which a watch on the
 * old file would miss. */
pub fn spawn(file: &str) -> Result<Receiver<()>, String> {
    let path = std::fs::canonicalize(file)
        .map_err(|x| format!("Could not resolve the file path: {}", x))?;
    let folder = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let name = path.file_name().map(|n| n.to_owned());

    let (sender_change, receiver_change) = unbounded::<()>();
    let (sender_file_reload, receiver_file_reload) = unbounded::<()>();

    let mut watcher_file =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            let relevant =
                matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_));
            let ours = event.paths.iter().any(|p| p.file_name() == name.as_deref());
            if relevant && ours {
                let _ = sender_change.send(());
            }
        })
        .map_err(|x| format!("Could not initialize a file watcher: {}", x))?;

    watcher_file
        .watch(&folder, RecursiveMode::NonRecursive)
        .map_err(|x| format!("Could not watch {}: {}", folder.display(), x))?;

    WATCHER_FILE.get_or_init(|| watcher_file);

    thread::spawn(move || {
        while receiver_change.recv().is_ok() {
            loop {
                match receiver_change.recv_timeout(SETTLE) {
                    Ok(()) => continue,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            if sender_file_reload.send(()).is_err() {
                return;
            }
        }
    });

    Ok(receiver_file_reload)
}

#[cfg(test)]
mod tests {
    use super::*;

    /* Saving by writing a temporary file and renaming it over the watched one, as
     * Helix and many PDF tools do, must still trigger exactly one reload */
    #[test]
    fn replace_by_rename_triggers_one_reload() {
        let folder =
            std::env::temp_dir().join(format!("viewmer-watch-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join("doc.pdf");
        std::fs::write(&file, b"one").unwrap();

        let reloads = spawn(file.to_str().unwrap()).unwrap();
        /* Let the watcher settle so the initial write isn't reported */
        thread::sleep(Duration::from_millis(300));
        while reloads.try_recv().is_ok() {}

        let temp = folder.join("doc.pdf.tmp");
        std::fs::write(&temp, b"two").unwrap();
        std::fs::rename(&temp, &file).unwrap();

        let got = reloads.recv_timeout(Duration::from_secs(5));
        thread::sleep(SETTLE * 2);
        let extra = reloads.try_iter().count();
        std::fs::remove_dir_all(&folder).unwrap();

        assert!(got.is_ok(), "no reload after a rename-over save");
        assert_eq!(extra, 0, "one save should reload once");
    }
}
