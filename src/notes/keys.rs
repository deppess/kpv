/* What the editor in the notes panel is sent for a key, a mouse event or a paste: the
 * bytes a terminal would send it, in the form it asked for. With kitty's keyboard
 * protocol turned on (as helix does) keys like Shift-Enter and Ctrl-h arrive as
 * themselves; without it they arrive the old way. */

use alacritty_terminal::term::TermMode;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

/* Kitty's modifier number: 1 plus the bits of the held modifiers */
fn modifier_number(mods: KeyModifiers) -> u8 {
    let mut n = 0;
    if mods.contains(KeyModifiers::SHIFT) {
        n |= 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        n |= 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        n |= 4;
    }
    if mods.contains(KeyModifiers::SUPER) {
        n |= 8;
    }
    1 + n
}

/* `CSI <code>;<mods> <end>`, leaving out what isn't needed */
fn csi(code: u32, mods: u8, end: char) -> Vec<u8> {
    let text = match (code, mods) {
        (1, 1) if end != '~' && end != 'u' => format!("\x1b[{}", end),
        (_, 1) => format!("\x1b[{}{}", code, end),
        _ => format!("\x1b[{};{}{}", code, mods, end),
    };
    text.into_bytes()
}

/* Keys that aren't text: arrows, Home, F-keys and the like, as `(code, final)` */
fn functional(code: KeyCode) -> Option<(u32, char)> {
    Some(match code {
        KeyCode::Up => (1, 'A'),
        KeyCode::Down => (1, 'B'),
        KeyCode::Right => (1, 'C'),
        KeyCode::Left => (1, 'D'),
        KeyCode::Home => (1, 'H'),
        KeyCode::End => (1, 'F'),
        KeyCode::Insert => (2, '~'),
        KeyCode::Delete => (3, '~'),
        KeyCode::PageUp => (5, '~'),
        KeyCode::PageDown => (6, '~'),
        KeyCode::F(1) => (1, 'P'),
        KeyCode::F(2) => (1, 'Q'),
        KeyCode::F(3) => (13, '~'),
        KeyCode::F(4) => (1, 'S'),
        KeyCode::F(5) => (15, '~'),
        KeyCode::F(6) => (17, '~'),
        KeyCode::F(7) => (18, '~'),
        KeyCode::F(8) => (19, '~'),
        KeyCode::F(9) => (20, '~'),
        KeyCode::F(10) => (21, '~'),
        KeyCode::F(11) => (23, '~'),
        KeyCode::F(12) => (24, '~'),
        _ => return None,
    })
}

pub fn encode_key(key: &KeyEvent, mode: TermMode) -> Vec<u8> {
    let mods = key.modifiers;
    let m = modifier_number(mods);

    if let Some((code, end)) = functional(key.code) {
        /* Arrows, Home and End in the application cursor mode, without modifiers */
        if m == 1 && code == 1 && matches!(end, 'A'..='D' | 'H' | 'F') {
            if mode.contains(TermMode::APP_CURSOR) {
                return format!("\x1bO{}", end).into_bytes();
            }
            return format!("\x1b[{}", end).into_bytes();
        }
        /* F1, F2 and F4 without modifiers */
        if m == 1 && matches!(end, 'P' | 'Q' | 'S') {
            return format!("\x1bO{}", end).into_bytes();
        }
        return csi(code, m, end);
    }

    if mode
        .intersects(TermMode::DISAMBIGUATE_ESC_CODES | TermMode::REPORT_ALL_KEYS_AS_ESC)
    {
        return kitty(key, m, mode);
    }
    legacy(key)
}

