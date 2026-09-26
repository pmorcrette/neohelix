//! A shell running under a pseudo-terminal, with Alacritty's VT emulator on
//! top of it.
//!
//! Three threads are involved and none of them is the editor's:
//!
//! * a **reader** thread drains the PTY and feeds Alacritty's ANSI parser,
//! * a **writer** thread drains a channel and writes to the PTY,
//! * the editor's thread only ever locks the grid to draw it, and sends bytes
//!   to the channel.
//!
//! Nothing the editor does can block on the PTY: a shell producing output
//! faster than it is drawn, or refusing to read its input, slows the terminal
//! down but never the editor.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{ClipboardType, Config, Osc52, Term};
use alacritty_terminal::vte::ansi::{Processor, Rgb, StdSyncHandler};
use crossbeam_channel::{Sender, TrySendError};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// How many writes may be queued before the shell is considered wedged.
///
/// A shell that stops reading its input must not grow this without bound, and
/// dropping keystrokes is better than growing until memory runs out.
const WRITE_QUEUE: usize = 1024;

/// How a terminal is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    /// Lines kept above the screen, to scroll back through.
    pub scrollback: usize,
    /// Whether programs may switch the Kitty keyboard protocol on.
    pub kitty_keyboard: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            // Alacritty's own default, chosen here rather than inherited.
            scrollback: 10_000,
            kitty_keyboard: true,
        }
    }
}

/// The terminal Helix draws from.
pub type SharedTerm = Arc<FairMutex<Term<EventProxy>>>;

/// Errors starting or driving a terminal.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not open a pseudo-terminal: {0}")]
    OpenPty(String),
    #[error("could not start {shell}: {reason}")]
    Spawn { shell: String, reason: String },
    #[error("could not attach to the pseudo-terminal: {0}")]
    Attach(String),
}

/// The grid's size, in cells.
///
/// Alacritty asks for this through its own trait; implementing it here avoids
/// depending on the `test` helper it otherwise ships.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub columns: usize,
    pub screen_lines: usize,
}

impl TermSize {
    pub fn new(columns: usize, screen_lines: usize) -> Self {
        // A zero-sized grid would make Alacritty index out of bounds.
        Self {
            columns: columns.max(1),
            screen_lines: screen_lines.max(1),
        }
    }
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// Where a program asked for text to be copied (OSC 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clipboard {
    /// The clipboard proper.
    Clipboard,
    /// The primary selection, where there is one.
    Selection,
}

/// What the emulator told the embedder, kept until the view reads it: the
/// view runs on the editor's thread, the emulator on the reader's.
#[derive(Debug, Default)]
struct Notices {
    /// The title the running program set, if any (OSC 0 and 2).
    title: Option<String>,
    /// Text programs asked to copy, oldest first.
    copied: Vec<(Clipboard, String)>,
    /// Whether the bell rang since the view last looked.
    bell: bool,
    /// The grid's size, for programs that ask for it.
    size: Option<TermSize>,
    /// The default colours the view draws with, for programs that ask for
    /// them; `None` where they are not known.
    foreground: Option<(u8, u8, u8)>,
    background: Option<(u8, u8, u8)>,
}

/// Relays Alacritty's events out of the emulator.
///
/// Alacritty answers some escape sequences itself — device attributes, cursor
/// position reports — by handing back bytes to write to the PTY. Dropping
/// those hangs any program that waits for the reply, so they go to the writer
/// thread like anything else. The rest is either answered here, from what
/// the view has said about itself, or kept for the view to act on.
///
/// - The title, text to copy and the bell are kept for the view.
/// - A colour request is answered for the 240 fixed palette entries, whose
///   values every terminal agrees on, and for the default foreground and
///   background when the theme defines them. The sixteen named colours are
///   whatever the terminal Helix runs in makes them, which cannot be known
///   from here, so they go unanswered and the program keeps its own guess.
/// - A request for the text area's size is answered in cells; the pixel
///   size is not known, and is reported as zero, as terminals that cannot
///   tell do.
/// - Reading the clipboard (OSC 52 paste) is never offered: the emulator is
///   configured to accept only copying, so a program cannot read what the
///   user copied elsewhere.
/// - The cursor's blinking and the mouse pointer's shape have nothing to
///   act on: the view draws a steady block cursor in a text interface.
#[derive(Clone)]
pub struct EventProxy {
    writes: Sender<Vec<u8>>,
    redraw: Arc<dyn Fn() + Send + Sync>,
    notices: Arc<Mutex<Notices>>,
}

