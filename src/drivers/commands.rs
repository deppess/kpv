use core::fmt;
use crossterm::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearImages;
impl Command for ClearImages {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        /* Only the pages, by their layers: the panels' tints stay */
        write!(
            f,
            "\x1B_Gq=2,a=d,d=z,z={}\x1B\\\x1B_Gq=2,a=d,d=z,z={}\x1B\\",
            crate::drivers::graphics::PAGE_Z,
            crate::drivers::graphics::UNDER_Z
        )
    }
}

/* Frees every image tpv gave kitty, placed or kept for later, so that kitty gets
 * its memory back when tpv leaves */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreeImages;
impl Command for FreeImages {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1B_Gq=2,a=d,d=A\x1B\\\x1B_Gq=2,a=d,d=R,x=1,y=4294967295\x1B\\")
    }
}

/* A small hack to get cursor position in pixels
 * Replacing ?1006 with ?1016h reports cursor position in pixels instead of cells */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnableMouseCapturePixels;
impl Command for EnableMouseCapturePixels {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(concat!(
            "\x1B[?1000h",
            "\x1B[?1002h",
            "\x1B[?1003h",
            "\x1B[?1015h",
            "\x1B[?1016h",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisableMouseCapturePixels;
impl Command for DisableMouseCapturePixels {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(concat!(
            "\x1B[?1016l",
            "\x1B[?1015l",
            "\x1B[?1003l",
            "\x1B[?1002l",
            "\x1B[?1000l",
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerShape {
    Default,
    Pointer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetPointerShape(pub PointerShape);
impl Command for SetPointerShape {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        let name = match self.0 {
            PointerShape::Default => "",
            PointerShape::Pointer => "pointer",
        };
        write!(f, "\x1B]22;{}\x1B\\", name)
    }
}
