//! Encoding keys into the bytes a terminal expects.
//!
//! This is the xterm encoding every shell and full-screen program assumes.
//! It lives here rather than in the UI layer because it is terminal protocol,
//! not editor behaviour, and because it is worth testing on its own.

/// The modifiers a key was pressed with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
    };

    pub fn ctrl() -> Self {
        Self {
            ctrl: true,
            ..Self::NONE
        }
    }

    fn is_empty(self) -> bool {
        self == Self::NONE
    }

    /// xterm's modifier parameter: a bitfield, offset by one.
    fn parameter(self) -> u8 {
        1 + u8::from(self.shift) + 2 * u8::from(self.alt) + 4 * u8::from(self.ctrl)
    }
}

/// A key, in the subset a terminal can express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Tab,
    BackTab,
    Backspace,
    Escape,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Function(u8),
}

/// Encodes a keypress for the shell.
///
/// `application_cursor` reflects the terminal's DECCKM mode: programs like
/// `vim` and `less` switch it on and then expect `ESC O A` for Up rather than
/// `ESC [ A`. Ignoring it makes arrow keys insert junk in those programs.
pub fn encode_key(key: Key, modifiers: Modifiers, application_cursor: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);

    match key {
        Key::Char(c) => {
            if modifiers.alt {
                out.push(0x1b);
            }
            match control_byte(c, modifiers) {
                Some(byte) => out.push(byte),
                None => {
                    let mut buffer = [0u8; 4];
                    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
                }
            }
        }
        Key::Enter => {
            if modifiers.alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        Key::Tab => {
            if modifiers.alt {
                out.push(0x1b);
            }
            out.push(b'\t');
        }
        // Shift-Tab has its own sequence rather than a modifier parameter.
        Key::BackTab => out.extend_from_slice(b"\x1b[Z"),
        Key::Backspace => {
            if modifiers.alt {
                out.push(0x1b);
            }
            // Terminals send DEL for backspace; Ctrl-Backspace sends BS.
            out.push(if modifiers.ctrl { 0x08 } else { 0x7f });
        }
        Key::Escape => out.push(0x1b),

        Key::Left | Key::Right | Key::Up | Key::Down | Key::Home | Key::End => {
            let final_byte = match key {
                Key::Up => b'A',
                Key::Down => b'B',
                Key::Right => b'C',
                Key::Left => b'D',
                Key::Home => b'H',
                _ => b'F',
            };
            if modifiers.is_empty() {
                out.push(0x1b);
                out.push(if application_cursor { b'O' } else { b'[' });
                out.push(final_byte);
            } else {
                // A modified key always uses the CSI form, whatever the mode.
                out.extend_from_slice(b"\x1b[1;");
                out.extend_from_slice(modifiers.parameter().to_string().as_bytes());
                out.push(final_byte);
            }
        }

        Key::Insert => out.extend_from_slice(&tilde_sequence(2, modifiers)),
        Key::Delete => out.extend_from_slice(&tilde_sequence(3, modifiers)),
        Key::PageUp => out.extend_from_slice(&tilde_sequence(5, modifiers)),
        Key::PageDown => out.extend_from_slice(&tilde_sequence(6, modifiers)),

        Key::Function(n) => out.extend_from_slice(&function_key(n, modifiers)),
    }

    out
}

/// `ESC [ <number> ~`, with a modifier parameter when there is one.
fn tilde_sequence(number: u8, modifiers: Modifiers) -> Vec<u8> {
    let mut out = b"\x1b[".to_vec();
    out.extend_from_slice(number.to_string().as_bytes());
    if !modifiers.is_empty() {
        out.push(b';');
        out.extend_from_slice(modifiers.parameter().to_string().as_bytes());
    }
    out.push(b'~');
    out
}

