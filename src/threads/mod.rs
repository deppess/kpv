pub mod event;
pub mod fnotify;
pub mod renderer;
pub mod signals;

use std::sync::{
    mpsc::{channel, Receiver, Sender},
    OnceLock,
};

use crossterm::event::MouseEvent;

use event::InputEvent;
use renderer::RendererResult;

/* What wakes the main loop. Every thread sends here, so the loop waits in one place. */
pub enum Wake {
    Input(InputEvent),
    Mouse(MouseEvent),
    Resize,
    Rendered(RendererResult),
    /* The file changed on disk and settled */
    Reload,
    /* Something wants the screen drawn again */
    Redraw,
    /* The window closed, `kill` or an interrupt: quit as on `q` */
    Stop,
    /* A part the viewer can't go on without stopped */
    Fatal(String),
}

static WAKER: OnceLock<Sender<Wake>> = OnceLock::new();

/* Makes the main loop's inbox; call once, before starting the threads */
pub fn inbox() -> Receiver<Wake> {
    let (sender, receiver) = channel();
    let _ = WAKER.set(sender);
    receiver
}

/* Wakes the main loop; `false` when there's none to wake */
pub fn wake(wake: Wake) -> bool {
    WAKER.get().is_some_and(|sender| sender.send(wake).is_ok())
}
