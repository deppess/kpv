/* The notes panel's program, run in a pseudo-terminal, whose screen a terminal
 * emulator keeps so the panel can draw it */

use std::{
    fs::File,
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        mpsc::{channel, Receiver, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use alacritty_terminal::{
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line},
    term::ClipboardType,
    term::{cell::Flags, Config, Term, TermMode},
    vte::ansi::{Color as TermColor, CursorShape, NamedColor, Processor, Rgb},
};
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers, MouseEventKind},
    style::{Attribute, Attributes, Color},
};

use super::graphics::{Piece, Splitter, Translator};
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

/* One cell of the program's screen, ready to draw */
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /* `\0` for the second half of a wide character, which isn't drawn */
    pub c: char,
    /* Combining marks drawn with it; empty, and so not allocated, nearly always */
    pub marks: Vec<char>,
    pub style: Style,
}

/* Colors and attributes; reverse video goes to the outer terminal, which does it with
 * its own default colors */
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub attributes: Attributes,
}

/* Lines kept of what scrolled off the top, and how many the wheel moves */
const HISTORY: usize = 5000;
const WHEEL_LINES: i32 = 3;

pub struct Pane {
    /* The terminal's side of the pseudo-terminal */
    master: File,
    child: Child,
    term: Term<Proxy>,
    parser: Processor,
    events: Arc<Mutex<Vec<Event>>>,
    /* What the program writes; closes when it exits */
    output: Receiver<Vec<u8>>,
    cols: u16,
    rows: u16,
    pixels: (u16, u16),
    /* The title the program gave its window */
    pub title: Option<String>,
    /* Image commands taken out of the output, and what they and copies to the
     * clipboard become for kitty */
    splitter: Splitter,
    translator: Translator,
    graphics: Vec<u8>,
    /* Where the screen's top-left cell is in the window */
    pub origin: (u16, u16),
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
        let (master, slave) = open_pty(cols, rows, pixels)
            .map_err(|x| format!("Could not open a terminal for {}: {}", program, x))?;
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .envs(env.iter().map(|(k, v)| (k, v)));
        /* The program runs in a terminal of its own, not in kitty's window, which
         * kitty's variables would point it at */
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
        let stdio = |fd: &OwnedFd| fd.try_clone().map(Stdio::from);
        let failed = |x: io::Error| format!("Could not start {}: {}", program, x);
        command
            .stdin(stdio(&slave).map_err(failed)?)
            .stdout(stdio(&slave).map_err(failed)?)
            .stderr(Stdio::from(slave));
        /* Its own session, with the pseudo-terminal as its terminal, so that job
         * control and ^C work in it */
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(failed)?;
        let master = File::from(master);
        let mut reader = master.try_clone().map_err(failed)?;
        /* The main loop hears of every read, and of the end once the program and
         * all it started are gone */
        let (sender, output) = channel::<Vec<u8>>();
        thread::spawn(move || {
            let mut buffer = [0u8; 65536];
            while let Ok(n @ 1..) = reader.read(&mut buffer) {
                if sender.send(buffer[..n].to_vec()).is_err() {
                    return;
                }
                crate::threads::wake(crate::threads::Wake::Terminal);
            }
            drop(sender);
            crate::threads::wake(crate::threads::Wake::Terminal);
        });

