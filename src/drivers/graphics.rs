use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    collections::HashMap,
    fs::File,
    io::{stdout, Write},
    time::Duration,
};

use crate::{RECEIVER_GR, SOFTWARE_ID};

/* Should be executed only after uncooking the terminal. This method expects the
 * terminal that a non-blocking and unbuffered read from stdin is possible */
pub fn terminal_graphics_test_support() -> Result<(), String> {
    let mut handle1 = stdout().lock();
    handle1
        .write_all(b"\x1B_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1B\\")
        .unwrap();
    handle1.flush().unwrap();

    /* Timeout since we don't really know yet if the kitty graphics protocol
     * is supported or not */
    let response = RECEIVER_GR
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .recv_timeout(Duration::from_millis(1000))
        .map_err(|x| {
            format!("Could not receive from Graphics Response channel: {}", x)
        })?;

    if !response.payload().contains("OK") {
        Err(format!(
            "Terminal responded with failed graphics response: {}",
            response.payload()
        ))?;
    }
    Ok(())
}

#[allow(dead_code)]
pub fn terminal_graphics_deallocate_id(id: usize) -> Result<(), String> {
    let mut handle = stdout().lock();
    write!(handle, "\x1B_Ga=d,d=I,i={};\x1B\\", id).unwrap();

    handle.flush().unwrap();

    Ok(())
}

pub fn terminal_graphics_transfer_bitmap(
    id: usize,
    width: usize,
    height: usize,
    data: &[u8],
    alpha: bool,
) -> Result<(), String> {
    let mut handle = stdout().lock();
    let mut tmp_file_path = std::env::temp_dir();

    tmp_file_path.push(format!(
        "tty-graphics-protocol-{}-{}",
        SOFTWARE_ID.get().unwrap(),
        id
    ));

    /* Wait for the file to get automatically get deleted by Kitty from a previous
     * render instance of this exact image with the same ID. If this is not done
     * this will lead to extreme bugs where the Kitty terminal can crash. Kitty
     * may also never read it (e.g. the command was lost), so don't wait forever. */
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while tmp_file_path.as_path().exists() {
        if std::time::Instant::now() >= deadline {
            let _ = std::fs::remove_file(tmp_file_path.as_path());
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    {
        let mut tmp_file = File::create(tmp_file_path.as_path()).unwrap();
        tmp_file.write_all(data).unwrap();
    }

    /* First chunk with bitmap metadata */
    write!(
        handle,
        "\x1B_Gq=2,f={},i={},s={},v={},t=t;{}\x1B\\",
        if alpha { 32 } else { 24 },
        id,
        width,
        height,
        STANDARD.encode(tmp_file_path.to_str().unwrap())
    )
    .unwrap();

    handle.flush().unwrap();

    Ok(())
}

pub fn terminal_graphics_display_image(
    id: usize,
    col: usize,
    row: usize,

    rect: (usize, usize, usize, usize),

    c: usize,
    r: usize,
) -> Result<(), String> {
    let mut handle = stdout().lock();

    write!(handle, "\x1B[s\x1B[{};{}H", row, col).unwrap();

    /* Z-index < -1,073,741,824 will make the images to be drawn behind
     * cells with colored background */
    write!(
        handle,
        "\x1B_Gz=-1073741825,a=p,C=1,i={},x={},y={},w={},h={},c={},r={};\x1B\\",
        id, rect.0, rect.1, rect.2, rect.3, c, r
    )
    .unwrap();

    handle.write_all(b"\x1B[u").unwrap();

    handle
        .flush()
        .map_err(|x: std::io::Error| format!("Could not flush stdout: {}", x))?;

    Ok(())
}

/* A structure which extracts the Kitty graphics response in a lazy way */
#[derive(Debug, Clone)]
pub struct GraphicsResponse {
    source: String,
    loaded: bool,
    control: HashMap<String, String>,
    payload: String,
}

impl GraphicsResponse {
    pub fn new(source: &[u8]) -> Self {
        let source = std::str::from_utf8(source).unwrap();
        let spl: Vec<&str> = source.split(';').collect();

        Self {
            source: spl.first().unwrap_or(&"").to_string(),
            loaded: false,
            control: HashMap::new(),
            payload: spl.get(1).unwrap_or(&"").to_string(),
        }
    }

    #[allow(dead_code)]
    fn load(&mut self) {
        let spl1 = self.source.split(',');
        for kv in spl1 {
            let spl2: Vec<&str> = kv.split('=').collect();
            if spl2.len() != 2 {
                continue;
            }

            let _ = self
                .control
                .insert(spl2[0].to_string(), spl2[1].to_string());
        }

        self.loaded = true;
    }

    #[allow(dead_code)]
    pub fn control(&mut self) -> &HashMap<String, String> {
        if !self.loaded {
            self.load();
        }
        &self.control
    }

    pub fn payload(&self) -> &str {
        self.payload.as_str()
    }
}

/* Removes temporary image files that kitty never read: this instance's, and any
 * older than a minute from earlier instances (kitty reads them within
 * milliseconds, so those are stale) */
pub fn terminal_graphics_remove_leftovers() {
    const STALE: Duration = Duration::from_secs(60);

    let own = format!("tty-graphics-protocol-{}-", SOFTWARE_ID.get().unwrap());
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_image_temp_file(&name) {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > STALE);
        if name.starts_with(&own) || stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/* `tty-graphics-protocol-<hex instance id>-<image id>`, as written above */
fn is_image_temp_file(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("tty-graphics-protocol-") else {
        return false;
    };
    let Some((instance, image)) = rest.split_once('-') else {
        return false;
    };
    !instance.is_empty()
        && instance.chars().all(|c| c.is_ascii_hexdigit())
        && !image.is_empty()
        && image.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_temp_file_names_match() {
        assert!(is_image_temp_file(
            "tty-graphics-protocol-C76AC3E0051128DB-3"
        ));
        assert!(!is_image_temp_file(
            "tty-graphics-protocol-C76AC3E0051128DB-3x"
        ));
        assert!(!is_image_temp_file("tty-graphics-protocol-other.png"));
        assert!(!is_image_temp_file("tty-graphics-protocol-ZZ-1"));
        assert!(!is_image_temp_file("notes.txt"));
    }
}
