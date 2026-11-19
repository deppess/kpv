use std::{
    sync::{
        atomic::Ordering,
        mpsc::{channel, Receiver},
    },
    thread,
};

use crossterm::event::{read, Event, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use super::{wake, Wake};
use crate::globals::RUNNING;

pub enum InputEvent {
    Key(KeyEvent),
    /* The wheel, and where the mouse was, in pixels */
    MouseScroll(MouseEventKind, KeyModifiers, u16, u16),
    Paste(String),
    /* The terminal window gained or lost focus */
    Focus(bool),
}

/* Reads the terminal: keys, the mouse and the window's size go to the main loop, and
 * kitty's answers to image commands (the text after `;`, such as `OK`) to the channel
 * returned */
pub fn spawn() -> Receiver<String> {
    let (answers, receiver) = channel::<String>();
    thread::spawn(move || {
        while RUNNING.load(Ordering::Acquire) {
            let Ok(event) = read() else {
                wake(Wake::Fatal("Lost the terminal".to_owned()));
                return;
            };
            let sent = match event {
                Event::Key(event) => wake(Wake::Input(InputEvent::Key(event))),
                Event::ApplicationProgramCommand(command) => {
                    let answer = command.split_once(';').map_or("", |(_, a)| a);
                    answers.send(answer.to_owned()).is_ok()
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
                    wake(Wake::Input(InputEvent::MouseScroll(
                        kind, modifiers, column, row,
                    ))) && wake(Wake::Mouse(event))
                }
                Event::Mouse(event) => wake(Wake::Mouse(event)),
                Event::Resize(..) => wake(Wake::Resize),
                Event::Paste(text) => wake(Wake::Input(InputEvent::Paste(text))),
                Event::FocusGained => wake(Wake::Input(InputEvent::Focus(true))),
                Event::FocusLost => wake(Wake::Input(InputEvent::Focus(false))),
            };
            if !sent {
                return;
            }
        }
    });
    receiver
}
