/* The crash watcher. `viewmer` runs itself as a child and waits for it. When the
 * viewer ends normally, so does the watcher, with its exit code. When it crashes, the
 * watcher gives the terminal back, writes down what happened and starts it again; the
 * viewer then opens as it was, from the position and layout it keeps saved. */

use std::{
    io::Write,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::store::Store;

/* Set for the viewer the watcher runs */
pub const WATCHED: &str = "VIEWMER_WATCHED";
/* Set when it was started again after a crash */
pub const RECOVERED: &str = "VIEWMER_RECOVERED";

/* More crashes than this within a minute and the watcher stops trying */
const MAX_CRASHES: usize = 3;

/* Signals that mean the viewer crashed, rather than was told to stop */
const CRASHES: [i32; 6] = [
    libc::SIGABRT,
    libc::SIGSEGV,
    libc::SIGBUS,
    libc::SIGILL,
    libc::SIGFPE,
    libc::SIGTRAP,
];

pub fn crash_log() -> Option<PathBuf> {
    Some(Store::default_dir()?.join("crash.log"))
}

/* Adds a line to the crash log */
pub fn log(line: &str) {
    let Some(file) = crash_log() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
    {
        let _ = writeln!(f, "{}  {}", now(), line);
    }
}

/* The time in UTC, as 2026-11-18 14:05:09 */
fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    /* Days to a civil date, as in Howard Hinnant's algorithm */
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year,
        month,
        day,
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/* Runs the viewer under the watcher and exits with it. Returns only when it can't
 * watch, and the viewer should just run. */
pub fn run(args: &[String], reset_screen: fn()) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    /* The terminal's settings as they are now, to put back after a crash */
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    let have_saved = unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut saved) } == 0;

    /* The watcher outlives the signals that stop the viewer, and passes them on */
    let flags: Vec<(i32, Arc<AtomicBool>)> = [SIGHUP, SIGINT, SIGTERM]
        .into_iter()
        .map(|sig| (sig, Arc::new(AtomicBool::new(false))))
        .collect();
    for (sig, flag) in &flags {
        if signal_hook::flag::register(*sig, flag.clone()).is_err() {
            return;
        }
    }

    let mut crashes: Vec<Instant> = Vec::new();
    let mut hung_up = false;
    loop {
        let mut command = Command::new(&exe);
        command.args(&args[1..]).env(WATCHED, "1");
        if !crashes.is_empty() {
            command.env(RECOVERED, "1");
        }
        let Ok(mut child) = command.spawn() else {
            return;
        };
        let pid = child.id() as libc::pid_t;

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => (),
                Err(_) => std::process::exit(1),
            }
            for (sig, flag) in &flags {
                if flag.swap(false, Ordering::AcqRel) {
                    hung_up |= *sig == SIGHUP;
                    unsafe { libc::kill(pid, *sig) };
                }
            }
            thread::sleep(Duration::from_millis(50));
        };

        let Some(signal) = status.signal() else {
            std::process::exit(status.code().unwrap_or(1));
        };
        /* The viewer can't have given the terminal back */
        reset_screen();
        if have_saved {
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &saved) };
        }
        if !CRASHES.contains(&signal) || hung_up {
            std::process::exit(128 + signal);
        }

        let file = args.get(1).map(String::as_str).unwrap_or("");
        log(&format!(
            "viewmer crashed with signal {} on {}",
            signal, file
        ));
        crashes.retain(|t| t.elapsed() < Duration::from_secs(60));
        crashes.push(Instant::now());
        if crashes.len() > MAX_CRASHES {
            log("crashed too often in a minute; not starting it again");
            eprintln!(
                "viewmer: crashed {} times in a minute; what happened is in {}",
                crashes.len(),
                crash_log()
                    .map(|f| f.display().to_string())
                    .unwrap_or_default()
            );
            std::process::exit(1);
        }
        log("started it again");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_reads_as_a_date() {
        let text = now();
        assert_eq!(text.len(), 23, "{}", text);
        assert!(text.starts_with("20") && text.ends_with(" UTC"));
    }
}