impl EventProxy {
    fn notices(&self) -> std::sync::MutexGuard<'_, Notices> {
        // A panic while holding it leaves plain data behind, still usable.
        self.notices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn reply(&self, text: String) {
        let _ = self.writes.try_send(text.into_bytes());
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.reply(text),
            Event::Wakeup => (self.redraw)(),
            Event::Bell => {
                self.notices().bell = true;
                (self.redraw)();
            }
            Event::Title(title) => {
                self.notices().title = Some(title);
                (self.redraw)();
            }
            Event::ResetTitle => {
                self.notices().title = None;
                (self.redraw)();
            }
            Event::ClipboardStore(kind, text) => {
                let kind = match kind {
                    ClipboardType::Clipboard => Clipboard::Clipboard,
                    ClipboardType::Selection => Clipboard::Selection,
                };
                self.notices().copied.push((kind, text));
                (self.redraw)();
            }
            Event::ColorRequest(index, format) => {
                let color = {
                    let notices = self.notices();
                    requested_color(index, notices.foreground, notices.background)
                };
                if let Some((r, g, b)) = color {
                    self.reply(format(Rgb { r, g, b }));
                }
            }
            Event::TextAreaSizeRequest(format) => {
                let size = self.notices().size;
                if let Some(size) = size {
                    self.reply(format(WindowSize {
                        num_lines: size.screen_lines as u16,
                        num_cols: size.columns as u16,
                        cell_width: 0,
                        cell_height: 0,
                    }));
                }
            }
            // Never offered: see above.
            Event::ClipboardLoad(..) => {}
            Event::CursorBlinkingChange | Event::MouseCursorDirty => {}
            // The reader sees the shell's end of file itself.
            Event::Exit | Event::ChildExit(_) => {}
        }
    }
}

