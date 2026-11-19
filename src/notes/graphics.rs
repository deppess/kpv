/* Kitty's image commands from programs in the notes terminal (yazi's previews), passed
 * on to kitty: placed where the program's cursor was, with image numbers of their own
 * so they never meet the PDF's pages, and quiet, since the terminal answers them. */

use std::collections::HashMap;

/* The image numbers the terminal's programs get in kitty */
pub const FIRST_ID: u32 = 0x4000_0000;
pub const LAST_ID: u32 = 0x4FFF_FFFF;

/* What a program wrote: text for the terminal, or an image command for kitty */
#[derive(Debug, PartialEq)]
pub enum Piece {
    Text(Vec<u8>),
    Command(String),
}

const START: &[u8] = b"\x1b_G";
const END: &[u8] = b"\x1b\\";

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/* Takes image commands out of what a program writes, which can come split anywhere */
#[derive(Default)]
pub struct Splitter {
    pending: Vec<u8>,
}

impl Splitter {
    pub fn split(&mut self, bytes: &[u8]) -> Vec<Piece> {
        self.pending.extend_from_slice(bytes);
        let mut pieces = Vec::new();
        loop {
            let Some(start) = find(&self.pending, START) else {
                /* Keep what may be the start of a command coming in the next read */
                let keep = (1..START.len())
                    .rev()
                    .find(|&k| self.pending.ends_with(&START[..k]))
                    .unwrap_or(0);
                let text: Vec<u8> =
                    self.pending.drain(..self.pending.len() - keep).collect();
                if !text.is_empty() {
                    pieces.push(Piece::Text(text));
                }
                return pieces;
            };
            if start > 0 {
                pieces.push(Piece::Text(self.pending.drain(..start).collect()));
            }
            let Some(len) = find(&self.pending[START.len()..], END) else {
                return pieces;
            };
            let body = &self.pending[START.len()..START.len() + len];
            pieces.push(Piece::Command(String::from_utf8_lossy(body).into_owned()));
            self.pending.drain(..START.len() + len + END.len());
        }
    }
}

/* Turns a program's image commands into commands for kitty */
pub struct Translator {
    /* The program's image ids and numbers, and the ids they have in kitty */
    ids: HashMap<u32, u32>,
    numbers: HashMap<u32, u32>,
    next: u32,
    /* An image coming in pieces, kept until its last piece so that it reaches kitty in
     * one go, and whether a cursor move around it needs undoing at the end */
    chunks: Option<(Vec<u8>, bool)>,
}

impl Default for Translator {
    fn default() -> Self {
        Self {
            ids: HashMap::new(),
            numbers: HashMap::new(),
            next: FIRST_ID,
            chunks: None,
        }
    }
}

impl Translator {
    fn map(
        map: &mut HashMap<u32, u32>,
        next: &mut u32,
        id: u32,
        make: bool,
    ) -> Option<u32> {
        if let Some(ours) = map.get(&id) {
            return Some(*ours);
        }
        if !make {
            return None;
        }
        let ours = *next;
        *next = if *next >= LAST_ID {
            FIRST_ID
        } else {
            *next + 1
        };
        map.insert(id, ours);
        Some(ours)
    }

    /* `command` (what came between `ESC _ G` and `ESC \`) with the program's cursor at
     * `at`, a 0-indexed cell of the window. Returns what to send kitty, and the answer
     * for the program. */
    pub fn command(&mut self, command: &str, at: (u16, u16)) -> (Vec<u8>, Vec<u8>) {
        let (control, payload) = command.split_once(';').unwrap_or((command, ""));
        let keys: Vec<(&str, &str)> = control
            .split(',')
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let get = |key: &str| keys.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        let more = get("m") == Some("1");

        /* The next piece of an image coming in pieces */
        if let Some((mut batch, moved)) = self.chunks.take() {
            batch.extend(format!("\x1b_G{};{}\x1b\\", control, payload).bytes());
            if more {
                self.chunks = Some((batch, moved));
                return (Vec::new(), Vec::new());
            }
            if moved {
                batch.extend(b"\x1b[u");
            }
            return (batch, Vec::new());
        }

        let action = get("a").unwrap_or("t");
        let id = get("i").and_then(|v| v.parse::<u32>().ok());
        if action == "q" {
            /* The program asks whether images work: they do */
            let reply = match id {
                Some(id) => format!("\x1b_Gi={};OK\x1b\\", id).into_bytes(),
                None => Vec::new(),
            };
            return (Vec::new(), reply);
        }

        let makes = matches!(action, "t" | "T" | "f");
        let mut out_keys: Vec<String> = Vec::new();
        for (key, value) in &keys {
            match *key {
                "q" => (),
                "i" => {
                    let Some(ours) = value
                        .parse::<u32>()
                        .ok()
                        .and_then(|v| Self::map(&mut self.ids, &mut self.next, v, makes))
                    else {
                        return (Vec::new(), Vec::new());
                    };
                    out_keys.push(format!("i={}", ours));
                }
                "I" => {
                    let Some(ours) = value.parse::<u32>().ok().and_then(|v| {
                        Self::map(&mut self.numbers, &mut self.next, v, makes)
                    }) else {
                        return (Vec::new(), Vec::new());
                    };
                    out_keys.push(format!("I={}", ours));
                }
                /* Deleting everything means everything of the program's own */
                "d" if matches!(*value, "a" | "A") => {
                    let range = if *value == "a" { "r" } else { "R" };
                    out_keys.push(format!("d={},x={},y={}", range, FIRST_ID, LAST_ID));
                }
                _ => out_keys.push(format!("{}={}", key, value)),
            }
        }
        out_keys.push("q=2".to_owned());
        let mut out = Vec::new();
        /* Placed at the program's cursor, wherever that is in the window */
        let places = matches!(action, "T" | "p");
        if places {
            out.extend(format!("\x1b[s\x1b[{};{}H", at.1 + 1, at.0 + 1).bytes());
        }
        out.extend(format!("\x1b_G{};{}\x1b\\", out_keys.join(","), payload).bytes());
        if more {
            self.chunks = Some((out, places));
            return (Vec::new(), Vec::new());
        }
        if places {
            out.extend(b"\x1b[u");
        }
        (out, Vec::new())
    }
}