fn kitty(key: &KeyEvent, m: u8, mode: TermMode) -> Vec<u8> {
    let all = mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC);
    let shift_only = key.modifiers.difference(KeyModifiers::SHIFT).is_empty();
    match key.code {
        KeyCode::Char(c) => {
            if shift_only && !all {
                return c.to_string().into_bytes();
            }
            /* The key's own code is the lowercase letter, whatever Shift did to it; the
             * letter Shift made goes along when the program asks, and so does the text
             * the key types */
            let base = c.to_lowercase().next().unwrap_or(c);
            let shifted = c != base;
            let m = if shifted {
                modifier_number(key.modifiers | KeyModifiers::SHIFT)
            } else {
                m
            };
            let code = if shifted && mode.contains(TermMode::REPORT_ALTERNATE_KEYS) {
                format!("{}:{}", base as u32, c as u32)
            } else {
                (base as u32).to_string()
            };
            let text = mode.contains(TermMode::REPORT_ASSOCIATED_TEXT) && shift_only;
            match (text, m) {
                (true, _) => format!("\x1b[{};{};{}u", code, m, c as u32),
                (false, 1) => format!("\x1b[{}u", code),
                (false, _) => format!("\x1b[{};{}u", code, m),
            }
            .into_bytes()
        }
        KeyCode::Enter if m == 1 && !all => b"\r".to_vec(),
        KeyCode::Tab if m == 1 && !all => b"\t".to_vec(),
        KeyCode::Backspace if m == 1 && !all => b"\x7f".to_vec(),
        KeyCode::Enter => csi(13, m, 'u'),
        KeyCode::Tab => csi(9, m, 'u'),
        KeyCode::BackTab => {
            csi(9, modifier_number(key.modifiers | KeyModifiers::SHIFT), 'u')
        }
        KeyCode::Backspace => csi(127, m, 'u'),
        KeyCode::Esc => csi(27, m, 'u'),
        _ => Vec::new(),
    }
}

fn legacy(key: &KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let mut out = match key.code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            ' ' | '@' | '2' => vec![0],
            c @ ('a'..='z') => vec![c as u8 - b'a' + 1],
            '[' | '3' => vec![0x1b],
            '\\' | '4' => vec![0x1c],
            ']' | '5' => vec![0x1d],
            '^' | '6' => vec![0x1e],
            '_' | '7' | '/' => vec![0x1f],
            '8' | '?' => vec![0x7f],
            c => c.to_string().into_bytes(),
        },
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::BackTab => return b"\x1b[Z".to_vec(),
        KeyCode::Backspace if ctrl => vec![0x08],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        _ => return Vec::new(),
    };
    if alt {
        out.insert(0, 0x1b);
    }
    out
}

