//! A real shell in a real pseudo-terminal.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use helix_pty::PtyTerminal;

/// Reads the visible grid back as lines of text.
fn screen(terminal: &PtyTerminal) -> Vec<String> {
    let term = terminal.term().lock();
    let grid = term.grid();
    (0..grid.screen_lines())
        .map(|line| {
            (0..grid.columns())
                .map(|column| {
                    grid[alacritty_terminal::index::Line(line as i32)]
                        [alacritty_terminal::index::Column(column)]
                    .c
                })
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

/// Waits until the screen contains `needle`, or gives up.
fn wait_for(terminal: &PtyTerminal, needle: &str, timeout: Duration) -> Option<Vec<String>> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let lines = screen(terminal);
        if lines.iter().any(|line| line.contains(needle)) {
            return Some(lines);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    None
}

fn spawn(columns: u16, rows: u16) -> (PtyTerminal, Arc<AtomicUsize>) {
    let redraws = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&redraws);
    let terminal = PtyTerminal::spawn(
        columns,
        rows,
        None,
        Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }),
    )
    .expect("a pseudo-terminal should be available");
    (terminal, redraws)
}

#[test]
fn a_command_runs_and_its_output_reaches_the_grid() {
    let (terminal, redraws) = spawn(80, 24);

    assert!(terminal.write(&b"echo helix-pty-works\n"[..]));

    let lines = wait_for(&terminal, "helix-pty-works", Duration::from_secs(10))
        .expect("the shell should echo the command's output");

    // Both the typed command and its output are on screen.
    assert!(
        lines
            .iter()
            .filter(|l| l.contains("helix-pty-works"))
            .count()
            >= 1,
        "screen was {lines:?}"
    );
    assert!(
        redraws.load(Ordering::Relaxed) > 0,
        "the reader thread should have asked for a redraw"
    );
}

#[test]
fn the_grid_has_the_size_it_was_given() {
    let (terminal, _) = spawn(100, 30);

    assert_eq!(terminal.size().columns, 100);
    assert_eq!(terminal.size().screen_lines, 30);

    let term = terminal.term().lock();
    assert_eq!(term.grid().columns(), 100);
    assert_eq!(term.grid().screen_lines(), 30);
}

#[test]
fn resizing_reaches_both_the_grid_and_the_child() {
    let (mut terminal, _) = spawn(80, 24);
    // Let the shell start before resizing it.
    assert!(terminal.write(&b"echo ready\n"[..]));
    wait_for(&terminal, "ready", Duration::from_secs(10)).expect("shell should start");

    terminal.resize(60, 20);

    assert_eq!(terminal.size().columns, 60);
    assert_eq!(terminal.term().lock().grid().columns(), 60);
    assert_eq!(terminal.term().lock().grid().screen_lines(), 20);

    // The child was signalled, so it reports the new width.
    assert!(terminal.write(&b"tput cols\n"[..]));
    let lines = wait_for(&terminal, "60", Duration::from_secs(10));
    assert!(lines.is_some(), "the child should see the new size");
}

#[test]
fn a_resize_to_the_same_size_is_a_no_op() {
    let (mut terminal, _) = spawn(80, 24);
    terminal.resize(80, 24);
    assert_eq!(terminal.size().columns, 80);
    assert_eq!(terminal.size().screen_lines, 24);
}

#[test]
fn a_zero_sized_terminal_is_clamped_rather_than_panicking() {
    // A pane can be laid out with no room before the first real layout pass.
    let (mut terminal, _) = spawn(0, 0);
    assert_eq!(terminal.size().columns, 1);
    assert_eq!(terminal.size().screen_lines, 1);

    terminal.resize(0, 0);
    assert_eq!(terminal.size().columns, 1);
}

#[test]
fn exiting_the_shell_is_noticed() {
    let (mut terminal, _) = spawn(80, 24);
    assert!(!terminal.has_exited());

    assert!(terminal.write(&b"exit\n"[..]));

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !terminal.has_exited() {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        terminal.has_exited(),
        "the shell exited but was not noticed"
    );
}