/// F1–F4 are SS3 sequences; F5 upwards use the `~` form, with gaps in the
/// numbering that come from the original DEC keyboards.
fn function_key(n: u8, modifiers: Modifiers) -> Vec<u8> {
    match n {
        1..=4 if modifiers.is_empty() => {
            vec![0x1b, b'O', b'P' + (n - 1)]
        }
        1..=4 => {
            let mut out = b"\x1b[1;".to_vec();
            out.extend_from_slice(modifiers.parameter().to_string().as_bytes());
            out.push(b'P' + (n - 1));
            out
        }
        5 => tilde_sequence(15, modifiers),
        6..=10 => tilde_sequence(17 + (n - 6), modifiers),
        11 | 12 => tilde_sequence(23 + (n - 11), modifiers),
        _ => Vec::new(),
    }
}

/// The control character a key produces when held with Ctrl, if any.
fn control_byte(c: char, modifiers: Modifiers) -> Option<u8> {
    if !modifiers.ctrl {
        return None;
    }

    match c {
        // Ctrl-a through Ctrl-z are 0x01..=0x1a.
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        // The remaining control codes, as a terminal keyboard produces them.
        '@' | ' ' => Some(0x00),
        '[' => Some(0x1b),
        '\\' => Some(0x1c),
        ']' => Some(0x1d),
        '^' => Some(0x1e),
        '_' | '/' => Some(0x1f),
        '?' => Some(0x7f),
        _ => None,
    }
}

/// The parts of the Kitty keyboard protocol a program switched on.
///
/// Alacritty keeps the protocol's mode stack and answers queries about it;
/// what is left to the embedder is to encode keys as the modes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KittyModes {
    /// Flag 1: keys that are ambiguous in the legacy encoding — Escape,
    /// and anything with Ctrl or Alt — are sent as `CSI … u`.
    pub disambiguate: bool,
    /// Flag 8: every key is sent as an escape code, plain text included.
    pub all_as_escape: bool,
}

/// Encodes a keypress for a program that may have switched the Kitty
/// keyboard protocol on; without it, as [`encode_key`] does.
///
/// Key release events are not reported: Helix does not pass them on.
pub fn encode_key_with(
    key: Key,
    modifiers: Modifiers,
    application_cursor: bool,
    kitty: KittyModes,
) -> Vec<u8> {
    kitty_sequence(key, modifiers, kitty)
        .unwrap_or_else(|| encode_key(key, modifiers, application_cursor))
}