        let events = Arc::new(Mutex::new(Vec::new()));
        let config = Config {
            kitty_keyboard: true,
            scrolling_history: HISTORY,
            ..Config::default()
        };
        let size = Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        };
        Ok(Self {
            master,
            child,
            term: Term::new(config, &size, Proxy(events.clone())),
            parser: Processor::new(),
            events,
            output,
            cols,
            rows,
            pixels,
            title: None,
            splitter: Splitter::default(),
            translator: Translator::default(),
            graphics: Vec::new(),
            origin: (0, 0),
        })
    }

    /* Takes in everything the program has written so far; `false` once it has ended
     * and there's nothing more */
    pub fn take_output(&mut self) -> bool {
        let open = loop {
            match self.output.try_recv() {
                Ok(bytes) => self.process(&bytes),
                Err(TryRecvError::Empty) => break true,
                Err(TryRecvError::Disconnected) => break false,
            }
        };
        self.answer();
        open
    }

    /* Text goes to the screen; image commands become commands for kitty, placed where
     * the cursor is when they come */
    fn process(&mut self, bytes: &[u8]) {
        for piece in self.splitter.split(bytes) {
            match piece {
                Piece::Text(text) => self.parser.advance(&mut self.term, &text),
                Piece::Command(command) => {
                    let (col, row) = self.cursor_point();
                    let at = (self.origin.0 + col, self.origin.1 + row);
                    let (out, reply) = self.translator.command(&command, at);
                    self.graphics.extend(out);
                    self.send(&reply);
                }
            }
        }
    }

    /* The image commands for kitty since last asked */
    pub fn take_graphics(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.graphics)
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
                /* A copy (OSC 52, as helix over ssh, tmux and nvim do it) goes on to
                 * kitty, which puts it on the clipboard */
                Event::ClipboardStore(kind, text) => {
                    use base64::{engine::general_purpose::STANDARD, Engine};
                    let target = match kind {
                        ClipboardType::Selection => 'p',
                        ClipboardType::Clipboard => 'c',
                    };
                    let copy =
                        format!("\x1b]52;{};{}\x1b\\", target, STANDARD.encode(text));
                    self.graphics.extend(copy.bytes());
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
        let _ = self.master.write_all(bytes);
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn key(&mut self, key: &KeyEvent) {
        /* Shift-PageUp and -PageDown page through what scrolled off, as in most
         * terminals, unless the program has the whole screen to itself */
        let paging = key.modifiers == KeyModifiers::SHIFT
            && !self.mode().contains(TermMode::ALT_SCREEN);
        match key.code {
            KeyCode::PageUp if paging => return self.scroll(self.rows as i32 - 1),
            KeyCode::PageDown if paging => return self.scroll(1 - self.rows as i32),
            _ => (),
        }
        /* Typing goes back to the bottom, where it shows */
        self.term.scroll_display(Scroll::Bottom);
        let bytes = encode_key(key, self.mode());
        self.send(&bytes);
    }

    pub fn paste(&mut self, text: &str) {
        self.term.scroll_display(Scroll::Bottom);
        let bytes = encode_paste(text, self.mode());
        self.send(&bytes);
    }

    /* Moves the view `lines` back into what scrolled off, or forward with fewer */
    fn scroll(&mut self, lines: i32) {
        self.term.scroll_display(Scroll::Delta(lines));
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
        /* The wheel goes to a program that takes the mouse or has the whole screen;
         * otherwise it scrolls back through what scrolled off */
        let mode = self.mode();
        if !mode.intersects(TermMode::MOUSE_MODE) && !mode.contains(TermMode::ALT_SCREEN)
        {
            match kind {
                MouseEventKind::ScrollUp => return self.scroll(WHEEL_LINES),
                MouseEventKind::ScrollDown => return self.scroll(-WHEEL_LINES),
                _ => (),
            }
        }
        let bytes = encode_mouse(kind, mods, col, row, mode);
        self.send(&bytes);
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn resize(&mut self, cols: u16, rows: u16, pixels: (u16, u16)) {
        if (cols, rows, pixels) == (self.cols, self.rows, self.pixels) {
            return;
        }
        (self.cols, self.rows, self.pixels) = (cols, rows, pixels);
        let size = winsize(cols, rows, pixels);
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) };
        self.term.resize(Size {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        });
    }

    /* Whether the program has ended */
    pub fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }

    /* Hangs up on the program, as closing a terminal window does, and makes sure it
     * ends */
    pub fn kill(&mut self) {
        if self.exited() {
            return;
        }
        unsafe { libc::kill(self.child.id() as i32, libc::SIGHUP) };
        let end = Instant::now() + Duration::from_millis(300);
        while !self.exited() && Instant::now() < end {
            thread::sleep(Duration::from_millis(5));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /* The screen, row by row. `fg` and `bg` stand in for the program's default
     * colors. */
    pub fn screen(&self, fg: Color, bg: Color) -> Vec<Vec<Cell>> {
        let grid = self.term.grid();
        let back = grid.display_offset() as i32;
        (0..grid.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32 - back)];
                (0..grid.columns())
                    .map(|col| {
                        let cell = &row[Column(col)];
                        let flags = cell.flags;
                        let c = if flags.intersects(
                            Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER,
                        ) {
                            '\0'
                        /* The emulator keeps a tab where it was typed, which printed
                         * would jump the outer terminal's cursor on */
                        } else if flags.contains(Flags::HIDDEN) || cell.c.is_control() {
                            ' '
                        } else {
                            cell.c
                        };
                        let mut attributes = Attributes::default();
                        for (flag, attribute) in [
                            (Flags::BOLD, Attribute::Bold),
                            (Flags::DIM, Attribute::Dim),
                            (Flags::ITALIC, Attribute::Italic),
                            (Flags::ALL_UNDERLINES, Attribute::Underlined),
                            (Flags::STRIKEOUT, Attribute::CrossedOut),
                            (Flags::INVERSE, Attribute::Reverse),
                        ] {
                            if flags.intersects(flag) {
                                attributes.set(attribute);
                            }
                        }
                        Cell {
                            c,
                            marks: cell
                                .zerowidth()
                                .map(<[char]>::to_vec)
                                .unwrap_or_default(),
                            style: Style {
                                fg: color(cell.fg, fg, bg),
                                bg: color(cell.bg, fg, bg),
                                attributes,
                            },
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
            .map(|row| {
                row.iter()
                    .filter(|c| c.c != '\0')
                    .flat_map(|c| std::iter::once(c.c).chain(c.marks.iter().copied()))
                    .collect()
            })
            .collect()
    }

    /* Where the cursor is, 0-indexed, and its shape, when the program shows it */
    pub fn cursor(&self) -> Option<(u16, u16, CursorShape)> {
        /* Scrolled back, the cursor is below what shows */
        if !self.mode().contains(TermMode::SHOW_CURSOR)
            || self.term.grid().display_offset() > 0
        {
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

impl Drop for Pane {
    fn drop(&mut self) {
        self.kill();
    }
}

fn winsize(cols: u16, rows: u16, pixels: (u16, u16)) -> libc::winsize {
    libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: pixels.0,
        ws_ypixel: pixels.1,
    }
}

/* A pseudo-terminal: the terminal's side and the program's, neither of which other
 * programs viewmer starts inherit */
fn open_pty(cols: u16, rows: u16, pixels: (u16, u16)) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave) = (-1, -1);
    let mut size = winsize(cols, rows, pixels);
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if opened != 0 {
        return Err(io::Error::last_os_error());
    }
    let (master, slave) =
        unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    /* Text in and out is UTF-8, so erasing takes a whole character */
    unsafe {
        let mut term: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(slave.as_raw_fd(), &mut term) == 0 {
            term.c_iflag |= libc::IUTF8;
            libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &term);
        }
    }
    Ok((master, slave))
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

    /* Runs `sh -c script` and waits until `check` holds for the screen, or 5 seconds */
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
        pane.text()[row].trim_end().to_owned()
    }

    #[test]
    fn a_programs_output_lands_on_its_screen() {
        let pane = run_until("printf 'one\\r\\n\\033[31mtwo\\ttab'; sleep 2", |p| {
            row_text(p, 1).ends_with("tab")
        });
        assert_eq!(row_text(&pane, 0), "one");
        /* A tab is spaces on screen */
        assert_eq!(row_text(&pane, 1), "two     tab");
        let red = &pane.screen(Color::Reset, Color::Reset)[1][0];
        assert_eq!(red.style.fg, Color::AnsiValue(1));
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
    fn a_copy_goes_on_to_kittys_clipboard() {
        /* `hello`, as helix copies it over ssh */
        let mut pane = run_until("printf '\\033]52;c;aGVsbG8=\\007'; sleep 2", |p| {
            !p.graphics.is_empty()
        });
        assert_eq!(pane.take_graphics(), b"\x1b]52;c;aGVsbG8=\x1b\\");
    }

    #[test]
    fn what_scrolled_off_can_be_scrolled_back_to() {
        /* Four rows, ten lines: 1 to 6 scrolled off */
        let mut pane = run_until("seq 1 10; sleep 2", |p| {
            p.text().iter().any(|r| r.trim() == "10")
        });
        let top = |p: &Pane| p.text()[0].trim().to_owned();
        assert_eq!(top(&pane), "8");
        pane.mouse(MouseEventKind::ScrollUp, KeyModifiers::NONE, 0, 0);
        assert_eq!(top(&pane), "5");
        assert!(pane.cursor().is_none(), "no cursor while scrolled back");
        pane.key(&KeyEvent::new(KeyCode::PageUp, KeyModifiers::SHIFT));
        assert_eq!(top(&pane), "2");
        /* Typing goes back to the bottom */
        pane.key(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(top(&pane), "8");
    }

    #[test]
    fn the_pane_knows_when_its_program_ends() {
        let mut pane = run_until("exit 0", |_| true);
        let end = Instant::now() + Duration::from_secs(5);
        while !pane.exited() && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(pane.exited());
    }
}
