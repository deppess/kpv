use std::{
    fs,
    os::unix::fs::MetadataExt,
    thread,
    time::{Duration, SystemTime},
};

/* How often the file is looked at. A change is reloaded once the file has stayed the
 * same for one more look, so that a file written in several steps is only loaded once
 * it's complete. */
const EVERY: Duration = Duration::from_millis(200);

/* What tells one version of the file from the next: saving by writing a new file and
 * renaming it over the old one, as editors and PDF tools often do, changes the inode */
fn version(path: &str) -> Option<(u64, u64, SystemTime)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.ino(), meta.len(), meta.modified().ok()?))
}

/* Calls `changed` each time the file has changed and settled, until it returns
 * `false` */
pub fn spawn(
    file: &str,
    changed: impl Fn() -> bool + Send + 'static,
) -> Result<(), String> {
    let path = fs::canonicalize(file)
        .map_err(|x| format!("Could not resolve the file path: {}", x))?
        .to_string_lossy()
        .into_owned();
    thread::spawn(move || {
        let mut loaded = version(&path);
        let mut last = loaded;
        loop {
            thread::sleep(EVERY);
            let now = version(&path);
            /* Missing for a moment while it's replaced, or still being written */
            if now.is_none() || now != last {
                last = now;
                continue;
            }
            if now != loaded {
                loaded = now;
                if !changed() {
                    return;
                }
            }
        }
    });
    Ok(())
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
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("doc.pdf");
        fs::write(&file, b"one").unwrap();

        let (sender, reloads) = std::sync::mpsc::channel();
        spawn(file.to_str().unwrap(), move || sender.send(()).is_ok()).unwrap();
        thread::sleep(EVERY * 2);

        let temp = folder.join("doc.pdf.tmp");
        fs::write(&temp, b"two").unwrap();
        fs::rename(&temp, &file).unwrap();

        let got = reloads.recv_timeout(Duration::from_secs(5));
        thread::sleep(EVERY * 3);
        let extra = reloads.try_iter().count();
        fs::remove_dir_all(&folder).unwrap();

        assert!(got.is_ok(), "no reload after a rename-over save");
        assert_eq!(extra, 0, "one save should reload once");
    }
}
