use std::{sync::atomic::Ordering, thread};

use crossbeam_channel::{unbounded, Receiver};
use crossterm::event::{read, Event, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::globals::RUNNING;

pub enum InputEvent {
    Key(KeyEvent),
    /* The wheel, and where the mouse was, in pixels */
    MouseScroll(MouseEventKind, KeyModifiers, u16, u16),
    Paste(String),
    /* The terminal window gained or lost focus */
    Focus(bool),
}

pub struct EventThreadData(
    pub Receiver<InputEvent>,
    pub Receiver<MouseEvent>,
    pub Receiver<String>,
    pub Receiver<(u16, u16)>,
);

pub fn spawn() -> EventThreadData {
    let (sender_input, receive_input) = unbounded::<InputEvent>();
    let (sender_mouse, receive_mouse) = unbounded::<MouseEvent>();
    let (sender_gr, receive_gr) = unbounded::<String>();
    let (sender_ws, receive_ws) = unbounded::<(u16, u16)>();

    /* Stops when the terminal can't be read any more or the main loop is gone; the
     * main loop sees the closed channels and exits */
    thread::spawn(move || {
        while RUNNING.load(Ordering::Acquire) {
            let Ok(event) = read() else { return };
            let sent = match event {
                Event::Key(event) => sender_input.send(InputEvent::Key(event)).is_ok(),
                Event::ApplicationProgramCommand(command) => {
                    let answer = command.split_once(';').map_or("", |(_, a)| a);
                    sender_gr.send(answer.to_owned()).is_ok()
                }
                Event::Mouse(
                    event @ MouseEvent {
                        kind:
                            kind @ (MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight),
                        modifiers,
                        column,
                        row,
                    },
                ) => {
                    sender_input
                        .send(InputEvent::MouseScroll(kind, modifiers, column, row))
                        .is_ok()
                        && sender_mouse.send(event).is_ok()
                }
                Event::Mouse(event) => sender_mouse.send(event).is_ok(),
                Event::Resize(width, height) => sender_ws.send((width, height)).is_ok(),
                Event::Paste(text) => sender_input.send(InputEvent::Paste(text)).is_ok(),
                Event::FocusGained => sender_input.send(InputEvent::Focus(true)).is_ok(),
                Event::FocusLost => sender_input.send(InputEvent::Focus(false)).is_ok(),
            };
            if !sent {
                return;
            }
        }
    });

    EventThreadData(receive_input, receive_mouse, receive_gr, receive_ws)
}
