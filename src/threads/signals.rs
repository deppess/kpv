use std::thread;

use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGTERM},
    iterator::Signals,
};

use super::{wake, Wake};

/* Reports a request to stop: the terminal window closing (SIGHUP), `kill` (SIGTERM) or
 * an interrupt (SIGINT). The main loop then quits as it does on `q`, so the position
 * is written and the terminal restored instead of the process just ending. */
pub fn spawn() -> Result<(), String> {
    let mut signals = Signals::new([SIGHUP, SIGINT, SIGTERM])
        .map_err(|x| format!("Could not listen for signals: {}", x))?;
    thread::spawn(move || {
        for _ in signals.forever() {
            if !wake(Wake::Stop) {
                return;
            }
        }
    });
    Ok(())
}