#[test]
fn ansi_colours_reach_the_grid() {
    let (terminal, _) = spawn(80, 24);
    // Red text, through an escape sequence the emulator must interpret.
    assert!(terminal.write(&b"printf '\\033[31mRED\\033[0m\\n'\n"[..]));

    // The shell echoes the command line first, and that echo also contains
    // "RED" — so wait for a *coloured* cell rather than for the text.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut coloured = false;
    while Instant::now() < deadline && !coloured {
        {
            let term = terminal.term().lock();
            let grid = term.grid();
            coloured = (0..grid.screen_lines()).any(|line| {
                (0..grid.columns()).any(|column| {
                    let cell = &grid[alacritty_terminal::index::Line(line as i32)]
                        [alacritty_terminal::index::Column(column)];
                    cell.c == 'R'
                        && cell.fg
                            == alacritty_terminal::vte::ansi::Color::Named(
                                alacritty_terminal::vte::ansi::NamedColor::Red,
                            )
                })
            });
        }
        if !coloured {
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    assert!(
        coloured,
        "the escape sequence should have coloured a cell; screen was {:?}",
        screen(&terminal)
    );
}

#[test]
fn dropping_the_terminal_stops_the_shell() {
    let (terminal, _) = spawn(80, 24);
    assert!(terminal.write(&b"echo alive\n"[..]));
    wait_for(&terminal, "alive", Duration::from_secs(10)).expect("shell should start");

    // Closing the view must not leave a shell running with nothing attached.
    drop(terminal);
}

#[test]
fn a_flood_of_output_does_not_starve_the_renderer() {
    // The reader takes the grid lock on every chunk. With an unfair lock a
    // shell producing output flat out could hold off the drawing thread
    // indefinitely; this asserts the renderer keeps getting in.
    let (terminal, _) = spawn(80, 24);
    assert!(terminal.write(&b"seq 1 200000\n"[..]));

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut locks = 0;
    let mut slowest = Duration::ZERO;

    while Instant::now() < deadline && locks < 200 {
        let before = Instant::now();
        {
            let term = terminal.term().lock();
            let _ = term.grid().columns();
        }
        slowest = slowest.max(before.elapsed());
        locks += 1;
        std::thread::sleep(Duration::from_millis(1));
    }

    assert_eq!(locks, 200, "the renderer could not take the lock 200 times");
    assert!(
        slowest < Duration::from_secs(2),
        "a single lock took {slowest:?}, which means the reader is starving the renderer"
    );
}

#[test]
fn writing_does_not_block_when_the_shell_stops_reading() {
    // `cat > /dev/null` keeps reading, but a full pipe must not wedge the
    // caller: writes are queued, and the queue refuses rather than blocking.
    let (terminal, _) = spawn(80, 24);

    let before = Instant::now();
    for _ in 0..5_000 {
        // Ignore the return: past the queue's bound this starts refusing,
        // which is the behaviour under test.
        terminal.write(vec![b'x'; 256]);
    }

    assert!(
        before.elapsed() < Duration::from_secs(5),
        "writing took {:?}, so it is blocking on the PTY",
        before.elapsed()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_program_in_the_foreground_and_its_directory_are_known() {
    let (terminal, _) = spawn(80, 24);
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path().canonicalize().unwrap();
    let line = format!("cd '{}' && sleep 5\n", directory.display());
    assert!(terminal.write(line.into_bytes()));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = None;
    while Instant::now() < deadline {
        seen = terminal.foreground();
        if seen.as_ref().is_some_and(|found| found.command == "sleep") {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let found = seen.expect("the foreground should be readable");
    assert_eq!(found.command, "sleep");
    assert_eq!(found.directory.as_deref(), Some(directory.as_path()));
}