/* A bracketed paste when the editor asked for one, so it doesn't run pasted text as
 * keys */
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    if mode.contains(TermMode::BRACKETED_PASTE) {
        /* A paste must not be able to end the bracket early */
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{}\x1b[201~", text).into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/* Focus in or out, when the editor asked to hear about it */
pub fn encode_focus(gained: bool, mode: TermMode) -> Vec<u8> {
    match (mode.contains(TermMode::FOCUS_IN_OUT), gained) {
        (false, _) => Vec::new(),
        (true, true) => b"\x1b[I".to_vec(),
        (true, false) => b"\x1b[O".to_vec(),
    }
}

/* A mouse event at a 0-indexed cell of the editor, when it asked for mouse events of
 * that kind */
pub fn encode_mouse(
    kind: MouseEventKind,
    mods: KeyModifiers,
    col: u16,
    row: u16,
    mode: TermMode,
) -> Vec<u8> {
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0u8,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (code, release) = match kind {
        MouseEventKind::Down(b) if mode.intersects(TermMode::MOUSE_MODE) => {
            (button(b), false)
        }
        MouseEventKind::Up(b) if mode.intersects(TermMode::MOUSE_MODE) => {
            (button(b), true)
        }
        MouseEventKind::Drag(b)
            if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) =>
        {
            (button(b) + 32, false)
        }
        MouseEventKind::Moved if mode.contains(TermMode::MOUSE_MOTION) => (35, false),
        MouseEventKind::ScrollUp if mode.intersects(TermMode::MOUSE_MODE) => (64, false),
        MouseEventKind::ScrollDown if mode.intersects(TermMode::MOUSE_MODE) => {
            (65, false)
        }
        MouseEventKind::ScrollUp if mode.contains(TermMode::ALT_SCREEN) => {
            return b"\x1b[A\x1b[A\x1b[A".to_vec()
        }
        MouseEventKind::ScrollDown if mode.contains(TermMode::ALT_SCREEN) => {
            return b"\x1b[B\x1b[B\x1b[B".to_vec()
        }
        _ => return Vec::new(),
    };
    let mut code = code;
    if mods.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if mods.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    let (x, y) = (col as u32 + 1, row as u32 + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return format!("\x1b[<{};{};{}{}", code, x, y, end).into_bytes();
    }
    /* The old form tops out at column and row 223 */
    let code = if release { 3 } else { code };
    let cell = |v: u32| (32 + v.min(223)) as u8;
    vec![0x1b, b'[', b'M', 32 + code, cell(x), cell(y)]
}

#[cfg(test)]
mod tests {
    use super::*;

    const KITTY: TermMode = TermMode::DISAMBIGUATE_ESC_CODES;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn kitty_key(code: KeyCode, mods: KeyModifiers) -> String {
        String::from_utf8(encode_key(&key(code, mods), KITTY)).unwrap()
    }

    fn legacy_key(code: KeyCode, mods: KeyModifiers) -> Vec<u8> {
        encode_key(&key(code, mods), TermMode::empty())
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;

    #[test]
    fn text_stays_text() {
        assert_eq!(kitty_key(KeyCode::Char('a'), NONE), "a");
        assert_eq!(kitty_key(KeyCode::Char('A'), SHIFT), "A");
        assert_eq!(kitty_key(KeyCode::Char('é'), NONE), "é");
        assert_eq!(legacy_key(KeyCode::Char('a'), NONE), b"a");
    }

    #[test]
    fn keys_the_old_way_cant_tell_apart_have_their_own_codes() {
        assert_eq!(kitty_key(KeyCode::Enter, SHIFT), "\x1b[13;2u");
        assert_eq!(kitty_key(KeyCode::Char('h'), CTRL), "\x1b[104;5u");
        assert_eq!(kitty_key(KeyCode::Char('i'), CTRL), "\x1b[105;5u");
        assert_eq!(kitty_key(KeyCode::Esc, NONE), "\x1b[27u");
        assert_eq!(kitty_key(KeyCode::BackTab, SHIFT), "\x1b[9;2u");
        assert_eq!(kitty_key(KeyCode::Char(' '), CTRL), "\x1b[32;5u");
        assert_eq!(kitty_key(KeyCode::Char('A'), CTRL | SHIFT), "\x1b[97;6u");
        assert_eq!(kitty_key(KeyCode::Char('x'), ALT), "\x1b[120;3u");
        /* Plain Enter, Tab and Backspace stay as they were */
        assert_eq!(kitty_key(KeyCode::Enter, NONE), "\r");
        assert_eq!(kitty_key(KeyCode::Tab, NONE), "\t");
        assert_eq!(kitty_key(KeyCode::Backspace, NONE), "\x7f");
    }

    #[test]
    fn every_key_as_a_code_keeps_what_shift_made() {
        let all = KITTY | TermMode::REPORT_ALL_KEYS_AS_ESC;
        let encode = |code, mods, mode| {
            String::from_utf8(encode_key(&key(code, mods), mode)).unwrap()
        };
        assert_eq!(encode(KeyCode::Char('f'), NONE, all), "\x1b[102u");
        assert_eq!(encode(KeyCode::Char('F'), SHIFT, all), "\x1b[102;2u");
        let alternate = all | TermMode::REPORT_ALTERNATE_KEYS;
        assert_eq!(
            encode(KeyCode::Char('F'), SHIFT, alternate),
            "\x1b[102:70;2u"
        );
        let text = alternate | TermMode::REPORT_ASSOCIATED_TEXT;
        assert_eq!(encode(KeyCode::Char('F'), SHIFT, text), "\x1b[102:70;2;70u");
        assert_eq!(encode(KeyCode::Char('a'), NONE, text), "\x1b[97;1;97u");
        /* A chord types no text */
        assert_eq!(encode(KeyCode::Char('a'), CTRL, text), "\x1b[97;5u");
        assert_eq!(encode(KeyCode::Enter, NONE, all), "\x1b[13u");
    }

    #[test]
    fn the_old_way() {
        assert_eq!(legacy_key(KeyCode::Char('h'), CTRL), vec![8]);
        assert_eq!(legacy_key(KeyCode::Char(' '), CTRL), vec![0]);
        assert_eq!(legacy_key(KeyCode::Char('\\'), CTRL), vec![0x1c]);
        assert_eq!(legacy_key(KeyCode::Char('x'), ALT), b"\x1bx");
        assert_eq!(legacy_key(KeyCode::Enter, SHIFT), b"\r");
        assert_eq!(legacy_key(KeyCode::Esc, NONE), b"\x1b");
        assert_eq!(legacy_key(KeyCode::BackTab, SHIFT), b"\x1b[Z");
    }

    #[test]
    fn arrows_page_keys_and_f_keys() {
        assert_eq!(kitty_key(KeyCode::Up, NONE), "\x1b[A");
        assert_eq!(kitty_key(KeyCode::Up, CTRL), "\x1b[1;5A");
        assert_eq!(
            encode_key(&key(KeyCode::Left, NONE), TermMode::APP_CURSOR),
            b"\x1bOD"
        );
        assert_eq!(kitty_key(KeyCode::PageDown, NONE), "\x1b[6~");
        assert_eq!(kitty_key(KeyCode::Delete, SHIFT), "\x1b[3;2~");
        assert_eq!(kitty_key(KeyCode::F(1), NONE), "\x1bOP");
        assert_eq!(kitty_key(KeyCode::F(1), SHIFT), "\x1b[1;2P");
        assert_eq!(kitty_key(KeyCode::F(5), NONE), "\x1b[15~");
    }

    #[test]
    fn pastes_are_bracketed_when_asked() {
        assert_eq!(
            encode_paste("a\nb", TermMode::BRACKETED_PASTE),
            b"\x1b[200~a\nb\x1b[201~"
        );
        assert_eq!(
            encode_paste("x\x1b[201~y", TermMode::BRACKETED_PASTE),
            b"\x1b[200~xy\x1b[201~"
        );
        assert_eq!(encode_paste("a\nb", TermMode::empty()), b"a\rb");
    }

    #[test]
    fn focus_only_when_asked() {
        assert_eq!(encode_focus(false, TermMode::FOCUS_IN_OUT), b"\x1b[O");
        assert_eq!(encode_focus(true, TermMode::FOCUS_IN_OUT), b"\x1b[I");
        assert!(encode_focus(false, TermMode::empty()).is_empty());
    }

    #[test]
    fn mouse_clicks_drags_and_wheel() {
        let sgr =
            TermMode::MOUSE_REPORT_CLICK | TermMode::MOUSE_DRAG | TermMode::SGR_MOUSE;
        let left = MouseButton::Left;
        assert_eq!(
            encode_mouse(MouseEventKind::Down(left), NONE, 4, 2, sgr),
            b"\x1b[<0;5;3M"
        );
        assert_eq!(
            encode_mouse(MouseEventKind::Up(left), NONE, 4, 2, sgr),
            b"\x1b[<0;5;3m"
        );
        assert_eq!(
            encode_mouse(MouseEventKind::Drag(left), CTRL, 0, 0, sgr),
            b"\x1b[<48;1;1M"
        );
        assert_eq!(
            encode_mouse(MouseEventKind::ScrollDown, NONE, 0, 0, sgr),
            b"\x1b[<65;1;1M"
        );
        assert!(encode_mouse(MouseEventKind::Moved, NONE, 0, 0, sgr).is_empty());
        assert!(
            encode_mouse(MouseEventKind::Down(left), NONE, 0, 0, TermMode::empty())
                .is_empty()
        );
        assert_eq!(
            encode_mouse(
                MouseEventKind::Down(left),
                NONE,
                0,
                0,
                TermMode::MOUSE_REPORT_CLICK
            ),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }
}
