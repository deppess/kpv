use base64::{engine::general_purpose::STANDARD, Engine};
use std::{
    fs::File,
    io::{stdout, Write},
    time::Duration,
};

use crate::globals::{RECEIVER_GR, SOFTWARE_ID};

/* Should be executed only after uncooking the terminal. This method expects the
 * terminal that a non-blocking and unbuffered read from stdin is possible */
pub fn terminal_graphics_test_support() -> Result<(), String> {
    let mut handle1 = stdout().lock();
    handle1
        .write_all(b"\x1B_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1B\\")
        .and_then(|_| handle1.flush())
        .map_err(io_error)?;
    drop(handle1);

    /* Timeout since we don't really know yet if the kitty graphics protocol
     * is supported or not */
    let response = RECEIVER_GR
        .get()
        .unwrap()
        .lock()
        .unwrap()
        .recv_timeout(Duration::from_millis(1000))
        .map_err(|_| {
            "kitty's graphics protocol is needed, and the terminal did not answer"
        })?;

    if !response.contains("OK") {
        Err(format!(
            "Terminal responded with failed graphics response: {}",
            response
        ))?;
    }
    Ok(())
}

/* Deletes an image and its placements from kitty, quietly */
pub fn terminal_graphics_delete_image(id: usize) -> Result<(), String> {
    let mut handle = stdout().lock();
    write!(handle, "\x1B_Gq=2,a=d,d=I,i={};\x1B\\", id)
        .and_then(|_| handle.flush())
        .map_err(io_error)
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

    File::create(tmp_file_path.as_path())
        .and_then(|mut f| f.write_all(data))
        .map_err(|x| format!("Could not write {}: {}", tmp_file_path.display(), x))?;

    /* First chunk with bitmap metadata */
    write!(
        handle,
        "\x1B_Gq=2,f={},i={},s={},v={},t=t;{}\x1B\\",
        if alpha { 32 } else { 24 },
        id,
        width,
        height,
        STANDARD.encode(tmp_file_path.as_os_str().as_encoded_bytes())
    )
    .and_then(|_| handle.flush())
    .map_err(io_error)
}

pub fn terminal_graphics_display_image(
    id: usize,
    col: usize,
    row: usize,

    rect: (usize, usize, usize, usize),

    c: usize,
    r: usize,
    under: bool,
) -> Result<(), String> {
    let mut handle = stdout().lock();
    let z = if under { UNDER_Z } else { PAGE_Z };
    write!(
        handle,
        "\x1B[s\x1B[{};{}H\x1B_Gz={},a=p,C=1,i={},x={},y={},w={},h={},c={},r={};\x1B\\\x1B[u",
        row, col, z, id, rect.0, rect.1, rect.2, rect.3, c, r
    )
    .and_then(|_| handle.flush())
    .map_err(io_error)
}

/* Images at a z-index below -1,073,741,824 are drawn behind cells with a colored
 * background, which lets an opaque panel cover them, and every negative z-index is
 * behind text. From the bottom up: pages, the ruler over them, the tint of a
 * see-through panel, and the tint of the selected entry in it. */
pub const PAGE_Z: i64 = -1_073_741_830;
/* A coarse whole page under the sharp part of a page, showing where that ends */
pub const UNDER_Z: i64 = -1_073_741_831;
const RULER_Z: i64 = -1_073_741_829;
pub const PANEL_Z: i64 = -1_073_741_828;
pub const SELECTED_Z: i64 = -1_073_741_827;

/* The ruler's image ID; page images count up from 1 and never get this far */
pub const RULER_ID: usize = 0x7FFF_0001;
/* Images of one pixel each, stretched under see-through panels */
pub const CONTENTS_TINT_ID: usize = 0x7FFF_0002;
pub const SELECTED_TINT_ID: usize = 0x7FFF_0003;
pub const HELP_TINT_ID: usize = 0x7FFF_0004;
pub const NOTES_TINT_ID: usize = 0x7FFF_0005;

/* Sends a one-pixel image, straight in the command since it's tiny. Quiet. */
pub fn terminal_graphics_send_pixel(id: usize, rgba: [u8; 4]) -> Result<(), String> {
    let mut handle = stdout().lock();
    write!(
        handle,
        "\x1B_Gq=2,a=t,f=32,s=1,v=1,t=d,i={};{}\x1B\\",
        id,
        STANDARD.encode(rgba)
    )
    .and_then(|_| handle.flush())
    .map_err(io_error)
}

/* Stretches image `id` over `cols` x `rows` cells from a 1-indexed cell at height
 * `z`, moving it if it's shown already. Quiet. */
pub fn terminal_graphics_place_tint(
    id: usize,
    z: i64,
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
) -> Result<(), String> {
    let mut handle = stdout().lock();
    write!(
        handle,
        "\x1B[s\x1B[{};{}H\x1B_Gq=2,z={},a=p,C=1,i={},p=1,c={},r={};\x1B\\\x1B[u",
        row, col, z, id, cols, rows
    )
    .and_then(|_| handle.flush())
    .map_err(io_error)
}

/* Takes image `id` off the screen and keeps it */
pub fn terminal_graphics_hide(id: usize) -> Result<(), String> {
    let mut handle = stdout().lock();
    write!(handle, "\x1B_Gq=2,a=d,d=i,i={};\x1B\\", id)
        .and_then(|_| handle.flush())
        .map_err(io_error)
}

/* Shows the ruler image over `cols` x `rows` cells from a 1-indexed cell, moving it
 * if it's shown already (it has the one placement, 1). Quiet, so no answer gets mixed
 * up with the answers for pages. */
pub fn terminal_graphics_place_ruler(
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
) -> Result<(), String> {
    let mut handle = stdout().lock();
    write!(
        handle,
        "\x1B[s\x1B[{};{}H\x1B_Gq=2,z={},a=p,C=1,i={},p=1,c={},r={};\x1B\\\x1B[u",
        row, col, RULER_Z, RULER_ID, cols, rows
    )
    .and_then(|_| handle.flush())
    .map_err(io_error)
}

fn io_error(x: std::io::Error) -> String {
    format!("Could not write to the terminal: {}", x)
}

/* A structure which extracts the Kitty graphics response in a lazy way */
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
