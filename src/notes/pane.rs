/* The editor in the notes panel: a program run in a pseudo-terminal, whose screen a
 * terminal emulator keeps so the panel can draw it */

use std::{
    io::{Read, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::Dimensions,
    index::{Column, Line},
    term::{cell::Flags, Config, Term, TermMode},
    vte::ansi::{Color as TermColor, CursorShape, NamedColor, Processor, Rgb},
};
use crossbeam_channel::{unbounded, Receiver};
use crossterm::{
    event::{KeyEvent, KeyModifiers, MouseEventKind},
    style::Color,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use super::keys::{encode_focus, encode_key, encode_mouse, encode_paste};

/* Collects what the emulator wants done, such as answering the program's questions */
#[derive(Clone)]
struct Proxy(Arc<Mutex<Vec<Event>>>);

impl EventListener for Proxy {
    fn send_event(&self, event: Event) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}

struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/* One cell of the editor's screen, ready to draw */
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /* Empty for the second half of a wide character, which isn't drawn */
    pub text: String,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /* Reverse video, which the outer terminal does with its own default colors */
    pub reverse: bool,
}

pub struct Pane {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    term: Term<Proxy>,
    parser: Processor,
    events: Arc<Mutex<Vec<Event>>>,
    /* What the program writes; closes when it exits */
    pub output: Receiver<Vec<u8>>,
    cols: u16,
    rows: u16,
    pixels: (u16, u16),
    /* The title the program gave its window */
    pub title: Option<String>,
}

impl Pane {
    /* Runs `program` with `args` in `cwd` on a screen of `cols` x `rows` cells that is
     * `pixels` wide and high */
    pub fn spawn(
        program: &str,
        args: &[String],
        cwd: &Path,
        cols: u16,
        rows: u16,
        pixels: (u16, u16),
        env: &[(String, String)],
    ) -> Result<Self, String> {
        let pty = native_pty_system()
            .openpty(pty_size(cols, rows, pixels))
            .map_err(|x| format!("Could not open a terminal for {}: {}", program, x))?;
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        /* The program runs in viewmer's panel, not in kitty: told it's kitty, yazi
         * would try to draw images the panel can't show */
        for (name, value) in env {
            command.env(name, value);
        }
        for name in [
            "KITTY_WINDOW_ID",
            "KITTY_PID",
            "KITTY_LISTEN_ON",
            "KITTY_PUBLIC_KEY",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "YAZI_LEVEL",
        ] {
            command.env_remove(name);
        }
        let child = pty
            .slave
            .spawn_command(command)
            .map_err(|x| format!("Could not start {}: {}", program, x))?;
        drop(pty.slave);

        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(|x| format!("Could not read from {}: {}", program, x))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|x| format!("Could not write to {}: {}", program, x))?;
        let (sender, output) = unbounded::<Vec<u8>>();
        thread::spawn(move || {
            let mut buffer = [0u8; 65536];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => {
                        if sender.send(buffer[..n].to_vec()).is_err() {
                            return;
                        }
                    }
                }
            }
        });

        let events = Arc::new(Mutex::new(Vec::new()));
        let config = Config {
            kitty_keyboard: true,
            scrolling_history: 0,
            ..Config::default()
        };
        let size = Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        };
        Ok(Self {
            master: pty.master,
            writer,
            child,
            term: Term::new(config, &size, Proxy(events.clone())),
            parser: Processor::new(),
            events,
            output,
            cols,
            rows,
            pixels,
            title: None,
        })
    }

    /* Takes in bytes the program wrote */
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
        self.answer();
    }

    /* Takes in everything the program has written so far; `true` if there was some */
    pub fn take_output(&mut self) -> bool {
        let mut any = false;
        while let Ok(bytes) = self.output.try_recv() {
            self.parser.advance(&mut self.term, &bytes);
            any = true;
        }
        self.answer();
        any
    }

    /* Answers what the program asked the terminal: its keyboard modes, its identity,
     * the size of the screen, its colors */
    fn answer(&mut self) {
        let events: Vec<Event> = match self.events.lock() {
            Ok(mut events) => events.drain(..).collect(),
            Err(_) => return,
        };
        for event in events {
            let reply = match event {
                Event::PtyWrite(text) => text,
                Event::Title(title) => {
                    self.title = Some(title);
                    continue;
                }
                Event::ResetTitle => {
                    self.title = None;
                    continue;
                }
                Event::TextAreaSizeRequest(format) => format(WindowSize {
                    num_lines: self.rows,
                    num_cols: self.cols,
                    cell_width: self.pixels.0 / self.cols.max(1),
                    cell_height: self.pixels.1 / self.rows.max(1),
                }),
                Event::ColorRequest(index, format) => {
                    /* Asked for its default text or background: a dark background */
                    let rgb = match index {
                        11 => Rgb { r: 0, g: 0, b: 0 },
                        _ => Rgb {
                            r: 229,
                            g: 229,
                            b: 229,
                        },
                    };
                    format(rgb)
                }
                _ => continue,
            };
            self.send(reply.as_bytes());
        }
    }

    pub fn send(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let _ = self
            .writer
            .write_all(bytes)
            .and_then(|_| self.writer.flush());
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn key(&mut self, key: &KeyEvent) {
        let bytes = encode_key(key, self.mode());
        self.send(&bytes);
    }

    pub fn paste(&mut self, text: &str) {
        let bytes = encode_paste(text, self.mode());
        self.send(&bytes);
    }

    pub fn focus(&mut self, gained: bool) {
        let bytes = encode_focus(gained, self.mode());
        self.send(&bytes);
    }

    /* A mouse event at a 0-indexed cell of the screen */
    pub fn mouse(
        &mut self,
        kind: MouseEventKind,
        mods: KeyModifiers,
        col: u16,
        row: u16,
    ) {
        let bytes = encode_mouse(kind, mods, col, row, self.mode());
        self.send(&bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16, pixels: (u16, u16)) {
        if (cols, rows, pixels) == (self.cols, self.rows, self.pixels) {
            return;
        }
        (self.cols, self.rows, self.pixels) = (cols, rows, pixels);
        let _ = self.master.resize(pty_size(cols, rows, pixels));
        self.term.resize(Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        });
    }

    /* Whether the program has ended */
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /* The screen, row by row. `fg` and `bg` stand in for the program's default
     * colors. */
    pub fn screen(&self, fg: Color, bg: Color) -> Vec<Vec<Cell>> {
        let grid = self.term.grid();
        (0..grid.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32)];
                (0..grid.columns())
                    .map(|col| {
                        let cell = &row[Column(col)];
                        let flags = cell.flags;
                        let (fore, back) =
                            (color(cell.fg, fg, bg), color(cell.bg, fg, bg));
                        let text = if flags.intersects(
                            Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
                        ) {
                            String::new()
                        } else if flags.contains(Flags::HIDDEN) {
                            " ".to_owned()
                        } else {
                            let mut text = cell.c.to_string();
                            if let Some(extra) = cell.zerowidth() {
                                text.extend(extra);
                            }
                            text
                        };
                        Cell {
                            text,
                            fg: fore,
                            bg: back,
                            bold: flags.contains(Flags::BOLD),
                            dim: flags.contains(Flags::DIM),
                            italic: flags.contains(Flags::ITALIC),
                            underline: flags.intersects(Flags::ALL_UNDERLINES),
                            strike: flags.contains(Flags::STRIKEOUT),
                            reverse: flags.contains(Flags::INVERSE),
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /* Where the program's cursor is, 0-indexed, even when it hides it: helix moves the
     * terminal's cursor to its own and draws it itself in normal mode */
    pub fn cursor_point(&self) -> (u16, u16) {
        let point = self.term.grid().cursor.point;
        (point.column.0 as u16, point.line.0.max(0) as u16)
    }

    /* The screen's text, row by row; a wide character takes one place */
    pub fn text(&self) -> Vec<String> {
        self.screen(Color::Reset, Color::Reset)
            .iter()
            .map(|row| row.iter().map(|c| c.text.as_str()).collect())
            .collect()
    }

    /* Where the cursor is, 0-indexed, and its shape, when the program shows it */
    pub fn cursor(&self) -> Option<(u16, u16, CursorShape)> {
        if !self.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let point = self.term.grid().cursor.point;
        let shape = self.term.cursor_style().shape;
        (shape != CursorShape::Hidden && point.line.0 >= 0).then_some((
            point.column.0 as u16,
            point.line.0 as u16,
            shape,
        ))
    }
}

fn pty_size(cols: u16, rows: u16, pixels: (u16, u16)) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: pixels.0,
        pixel_height: pixels.1,
    }
}

/* A color of the program's screen as a terminal color */
fn color(c: TermColor, fg: Color, bg: Color) -> Color {
    match c {
        TermColor::Spec(Rgb { r, g, b }) => Color::Rgb { r, g, b },
        TermColor::Indexed(i) => Color::AnsiValue(i),
        TermColor::Named(named) => match named {
            NamedColor::Background => bg,
            NamedColor::Foreground
            | NamedColor::BrightForeground
            | NamedColor::DimForeground => fg,
            NamedColor::Cursor => fg,
            n if (n as usize) < 16 => Color::AnsiValue(n as u8),
            n => match n {
                NamedColor::DimBlack => Color::AnsiValue(0),
                NamedColor::DimRed => Color::AnsiValue(1),
                NamedColor::DimGreen => Color::AnsiValue(2),
                NamedColor::DimYellow => Color::AnsiValue(3),
                NamedColor::DimBlue => Color::AnsiValue(4),
                NamedColor::DimMagenta => Color::AnsiValue(5),
                NamedColor::DimCyan => Color::AnsiValue(6),
                NamedColor::DimWhite => Color::AnsiValue(7),
                _ => fg,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /* Runs `sh -c script` and waits until `check` holds for the screen, or a second */
    fn run_until(script: &str, check: impl Fn(&Pane) -> bool) -> Pane {
        let mut pane = Pane::spawn(
            "sh",
            &["-c".to_owned(), script.to_owned()],
            Path::new("/"),
            20,
            4,
            (200, 80),
            &[],
        )
        .unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            pane.take_output();
            if check(&pane) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        pane
    }

    fn row_text(pane: &Pane, row: usize) -> String {
        pane.screen(Color::Reset, Color::Reset)[row]
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    #[test]
    fn a_programs_output_lands_on_its_screen() {
        let pane = run_until("printf 'one\\r\\n\\033[31mtwo'; sleep 2", |p| {
            row_text(p, 1) == "two"
        });
        assert_eq!(row_text(&pane, 0), "one");
        assert_eq!(row_text(&pane, 1), "two");
        let red = &pane.screen(Color::Reset, Color::Reset)[1][0];
        assert_eq!(red.fg, Color::AnsiValue(1));
    }

    #[test]
    fn the_program_learns_it_can_use_kittys_keyboard_protocol() {
        /* Ask for the keyboard flags and keep the answer, then turn on the first flag */
        let answer_file =
            std::env::temp_dir().join(format!("viewmer-pane-{}", std::process::id()));
        let script = format!(
            "stty raw -echo; printf '\\033[?u'; dd bs=1 count=5 2>/dev/null | od -c | head -1 > '{}'; printf '\\033[>1u'; sleep 2",
            answer_file.display()
        );
        let pane = run_until(&script, |p| {
            p.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES)
        });
        assert!(pane.mode().contains(TermMode::DISAMBIGUATE_ESC_CODES));
        let answer = std::fs::read_to_string(&answer_file).unwrap_or_default();
        let _ = std::fs::remove_file(&answer_file);
        assert!(answer.contains("?   0   u"), "answer: {:?}", answer);
    }

    #[test]
    fn the_pane_knows_when_its_program_ends() {
        let mut pane = run_until("exit 0", |_| false);
        let end = Instant::now() + Duration::from_secs(5);
        while !pane.exited() && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(pane.exited());
    }
}
