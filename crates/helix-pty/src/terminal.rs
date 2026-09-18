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
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use crossbeam_channel::{Sender, TrySendError};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// How many writes may be queued before the shell is considered wedged.
///
/// A shell that stops reading its input must not grow this without bound, and
/// dropping keystrokes is better than growing until memory runs out.
const WRITE_QUEUE: usize = 1024;

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

/// Relays Alacritty's events out of the emulator.
///
/// Alacritty answers some escape sequences itself — device attributes, cursor
/// position reports — by handing back bytes to write to the PTY. Dropping
/// those hangs any program that waits for the reply, so they go to the writer
/// thread like anything else.
#[derive(Clone)]
pub struct EventProxy {
    writes: Sender<Vec<u8>>,
    redraw: Arc<dyn Fn() + Send + Sync>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                let _ = self.writes.try_send(text.into_bytes());
            }
            Event::Wakeup | Event::Bell => (self.redraw)(),
            _ => {}
        }
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
        let proxy = EventProxy {
            writes: writes.clone(),
            redraw: Arc::clone(&redraw),
        };
        let term = Arc::new(FairMutex::new(Term::new(Config::default(), &size, proxy)));
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
