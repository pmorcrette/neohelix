//! Encoding mouse events for programs that asked for them.
//!
//! A program switches mouse reporting on with one of three modes — clicks
//! only, clicks and drags, or all motion — and chooses how coordinates are
//! written: the SGR form (`CSI < b ; x ; y M`), which has no limit, the
//! UTF-8 form, or the original X10 bytes, which stop at column 223.

use crate::keys::Modifiers;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    /// Moving with a button held.
    Drag(MouseButton),
    /// Moving with no button held.
    Move,
    WheelUp,
    WheelDown,
}

/// A mouse event at a cell of the grid, counted from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseReport {
    pub action: MouseAction,
    pub column: usize,
    pub line: usize,
    pub modifiers: Modifiers,
}

/// What the program asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseModes {
    /// Presses and releases (mode 1000).
    pub click: bool,
    /// Also moves with a button held (1002).
    pub drag: bool,
    /// Also any move (1003).
    pub motion: bool,
    /// The SGR encoding (1006).
    pub sgr: bool,
    /// The UTF-8 encoding (1005).
    pub utf8: bool,
}

impl MouseModes {
    /// Whether the program wants any mouse events at all.
    pub fn any(self) -> bool {
        self.click || self.drag || self.motion
    }
}

/// The bytes for `report`, or `None` when the program did not ask for this
/// kind of event, or the position cannot be written in its encoding.
pub fn encode_mouse(report: MouseReport, modes: MouseModes) -> Option<Vec<u8>> {
    let wanted = match report.action {
        MouseAction::Press(_)
        | MouseAction::Release(_)
        | MouseAction::WheelUp
        | MouseAction::WheelDown => modes.any(),
        MouseAction::Drag(_) => modes.drag || modes.motion,
        MouseAction::Move => modes.motion,
    };
    if !wanted {
        return None;
    }

    let button = |button: MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let mut code: u32 = match report.action {
        MouseAction::Press(pressed) => button(pressed),
        // The legacy encodings cannot say which button was released.
        MouseAction::Release(released) if modes.sgr => button(released),
        MouseAction::Release(_) => 3,
        MouseAction::Drag(held) => button(held) + 32,
        MouseAction::Move => 3 + 32,
        MouseAction::WheelUp => 64,
        MouseAction::WheelDown => 65,
    };
    if report.modifiers.shift {
        code += 4;
    }
    if report.modifiers.alt {
        code += 8;
    }
    if report.modifiers.ctrl {
        code += 16;
    }
    let (x, y) = (report.column + 1, report.line + 1);

    if modes.sgr {
        let last = if matches!(report.action, MouseAction::Release(_)) {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{x};{y}{last}").into_bytes());
    }

    let mut out = b"\x1b[M".to_vec();
    out.push(u8::try_from(32 + code).ok()?);
    for coordinate in [x, y] {
        let value = 32 + coordinate;
        if modes.utf8 {
            let c = char::from_u32(u32::try_from(value).ok()?)?;
            let mut buffer = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
        } else {
            out.push(u8::try_from(value).ok()?);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(action: MouseAction, column: usize, line: usize) -> MouseReport {
        MouseReport {
            action,
            column,
            line,
            modifiers: Modifiers::NONE,
        }
    }

    const CLICKS: MouseModes = MouseModes {
        click: true,
        drag: false,
        motion: false,
        sgr: false,
        utf8: false,
    };

    #[test]
    fn nothing_is_reported_unless_asked_for() {
        let press = at(MouseAction::Press(MouseButton::Left), 0, 0);
        assert_eq!(encode_mouse(press, MouseModes::default()), None);
        // Clicks only: no drags, no moves.
        assert!(encode_mouse(press, CLICKS).is_some());
        assert_eq!(
            encode_mouse(at(MouseAction::Drag(MouseButton::Left), 1, 1), CLICKS),
            None
        );
        assert_eq!(encode_mouse(at(MouseAction::Move, 1, 1), CLICKS), None);
    }

    #[test]
    fn the_sgr_form_names_the_released_button_and_has_no_limit() {
        let sgr = MouseModes {
            sgr: true,
            ..CLICKS
        };
        assert_eq!(
            encode_mouse(at(MouseAction::Press(MouseButton::Right), 299, 9), sgr).unwrap(),
            b"\x1b[<2;300;10M"
        );
        assert_eq!(
            encode_mouse(at(MouseAction::Release(MouseButton::Right), 299, 9), sgr).unwrap(),
            b"\x1b[<2;300;10m"
        );
        let mut wheel = at(MouseAction::WheelDown, 0, 0);
        wheel.modifiers = Modifiers::ctrl();
        assert_eq!(encode_mouse(wheel, sgr).unwrap(), b"\x1b[<81;1;1M");
    }

    #[test]
    fn the_x10_form_is_bytes_and_stops_at_its_limit() {
        assert_eq!(
            encode_mouse(at(MouseAction::Press(MouseButton::Left), 4, 2), CLICKS).unwrap(),
            b"\x1b[M %#"
        );
        assert_eq!(
            encode_mouse(at(MouseAction::Release(MouseButton::Left), 4, 2), CLICKS).unwrap(),
            b"\x1b[M#%#"
        );
        assert_eq!(
            encode_mouse(at(MouseAction::Press(MouseButton::Left), 300, 2), CLICKS),
            None
        );
        let utf8 = MouseModes {
            utf8: true,
            ..CLICKS
        };
        let far = encode_mouse(at(MouseAction::Press(MouseButton::Left), 300, 2), utf8).unwrap();
        assert_eq!(&far[..4], b"\x1b[M ");
        assert_eq!(std::str::from_utf8(&far[4..]).unwrap(), "\u{14d}#");
    }

    #[test]
    fn drags_and_moves_carry_the_motion_bit() {
        let all = MouseModes {
            motion: true,
            sgr: true,
            ..CLICKS
        };
        assert_eq!(
            encode_mouse(at(MouseAction::Drag(MouseButton::Left), 0, 0), all).unwrap(),
            b"\x1b[<32;1;1M"
        );
        assert_eq!(
            encode_mouse(at(MouseAction::Move, 0, 0), all).unwrap(),
            b"\x1b[<35;1;1M"
        );
    }
}