/// The `CSI code ; modifiers u` form of a key, when the active modes call
/// for it; `None` keeps the legacy encoding, which the protocol also uses for
/// the cursor, editing and function keys.
fn kitty_sequence(key: Key, mut modifiers: Modifiers, kitty: KittyModes) -> Option<Vec<u8>> {
    if !kitty.disambiguate && !kitty.all_as_escape {
        return None;
    }
    let code: u32 = match key {
        Key::Char(c) => {
            // The key's own code is the unshifted one.
            if c.is_ascii_uppercase() {
                modifiers.shift = true;
            }
            let plain_text = !modifiers.ctrl && !modifiers.alt;
            if plain_text && !kitty.all_as_escape {
                return None;
            }
            c.to_ascii_lowercase() as u32
        }
        Key::Escape => 27,
        Key::Enter | Key::Tab | Key::Backspace => {
            // Unmodified, these stay as they were, unless every key is to be
            // an escape code.
            if modifiers.is_empty() && !kitty.all_as_escape {
                return None;
            }
            match key {
                Key::Enter => 13,
                Key::Tab => 9,
                _ => 127,
            }
        }
        Key::BackTab => {
            modifiers.shift = true;
            9
        }
        _ => return None,
    };
    let mut out = format!("\x1b[{code}").into_bytes();
    if !modifiers.is_empty() {
        out.extend_from_slice(format!(";{}", modifiers.parameter()).as_bytes());
    }
    out.push(b'u');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(key: Key) -> Vec<u8> {
        encode_key(key, Modifiers::NONE, false)
    }

    #[test]
    fn a_plain_character_is_sent_as_itself() {
        assert_eq!(plain(Key::Char('a')), b"a");
        assert_eq!(plain(Key::Char('Z')), b"Z");
        // Non-ASCII goes through as UTF-8.
        assert_eq!(plain(Key::Char('é')), "é".as_bytes());
    }

    #[test]
    fn ctrl_letters_become_control_codes() {
        assert_eq!(encode_key(Key::Char('c'), Modifiers::ctrl(), false), [0x03]);
        assert_eq!(encode_key(Key::Char('d'), Modifiers::ctrl(), false), [0x04]);
        assert_eq!(encode_key(Key::Char('a'), Modifiers::ctrl(), false), [0x01]);
        // Case does not change the control code.
        assert_eq!(encode_key(Key::Char('C'), Modifiers::ctrl(), false), [0x03]);
    }

    #[test]
    fn the_other_control_codes_are_reachable() {
        for (c, byte) in [
            ('@', 0x00u8),
            (' ', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('_', 0x1f),
            ('?', 0x7f),
        ] {
            assert_eq!(
                encode_key(Key::Char(c), Modifiers::ctrl(), false),
                [byte],
                "Ctrl-{c}"
            );
        }
    }

    #[test]
    fn a_character_with_no_control_code_keeps_itself() {
        // Ctrl-1 has no control code; the shell should still see the digit.
        assert_eq!(encode_key(Key::Char('1'), Modifiers::ctrl(), false), b"1");
    }

    #[test]
    fn alt_prefixes_an_escape() {
        let alt = Modifiers {
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(encode_key(Key::Char('b'), alt, false), [0x1b, b'b']);
        assert_eq!(encode_key(Key::Enter, alt, false), [0x1b, b'\r']);

        // Alt and Ctrl together: escape, then the control code.
        let both = Modifiers {
            alt: true,
            ctrl: true,
            shift: false,
        };
        assert_eq!(encode_key(Key::Char('c'), both, false), [0x1b, 0x03]);
    }

    #[test]
    fn the_editing_keys_use_their_usual_bytes() {
        assert_eq!(plain(Key::Enter), b"\r");
        assert_eq!(plain(Key::Tab), b"\t");
        assert_eq!(plain(Key::Escape), [0x1b]);
        // Terminals send DEL for backspace, not BS.
        assert_eq!(plain(Key::Backspace), [0x7f]);
        assert_eq!(encode_key(Key::Backspace, Modifiers::ctrl(), false), [0x08]);
        assert_eq!(plain(Key::BackTab), b"\x1b[Z");
    }

    #[test]
    fn arrows_follow_the_application_cursor_mode() {
        // Normal mode, as a shell prompt expects.
        assert_eq!(plain(Key::Up), b"\x1b[A");
        assert_eq!(plain(Key::Down), b"\x1b[B");
        assert_eq!(plain(Key::Right), b"\x1b[C");
        assert_eq!(plain(Key::Left), b"\x1b[D");

        // Application mode, as `vim` and `less` switch on.
        assert_eq!(encode_key(Key::Up, Modifiers::NONE, true), b"\x1bOA");
        assert_eq!(encode_key(Key::Left, Modifiers::NONE, true), b"\x1bOD");
    }

    #[test]
    fn a_modified_arrow_always_uses_the_csi_form() {
        // Ctrl-Right: parameter 5 (1 + 4).
        assert_eq!(
            encode_key(Key::Right, Modifiers::ctrl(), false),
            b"\x1b[1;5C"
        );
        // Even in application cursor mode, a modifier forces CSI.
        assert_eq!(
            encode_key(Key::Right, Modifiers::ctrl(), true),
            b"\x1b[1;5C"
        );
        // Shift-Up: parameter 2.
        let shift = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(encode_key(Key::Up, shift, false), b"\x1b[1;2A");
        // Ctrl-Alt-Shift-Down: 1 + 1 + 2 + 4 = 8.
        let all = Modifiers {
            ctrl: true,
            alt: true,
            shift: true,
        };
        assert_eq!(encode_key(Key::Down, all, false), b"\x1b[1;8B");
    }

    #[test]
    fn home_and_end_share_the_arrow_encoding() {
        assert_eq!(plain(Key::Home), b"\x1b[H");
        assert_eq!(plain(Key::End), b"\x1b[F");
        assert_eq!(encode_key(Key::Home, Modifiers::NONE, true), b"\x1bOH");
        assert_eq!(encode_key(Key::End, Modifiers::ctrl(), false), b"\x1b[1;5F");
    }

    #[test]
    fn the_navigation_keys_use_the_tilde_form() {
        assert_eq!(plain(Key::Insert), b"\x1b[2~");
        assert_eq!(plain(Key::Delete), b"\x1b[3~");
        assert_eq!(plain(Key::PageUp), b"\x1b[5~");
        assert_eq!(plain(Key::PageDown), b"\x1b[6~");

        assert_eq!(
            encode_key(Key::Delete, Modifiers::ctrl(), false),
            b"\x1b[3;5~"
        );
    }

    #[test]
    fn function_keys_split_between_two_encodings() {
        // F1-F4 are SS3.
        assert_eq!(plain(Key::Function(1)), b"\x1bOP");
        assert_eq!(plain(Key::Function(4)), b"\x1bOS");
        // F5 upwards use the tilde form, with the historical gaps.
        assert_eq!(plain(Key::Function(5)), b"\x1b[15~");
        assert_eq!(plain(Key::Function(6)), b"\x1b[17~");
        assert_eq!(plain(Key::Function(10)), b"\x1b[21~");
        assert_eq!(plain(Key::Function(11)), b"\x1b[23~");
        assert_eq!(plain(Key::Function(12)), b"\x1b[24~");

        // A modified F-key uses the CSI form even below F5.
        assert_eq!(
            encode_key(Key::Function(1), Modifiers::ctrl(), false),
            b"\x1b[1;5P"
        );

        // Nothing is emitted for a key the encoding has no room for.
        assert!(plain(Key::Function(25)).is_empty());
    }

    #[test]
    fn the_modifier_parameter_matches_xterms_bitfield() {
        assert_eq!(Modifiers::NONE.parameter(), 1);
        assert_eq!(
            Modifiers {
                shift: true,
                ..Modifiers::NONE
            }
            .parameter(),
            2
        );
        assert_eq!(
            Modifiers {
                alt: true,
                ..Modifiers::NONE
            }
            .parameter(),
            3
        );
        assert_eq!(Modifiers::ctrl().parameter(), 5);
    }

    #[test]
    fn the_kitty_protocol_disambiguates_what_legacy_cannot() {
        let flag1 = KittyModes {
            disambiguate: true,
            all_as_escape: false,
        };
        let kitty = |key, modifiers| encode_key_with(key, modifiers, false, flag1);
        // Ctrl-i and Tab are the same byte in the legacy encoding.
        assert_eq!(encode_key(Key::Char('i'), Modifiers::ctrl(), false), b"\t");
        assert_eq!(kitty(Key::Char('i'), Modifiers::ctrl()), b"\x1b[105;5u");
        assert_eq!(kitty(Key::Tab, Modifiers::NONE), b"\t");
        assert_eq!(kitty(Key::Escape, Modifiers::NONE), b"\x1b[27u");
        // Plain text and the cursor keys are as before.
        assert_eq!(kitty(Key::Char('a'), Modifiers::NONE), b"a");
        assert_eq!(kitty(Key::Up, Modifiers::NONE), b"\x1b[A");
        let alt_shift = Modifiers {
            alt: true,
            shift: true,
            ..Modifiers::NONE
        };
        assert_eq!(kitty(Key::Enter, alt_shift), b"\x1b[13;4u");
        assert_eq!(kitty(Key::BackTab, Modifiers::NONE), b"\x1b[9;2u");
    }

    #[test]
    fn every_key_can_be_an_escape_code() {
        let all = KittyModes {
            disambiguate: true,
            all_as_escape: true,
        };
        let kitty = |key| encode_key_with(key, Modifiers::NONE, false, all);
        assert_eq!(kitty(Key::Char('a')), b"\x1b[97u");
        assert_eq!(kitty(Key::Char('A')), b"\x1b[97;2u");
        assert_eq!(kitty(Key::Enter), b"\x1b[13u");
        // Off, nothing changes.
        assert_eq!(
            encode_key_with(Key::Escape, Modifiers::NONE, false, KittyModes::default()),
            b"\x1b"
        );
    }
}
