use std::{
    sync::{
        atomic::Ordering,
        mpsc::{channel, Receiver},
    },
    thread,
};

use crossterm::event::{read, Event, KeyEvent, MouseEvent, MouseEventKind};

use super::{wake, Wake};
use crate::globals::RUNNING;

pub enum InputEvent {
    Key(KeyEvent),
    /* The wheel */
    MouseScroll(MouseEventKind),
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
                        ..
                    },
                ) => {
                    wake(Wake::Input(InputEvent::MouseScroll(kind)))
                        && wake(Wake::Mouse(event))
                }
                Event::Mouse(event) => wake(Wake::Mouse(event)),
                Event::Resize(..) => wake(Wake::Resize),
                Event::FocusGained | Event::FocusLost => true,
            };
            if !sent {
                return;
            }
        }
    });
    receiver
}
