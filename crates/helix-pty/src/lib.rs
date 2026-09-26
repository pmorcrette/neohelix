//! The integrated terminal: a shell under a pseudo-terminal, emulated by
//! Alacritty's VT parser.
//!
//! This crate owns everything that does not need the editor — the PTY, the
//! emulator's grid, and the encoding of keys into the bytes a terminal
//! expects. `helix-term` draws the grid and feeds it keys.

pub mod copy;
pub mod keys;
pub mod mouse;
pub mod terminal;

pub use keys::{encode_key, encode_key_with, Key, KittyModes, Modifiers};
pub use mouse::{encode_mouse, MouseAction, MouseButton, MouseModes, MouseReport};
pub use terminal::{default_shell, Clipboard, Error, Options, PtyTerminal, SharedTerm, TermSize};