/* Takes the terminal's images off the screen, and with `free` out of kitty's memory */
pub fn clear(free: bool) -> Vec<u8> {
    let range = if free { "R" } else { "r" };
    format!(
        "\x1b_Gq=2,a=d,d={},x={},y={}\x1b\\",
        range, FIRST_ID, LAST_ID
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Piece {
        Piece::Text(s.as_bytes().to_vec())
    }

    #[test]
    fn commands_come_out_of_the_text_even_split_across_reads() {
        let mut s = Splitter::default();
        assert_eq!(s.split(b"ab\x1b_Gi=1;AA"), vec![text("ab")]);
        assert_eq!(
            s.split(b"AA\x1b\\cd\x1b"),
            vec![Piece::Command("i=1;AAAA".to_owned()), text("cd")]
        );
        assert_eq!(s.split(b"[0m"), vec![text("\x1b[0m")]);
        /* Other escape sequences pass as text */
        assert_eq!(
            s.split(b"\x1b_Xother\x1b\\"),
            vec![text("\x1b_Xother\x1b\\")]
        );
    }

    fn sent(out: &[u8]) -> String {
        String::from_utf8_lossy(out).into_owned()
    }

    #[test]
    fn images_get_their_own_numbers_and_go_where_the_cursor_was() {
        let mut t = Translator::default();
        let (out, reply) = t.command("a=T,f=100,i=7,q=1;AAAA", (3, 5));
        assert!(reply.is_empty());
        assert_eq!(
            sent(&out),
            format!(
                "\x1b[s\x1b[6;4H\x1b_Ga=T,f=100,i={},q=2;AAAA\x1b\\\x1b[u",
                FIRST_ID
            )
        );
        /* The same image later is the same one in kitty */
        let (out, _) = t.command("a=d,d=i,i=7", (0, 0));
        assert_eq!(
            sent(&out),
            format!("\x1b_Ga=d,d=i,i={},q=2;\x1b\\", FIRST_ID)
        );
        /* An image kitty never got is not deleted in its place */
        assert!(t.command("a=d,d=i,i=99", (0, 0)).0.is_empty());
    }

    #[test]
    fn deleting_everything_deletes_only_the_terminals_images() {
        let mut t = Translator::default();
        let (out, _) = t.command("a=d,d=A", (0, 0));
        assert_eq!(
            sent(&out),
            format!("\x1b_Ga=d,d=R,x={},y={},q=2;\x1b\\", FIRST_ID, LAST_ID)
        );
    }

    #[test]
    fn an_image_in_pieces_reaches_kitty_in_one_go() {
        let mut t = Translator::default();
        assert!(t.command("a=T,i=1,m=1;AAAA", (0, 0)).0.is_empty());
        assert!(t.command("m=1;BBBB", (9, 9)).0.is_empty());
        let (out, _) = t.command("m=0;CC", (9, 9));
        let out = sent(&out);
        assert!(out.starts_with("\x1b[s\x1b[1;1H\x1b_Ga=T,i="));
        assert!(out.contains("\x1b_Gm=1;BBBB\x1b\\\x1b_Gm=0;CC\x1b\\"));
        assert!(out.ends_with("\x1b[u"));
    }

    #[test]
    fn a_question_about_images_is_answered() {
        let mut t = Translator::default();
        let (out, reply) = t.command("i=31,s=1,v=1,a=q,t=d,f=24;AAAA", (0, 0));
        assert!(out.is_empty());
        assert_eq!(sent(&reply), "\x1b_Gi=31;OK\x1b\\");
    }
}
