use std::{sync::atomic::Ordering, thread};

use crossbeam_channel::{unbounded, Receiver};
use crossterm::event::{read, Event, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::{drivers::graphics::GraphicsResponse, globals::RUNNING};

pub enum InputEvent {
    Key(KeyEvent),
    MouseScroll(MouseEventKind, KeyModifiers),
}

pub struct EventThreadData(
    pub Receiver<InputEvent>,
    pub Receiver<MouseEvent>,
    pub Receiver<GraphicsResponse>,
    pub Receiver<(u16, u16)>,
);

pub fn spawn() -> EventThreadData {
    let (sender_input, receive_input) = unbounded::<InputEvent>();
    let (sender_mouse, receive_mouse) = unbounded::<MouseEvent>();
    let (sender_gr, receive_gr) = unbounded::<GraphicsResponse>();
    let (sender_ws, receive_ws) = unbounded::<(u16, u16)>();

    /* Stops when the terminal can't be read any more or the main loop is gone; the
     * main loop sees the closed channels and exits */
    thread::spawn(move || {
        while RUNNING.load(Ordering::Acquire) {
            let Ok(event) = read() else { return };
            let sent = match event {
                Event::Key(event) => sender_input.send(InputEvent::Key(event)).is_ok(),
                Event::ApplicationProgramCommand(command) => {
                    sender_gr.send(GraphicsResponse::new(&command)).is_ok()
                }
                Event::Mouse(
                    event @ MouseEvent {
                        kind:
                            kind @ (MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight),
                        modifiers,
                        ..
                    },
                ) => {
                    sender_input
                        .send(InputEvent::MouseScroll(kind, modifiers))
                        .is_ok()
                        && sender_mouse.send(event).is_ok()
                }
                Event::Mouse(event) => sender_mouse.send(event).is_ok(),
                Event::Resize(width, height) => sender_ws.send((width, height)).is_ok(),
                _ => true,
            };
            if !sent {
                return;
            }
        }
    });

    EventThreadData(receive_input, receive_mouse, receive_gr, receive_ws)
}