/// The bytes of a paste. Inside the brackets, anything that would end them
/// early is taken out, so pasted text cannot pose as typing; outside, line
/// ends become carriage returns, as the Enter key sends.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let inner = text.replace("\x1b[201~", "").replace("\x1b[200~", "");
        format!("\x1b[200~{inner}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

/// The index Alacritty uses for the default foreground, then background.
const FOREGROUND_INDEX: usize = 256;
const BACKGROUND_INDEX: usize = 257;

/// The colour a program asked for by index, when it is known.
fn requested_color(
    index: usize,
    foreground: Option<(u8, u8, u8)>,
    background: Option<(u8, u8, u8)>,
) -> Option<(u8, u8, u8)> {
    match index {
        // The 6×6×6 cube.
        16..=231 => {
            let level = |value: usize| -> u8 {
                if value == 0 {
                    0
                } else {
                    (55 + 40 * value) as u8
                }
            };
            let cube = index - 16;
            Some((level(cube / 36), level(cube / 6 % 6), level(cube % 6)))
        }
        // The grey ramp.
        232..=255 => {
            let grey = (8 + 10 * (index - 232)) as u8;
            Some((grey, grey, grey))
        }
        FOREGROUND_INDEX => foreground,
        BACKGROUND_INDEX => background,
        _ => None,
    }
}

/// A shell attached to a pseudo-terminal.
pub struct PtyTerminal {
    term: SharedTerm,
    writes: Sender<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    size: TermSize,
    /// Set once the PTY reaches end of file, i.e. the shell exited.
    finished: Arc<AtomicBool>,
    notices: Arc<Mutex<Notices>>,
}

impl PtyTerminal {
    /// Starts the user's shell in a new pseudo-terminal.
    ///
    /// `redraw` is called from the reader thread whenever the grid changes; it
    /// must be cheap and must not block, since it runs on the read path.
    pub fn spawn(
        columns: u16,
        screen_lines: u16,
        working_directory: Option<std::path::PathBuf>,
        redraw: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, Error> {
        Self::spawn_with(
            columns,
            screen_lines,
            working_directory,
            redraw,
            Options::default(),
        )
    }

    /// [`spawn`](Self::spawn), set up as `options` says.
    pub fn spawn_with(
        columns: u16,
        screen_lines: u16,
        working_directory: Option<std::path::PathBuf>,
        redraw: Arc<dyn Fn() + Send + Sync>,
        options: Options,
    ) -> Result<Self, Error> {
        let size = TermSize::new(columns as usize, screen_lines as usize);
        let pty_size = PtySize {
            rows: size.screen_lines as u16,
            cols: size.columns as u16,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = native_pty_system()
            .openpty(pty_size)
            .map_err(|err| Error::OpenPty(err.to_string()))?;

        let shell = default_shell();
        let mut command = CommandBuilder::new(&shell);
        if let Some(directory) = working_directory {
            command.cwd(directory);
        }
        // Alacritty emulates a terminal of this class; claiming anything else
        // would make programs send sequences the emulator does not implement.
        command.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| Error::Spawn {
                shell: shell.clone(),
                reason: err.to_string(),
            })?;
        // The slave is held open by the child; keeping our end would stop the
        // reader ever seeing end of file when the shell exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|err| Error::Attach(err.to_string()))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|err| Error::Attach(err.to_string()))?;

        let (writes, pending) = crossbeam_channel::bounded::<Vec<u8>>(WRITE_QUEUE);
        let notices = Arc::new(Mutex::new(Notices {
            size: Some(size),
            ..Notices::default()
        }));
        let proxy = EventProxy {
            writes: writes.clone(),
            redraw: Arc::clone(&redraw),
            notices: Arc::clone(&notices),
        };
        let config = Config {
            // Programs may copy into the clipboard, never read it.
            osc52: Osc52::OnlyCopy,
            scrolling_history: options.scrollback,
            kitty_keyboard: options.kitty_keyboard,
            ..Config::default()
        };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, proxy)));
        let finished = Arc::new(AtomicBool::new(false));

        spawn_reader(
            reader,
            Arc::clone(&term),
            Arc::clone(&redraw),
            Arc::clone(&finished),
        );
        spawn_writer(writer, pending);

        Ok(Self {
            term,
            writes,
            master: pair.master,
            child,
            size,
            finished,
            notices,
        })
    }

    /// The emulator's grid, for rendering.
    pub fn term(&self) -> &SharedTerm {
        &self.term
    }

    /// The grid's current size.
    pub fn size(&self) -> TermSize {
        self.size
    }

    fn notices(&self) -> std::sync::MutexGuard<'_, Notices> {
        self.notices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The title the running program set, if it set one.
    pub fn title(&self) -> Option<String> {
        self.notices().title.clone()
    }

    /// The text programs asked to copy since the last call, oldest first.
    pub fn take_copied(&self) -> Vec<(Clipboard, String)> {
        std::mem::take(&mut self.notices().copied)
    }

    /// Whether the bell rang since the last call.
    pub fn take_bell(&self) -> bool {
        std::mem::take(&mut self.notices().bell)
    }

    /// Tells programs that ask what the default colours are: the view's own,
    /// when they are known colours rather than the host terminal's.
    pub fn set_default_colors(
        &self,
        foreground: Option<(u8, u8, u8)>,
        background: Option<(u8, u8, u8)>,
    ) {
        let mut notices = self.notices();
        notices.foreground = foreground;
        notices.background = background;
    }

    /// Queues bytes for the shell's standard input.
    ///
    /// Returns `false` when the queue is full, which means the shell has
    /// stopped reading; the keystroke is dropped rather than blocking the
    /// editor on it.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) -> bool {
        match self.writes.try_send(bytes.into()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                log::warn!("terminal input queue is full; dropping keystroke");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Sends `text` as a paste: marked as one when the program asked for
    /// bracketed paste, so a shell does not run it line by line as it
    /// arrives; typed as keys otherwise. Either way it lands at the bottom.
    pub fn paste(&self, text: &str) -> bool {
        let bracketed = {
            let mut term = self.term.lock();
            term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
            term.mode()
                .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
        };
        self.write(paste_bytes(text, bracketed))
    }

    /// Resizes both the emulator's grid and the pseudo-terminal.
    ///
    /// The grid is resized first so that output arriving in response to the
    /// `SIGWINCH` the PTY sends lands in a grid that already has the new
    /// shape. A resize to the current size is skipped, because it would signal
    /// the child for nothing.
    pub fn resize(&mut self, columns: u16, screen_lines: u16) {
        let size = TermSize::new(columns as usize, screen_lines as usize);
        if size == self.size {
            return;
        }
        self.size = size;
        self.notices().size = Some(size);

        self.term.lock().resize(size);

        if let Err(err) = self.master.resize(PtySize {
            rows: size.screen_lines as u16,
            cols: size.columns as u16,
            pixel_width: 0,
            pixel_height: 0,
        }) {
            log::error!("failed to resize the pseudo-terminal: {err}");
        }
    }

    /// Whether the shell has exited.
    pub fn has_exited(&mut self) -> bool {
        // The reader sees end of file first; `try_wait` then reaps the child.
        if self.finished.load(Ordering::Relaxed) {
            return true;
        }
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for PtyTerminal {
    fn drop(&mut self) {
        // Closing the view must not leave a shell running with no way to reach
        // it; the reader and writer threads then end on their own, because
        // their handles to the PTY close with it.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Feeds the PTY's output to the emulator.
fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    term: SharedTerm,
    redraw: Arc<dyn Fn() + Send + Sync>,
    finished: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("helix-pty-reader".into())
        .spawn(move || {
            let mut processor: Processor<StdSyncHandler> = Processor::new();
            let mut buffer = [0u8; 65536];

            loop {
                match reader.read(&mut buffer) {
                    // End of file: the shell exited and closed the PTY.
                    Ok(0) => break,
                    Ok(read) => {
                        // The lock is held only for parsing, never across the
                        // read, so a blocked read cannot stall rendering.
                        let mut term = term.lock();
                        processor.advance(&mut *term, &buffer[..read]);
                        drop(term);
                        redraw();
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(err) => {
                        log::debug!("terminal reader stopped: {err}");
                        break;
                    }
                }
            }

            finished.store(true, Ordering::Relaxed);
            redraw();
        })
        .expect("failed to start the terminal reader thread");
}

/// Writes queued bytes to the PTY.
fn spawn_writer(mut writer: Box<dyn Write + Send>, pending: crossbeam_channel::Receiver<Vec<u8>>) {
    std::thread::Builder::new()
        .name("helix-pty-writer".into())
        .spawn(move || {
            for bytes in pending {
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        })
        .expect("failed to start the terminal writer thread");
}

/// The user's shell, falling back to something that exists everywhere.
pub fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "powershell.exe".to_string()
            } else {
                "/bin/sh".to_string()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal with no PTY behind it: what it writes back is collected.
    fn emulator() -> (
        Term<EventProxy>,
        Arc<Mutex<Notices>>,
        crossbeam_channel::Receiver<Vec<u8>>,
    ) {
        let (writes, replies) = crossbeam_channel::unbounded();
        let size = TermSize::new(80, 24);
        let notices = Arc::new(Mutex::new(Notices {
            size: Some(size),
            foreground: Some((0xee, 0xee, 0xee)),
            ..Notices::default()
        }));
        let proxy = EventProxy {
            writes,
            redraw: Arc::new(|| {}),
            notices: Arc::clone(&notices),
        };
        let config = Config {
            osc52: Osc52::OnlyCopy,
            ..Config::default()
        };
        (Term::new(config, &size, proxy), notices, replies)
    }

    fn feed(term: &mut Term<EventProxy>, bytes: &[u8]) {
        let mut processor: Processor<StdSyncHandler> = Processor::new();
        processor.advance(term, bytes);
    }

    fn replies(receiver: &crossbeam_channel::Receiver<Vec<u8>>) -> Vec<String> {
        receiver
            .try_iter()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .collect()
    }

    #[test]
    fn the_title_and_the_bell_are_kept_for_the_view() {
        let (mut term, notices, _) = emulator();
        feed(&mut term, b"\x1b]2;vim notes.org\x07");
        assert_eq!(
            notices.lock().unwrap().title.as_deref(),
            Some("vim notes.org")
        );
        feed(&mut term, b"\x07");
        assert!(notices.lock().unwrap().bell);
    }

    #[test]
    fn copying_is_accepted_and_pasting_is_not() {
        let (mut term, notices, replies_to) = emulator();
        // "hello" in base64, to the clipboard.
        feed(&mut term, b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(
            notices.lock().unwrap().copied,
            [(Clipboard::Clipboard, "hello".to_string())]
        );
        // A request to read the clipboard gets no answer at all.
        feed(&mut term, b"\x1b]52;c;?\x07");
        assert!(replies(&replies_to).is_empty());
    }

    #[test]
    fn colours_are_answered_only_when_known() {
        let (mut term, _, replies_to) = emulator();
        // The foreground, known; the background, not.
        feed(&mut term, b"\x1b]10;?\x07\x1b]11;?\x07");
        let answers = replies(&replies_to);
        assert_eq!(answers.len(), 1, "{answers:?}");
        assert!(answers[0].contains("rgb:eeee/eeee/eeee"), "{answers:?}");

        // A cube entry is the same everywhere; a named colour is the host's.
        feed(&mut term, b"\x1b]4;196;?\x07\x1b]4;1;?\x07");
        let answers = replies(&replies_to);
        assert_eq!(answers.len(), 1, "{answers:?}");
        assert!(answers[0].contains("rgb:ffff/0000/0000"), "{answers:?}");
    }

    #[test]
    fn the_text_area_is_reported_in_cells() {
        let (mut term, _, replies_to) = emulator();
        feed(&mut term, b"\x1b[18t");
        assert_eq!(replies(&replies_to), ["\x1b[8;24;80t"]);
    }

    #[test]
    fn a_paste_is_bracketed_when_asked_and_cannot_close_its_brackets() {
        assert_eq!(
            paste_bytes("ls\nrm -rf x\n", true),
            b"\x1b[200~ls\nrm -rf x\n\x1b[201~"
        );
        assert_eq!(paste_bytes("a\x1b[201~b", true), b"\x1b[200~ab\x1b[201~");
        assert_eq!(paste_bytes("one\ntwo\r\n", false), b"one\rtwo\r");
    }

    #[test]
    fn the_fixed_palette_is_the_xterm_one() {
        assert_eq!(requested_color(16, None, None), Some((0, 0, 0)));
        assert_eq!(requested_color(21, None, None), Some((0, 0, 255)));
        assert_eq!(requested_color(231, None, None), Some((255, 255, 255)));
        assert_eq!(requested_color(232, None, None), Some((8, 8, 8)));
        assert_eq!(requested_color(255, None, None), Some((238, 238, 238)));
        assert_eq!(requested_color(3, None, None), None);
    }
}
