//! The integrated terminal's view.
//!
//! The shells themselves live in [`Editor::terminals`], not here, so that
//! hiding this view leaves them running: closing it with the escape sequence
//! and reopening `:terminal` comes back to the same shell. The view shows one
//! terminal at a time, with the others as tabs in its title bar.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use helix_pty::{encode_key_with, Key, KittyModes, Modifiers};
use helix_view::graphics::{Color, CursorKind, Modifier, Rect, Style, UnderlineStyle};
use helix_view::input::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;

use crate::compositor::{Component, Context, Event, EventResult};

/// The view showing the running shell, under a one-line title bar.
pub struct TerminalView {
    /// Set by `Ctrl-\`, which only means "leave" if `Ctrl-n` follows.
    pending_escape: bool,
    /// Set by `Ctrl-\ &`: the next key says whether to close the terminal.
    confirm_close: bool,
    /// Until when the title bar flashes for the bell.
    bell_until: Option<std::time::Instant>,
    /// Set while in copy mode.
    copy: Option<CopyMode>,
    /// Where the grid was last drawn, to place mouse events on it.
    grid_area: Rect,
}

/// Copy mode's own state; the cursor and the selection are the emulator's.
#[derive(Debug, Default)]
struct CopyMode {
    /// A count typed before a motion, as in `5j`.
    count: Option<usize>,
    /// The last search, to repeat with `n` and `N`: pattern and direction.
    search: Option<(String, bool)>,
    /// The match the cursor is on, to highlight.
    found: Option<helix_pty::copy::Match>,
}

/// How long the title bar flashes when the bell rings: long enough to see,
/// short enough not to be mistaken for an error.
const BELL_FLASH: std::time::Duration = std::time::Duration::from_millis(200);

/// Lines a notch of the mouse wheel scrolls.
const WHEEL_LINES: usize = 3;

impl TerminalView {
    pub const ID: &'static str = "terminal";

    pub fn new() -> Self {
        Self {
            pending_escape: false,
            confirm_close: false,
            bell_until: None,
            copy: None,
            grid_area: Rect::default(),
        }
    }

    /// The bytes a keypress sends to the shell.
    fn encode(editor: &Editor, key: KeyEvent) -> Option<Vec<u8>> {
        let terminal = editor.terminals.current()?;
        // Programs like `vim` and `less` switch DECCKM on and then expect the
        // SS3 form of the arrow keys.
        let mode = *terminal.term().lock().mode();
        let application_cursor = mode.contains(TermMode::APP_CURSOR);
        // A program that switched the Kitty keyboard protocol on gets keys
        // like `Ctrl-i` apart from `Tab`.
        let kitty = KittyModes {
            disambiguate: mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
            all_as_escape: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
        };

        let modifiers = translate_modifiers(key.modifiers);
        // Helix normalises Shift-Tab into Tab with the shift modifier, but a
        // terminal wants its own sequence for it.
        let key_code = match translate_key(key.code)? {
            Key::Tab if modifiers.shift => Key::BackTab,
            key_code => key_code,
        };

        Some(encode_key_with(
            key_code,
            modifiers,
            application_cursor,
            kitty,
        ))
    }
}

/// What a terminal is called: the name it was given, or else the title its
/// program set, or else what runs in it.
pub fn label(entry: &helix_view::terminals::Entry<helix_pty::PtyTerminal>) -> String {
    entry
        .name
        .clone()
        .or_else(|| {
            entry
                .terminal
                .title()
                .filter(|title| !title.trim().is_empty())
        })
        .or_else(|| entry.terminal.foreground().map(|found| found.command))
        .unwrap_or_else(|| "Terminal".to_string())
}

/// `text` cut to `width` characters, with an ellipsis when cut.
fn shorten(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut short: String = text.chars().take(width.saturating_sub(1)).collect();
    short.push('…');
    short
}

impl TerminalView {
    /// Shows the terminal numbered `number`, leaving copy mode in the one
    /// shown until now.
    pub fn select(&mut self, editor: &mut Editor, number: usize) {
        if editor
            .terminals
            .current_entry()
            .is_some_and(|entry| entry.number == number)
        {
            return;
        }
        if self.copy.is_some() {
            self.leave_copy_mode(editor);
        }
        self.confirm_close = false;
        if !editor.terminals.select(number) {
            editor.set_error(format!("There is no terminal {number}"));
        }
    }

    /// Shows the next terminal, or the previous one.
    fn cycle(&mut self, editor: &mut Editor, forward: bool) {
        if self.copy.is_some() {
            self.leave_copy_mode(editor);
        }
        editor.terminals.cycle(forward);
    }

    /// Takes the terminal shown out of the list, ending its shell; `true`
    /// when none is left and the view should close.
    fn close_current(&mut self, editor: &mut Editor, why: &str) -> bool {
        self.copy = None;
        self.confirm_close = false;
        let Some(entry) = editor.terminals.remove_current() else {
            return true;
        };
        let number = entry.number;
        drop(entry);
        match editor.terminals.current_entry() {
            Some(next) => {
                editor.set_status(format!(
                    "Terminal {number} {why}; terminal {} is shown",
                    next.number
                ));
                false
            }
            None => {
                editor.set_status(format!("Terminal {number} {why}"));
                true
            }
        }
    }
}

impl TerminalView {
    /// Acts on what the emulator kept for the view: text a program copied
    /// goes to the clipboard registers, and the bell starts a flash.
    fn take_notices(&mut self, editor: &mut Editor) {
        // A terminal not shown whose shell exited leaves the list; the one
        // shown stays until a key is pressed, so its last output can be read.
        let current = editor.terminals.current_entry().map(|entry| entry.number);
        let gone = editor
            .terminals
            .remove_where(|entry| Some(entry.number) != current && entry.terminal.has_exited());
        if !gone.is_empty() {
            let numbers: Vec<String> = gone.iter().map(ToString::to_string).collect();
            editor.set_status(format!("Terminal {} exited", numbers.join(", ")));
        }

        let mut copied = Vec::new();
        let mut bell = false;
        for entry in editor.terminals.entries_mut() {
            copied.extend(entry.terminal.take_copied());
            if entry.terminal.take_bell() {
                if Some(entry.number) == current {
                    bell = true;
                } else {
                    // A terminal not shown is marked in the tabs instead.
                    entry.alert = true;
                }
            }
        }
        for (clipboard, text) in copied {
            let register = match clipboard {
                helix_pty::Clipboard::Clipboard => '+',
                helix_pty::Clipboard::Selection => '*',
            };
            let length = text.chars().count();
            match editor.registers.write(register, vec![text]) {
                Ok(()) => editor.set_status(format!(
                    "Copied {length} character{} from the terminal",
                    if length == 1 { "" } else { "s" }
                )),
                Err(err) => editor.set_error(format!("Could not copy: {err}")),
            }
        }
        if bell {
            self.bell_until = Some(std::time::Instant::now() + BELL_FLASH);
            // One more frame once the flash is over, to draw it away.
            std::thread::spawn(|| {
                std::thread::sleep(BELL_FLASH);
                helix_event::request_redraw();
            });
        }
    }
}

impl TerminalView {
    /// Pastes `text` into the shell, bracketed when the program asked for
    /// it so that a pasted newline does not run anything by itself.
    fn paste(&self, editor: &mut Editor, text: &str) {
        if let Some(terminal) = editor.terminals.current() {
            terminal.paste(text);
        }
    }

    /// Pastes the register's contents, its values one per line.
    fn paste_register(&self, editor: &mut Editor, register: char) {
        let text = match editor.registers.read(register, editor) {
            Some(values) => values.collect::<Vec<_>>().join("\n"),
            None => String::new(),
        };
        if text.is_empty() {
            editor.set_error(format!("Register {register} is empty"));
            return;
        }
        self.paste(editor, &text);
    }

    /// A mouse event: the program's when it asked for them, unless Shift is
    /// held, as terminals let Shift bypass the program; otherwise the wheel
    /// scrolls back.
    fn mouse(&mut self, event: &MouseEvent, editor: &mut Editor) {
        let Some(terminal) = editor.terminals.current() else {
            return;
        };
        let convert_button = |button| match button {
            helix_view::input::MouseButton::Left => helix_pty::MouseButton::Left,
            helix_view::input::MouseButton::Middle => helix_pty::MouseButton::Middle,
            helix_view::input::MouseButton::Right => helix_pty::MouseButton::Right,
        };
        let action = match event.kind {
            MouseEventKind::Down(button) => helix_pty::MouseAction::Press(convert_button(button)),
            MouseEventKind::Up(button) => helix_pty::MouseAction::Release(convert_button(button)),
            MouseEventKind::Drag(button) => helix_pty::MouseAction::Drag(convert_button(button)),
            MouseEventKind::Moved => helix_pty::MouseAction::Move,
            MouseEventKind::ScrollUp => helix_pty::MouseAction::WheelUp,
            MouseEventKind::ScrollDown => helix_pty::MouseAction::WheelDown,
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight => return,
        };
        let mut term = terminal.term().lock();
        let mode = *term.mode();
        let modes = helix_pty::MouseModes {
            click: mode.contains(TermMode::MOUSE_REPORT_CLICK),
            drag: mode.contains(TermMode::MOUSE_DRAG),
            motion: mode.contains(TermMode::MOUSE_MOTION),
            sgr: mode.contains(TermMode::SGR_MOUSE),
            utf8: mode.contains(TermMode::UTF8_MOUSE),
        };
        let area = self.grid_area;
        let inside = event.column >= area.x
            && event.column < area.right()
            && event.row >= area.y
            && event.row < area.bottom();
        if self.copy.is_none()
            && modes.any()
            && inside
            && !event.modifiers.contains(KeyModifiers::SHIFT)
        {
            let report = helix_pty::MouseReport {
                action,
                column: (event.column - area.x) as usize,
                line: (event.row - area.y) as usize,
                modifiers: translate_modifiers(event.modifiers),
            };
            if let Some(bytes) = helix_pty::encode_mouse(report, modes) {
                term.scroll_display(Scroll::Bottom);
                drop(term);
                terminal.write(bytes);
            }
            return;
        }

        let up = match action {
            helix_pty::MouseAction::WheelUp => true,
            helix_pty::MouseAction::WheelDown => false,
            _ => return,
        };
        if self.copy.is_some() {
            let motion = if up {
                helix_pty::copy::Motion::Up
            } else {
                helix_pty::copy::Motion::Down
            };
            helix_pty::copy::motion(&mut term, motion, WHEEL_LINES);
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            // A full-screen program like `less` has no scrollback; the wheel
            // moves it with the arrow keys instead, as other terminals do.
            drop(term);
            let key = if up { Key::Up } else { Key::Down };
            let application_cursor = mode.contains(TermMode::APP_CURSOR);
            for _ in 0..WHEEL_LINES {
                terminal.write(helix_pty::encode_key(
                    key,
                    Modifiers::default(),
                    application_cursor,
                ));
            }
        } else {
            let lines = WHEEL_LINES as i32;
            term.scroll_display(Scroll::Delta(if up { lines } else { -lines }));
        }
    }

    fn enter_copy_mode(&mut self, editor: &mut Editor) {
        if let Some(terminal) = editor.terminals.current() {
            helix_pty::copy::enter(&mut terminal.term().lock());
            self.copy = Some(CopyMode::default());
        }
    }

    fn leave_copy_mode(&mut self, editor: &mut Editor) {
        if let Some(terminal) = editor.terminals.current() {
            helix_pty::copy::leave(&mut terminal.term().lock());
        }
        self.copy = None;
    }

    /// Searches from the copy-mode cursor, remembering the search for `n`
    /// and `N`.
    pub fn search(&mut self, editor: &mut Editor, pattern: &str, forward: bool) {
        let Some(terminal) = editor.terminals.current() else {
            return;
        };
        let Some(copy) = self.copy.as_mut() else {
            return;
        };
        let result = helix_pty::copy::search(&mut terminal.term().lock(), pattern, forward);
        copy.search = Some((pattern.to_string(), forward));
        match result {
            Ok(found) => {
                if found.is_none() {
                    editor.set_error(format!("No match for {pattern}"));
                }
                copy.found = found;
            }
            Err(err) => {
                copy.found = None;
                editor.set_error(format!("Invalid pattern: {err}"));
            }
        }
    }

    fn copy_mode_key(&mut self, key: KeyEvent, cx: &mut Context) -> EventResult {
        use helix_pty::copy::{self, Motion, Selecting};

        let Some(terminal) = cx.editor.terminals.current() else {
            return EventResult::Consumed(None);
        };
        let Some(state) = self.copy.as_mut() else {
            return EventResult::Consumed(None);
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        // A count: digits, where a leading 0 is the start of the line.
        if let KeyCode::Char(digit @ '0'..='9') = key.code {
            if !ctrl && (digit != '0' || state.count.is_some()) {
                let digit = digit.to_digit(10).unwrap_or(0) as usize;
                state.count = Some(state.count.unwrap_or(0) * 10 + digit);
                return EventResult::Consumed(None);
            }
        }
        let count = state.count.take().unwrap_or(1);
        let half_page = (terminal.size().screen_lines / 2).max(1);
        let page = terminal.size().screen_lines.max(1);
        let motion = match (key.code, ctrl) {
            (KeyCode::Char('h') | KeyCode::Left, false) => Some((Motion::Left, count)),
            (KeyCode::Char('j') | KeyCode::Down, false) => Some((Motion::Down, count)),
            (KeyCode::Char('k') | KeyCode::Up, false) => Some((Motion::Up, count)),
            (KeyCode::Char('l') | KeyCode::Right, false) => Some((Motion::Right, count)),
            (KeyCode::Char('w'), false) => Some((Motion::SemanticRight, count)),
            (KeyCode::Char('b'), false) => Some((Motion::SemanticLeft, count)),
            (KeyCode::Char('e'), false) => Some((Motion::SemanticRightEnd, count)),
            (KeyCode::Char('W'), false) => Some((Motion::WordRight, count)),
            (KeyCode::Char('B'), false) => Some((Motion::WordLeft, count)),
            (KeyCode::Char('E'), false) => Some((Motion::WordRightEnd, count)),
            (KeyCode::Char('0') | KeyCode::Home, false) => Some((Motion::First, 1)),
            (KeyCode::Char('^'), false) => Some((Motion::FirstOccupied, 1)),
            (KeyCode::Char('$') | KeyCode::End, false) => Some((Motion::Last, 1)),
            (KeyCode::Char('H'), false) => Some((Motion::High, 1)),
            (KeyCode::Char('M'), false) => Some((Motion::Middle, 1)),
            (KeyCode::Char('L'), false) => Some((Motion::Low, 1)),
            (KeyCode::Char('%'), false) => Some((Motion::Bracket, 1)),
            (KeyCode::Char('{'), false) => Some((Motion::ParagraphUp, count)),
            (KeyCode::Char('}'), false) => Some((Motion::ParagraphDown, count)),
            (KeyCode::Char('u'), true) => Some((Motion::Up, half_page * count)),
            (KeyCode::Char('d'), true) => Some((Motion::Down, half_page * count)),
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), true) => Some((Motion::Up, page * count)),
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), true) => {
                Some((Motion::Down, page * count))
            }
            _ => None,
        };
        let mut term = terminal.term().lock();
        if let Some((motion, times)) = motion {
            copy::motion(&mut term, motion, times);
            state.found = None;
            return EventResult::Consumed(None);
        }
        match (key.code, ctrl) {
            (KeyCode::Char('g'), false) => copy::to_edge(&mut term, true),
            (KeyCode::Char('G'), false) => copy::to_edge(&mut term, false),
            (KeyCode::Char('v'), false) => copy::toggle_selection(&mut term, Selecting::Characters),
            (KeyCode::Char('V') | KeyCode::Char('x'), false) => {
                copy::toggle_selection(&mut term, Selecting::Lines)
            }
            (KeyCode::Char('v'), true) => copy::toggle_selection(&mut term, Selecting::Block),
            (KeyCode::Char('y'), false) => {
                let Some(text) = copy::selected_text(&term) else {
                    drop(term);
                    cx.editor
                        .set_error("Nothing selected: v, V or Ctrl-v selects");
                    return EventResult::Consumed(None);
                };
                drop(term);
                let register = cx.editor.config().default_yank_register;
                let lines = text.lines().count().max(1);
                match cx.editor.registers.write(register, vec![text]) {
                    Ok(()) => cx.editor.set_status(format!(
                        "Copied {lines} line{} into register {register}",
                        if lines == 1 { "" } else { "s" }
                    )),
                    Err(err) => cx.editor.set_error(err.to_string()),
                }
                self.leave_copy_mode(cx.editor);
            }
            (KeyCode::Char(slash @ ('/' | '?')), false) => {
                drop(term);
                let forward = slash == '/';
                let prompt = crate::ui::Prompt::new(
                    if forward {
                        "search: ".into()
                    } else {
                        "reverse search: ".into()
                    },
                    None,
                    |_, _| Vec::new(),
                    move |cx, input, event| {
                        if event != crate::ui::PromptEvent::Validate || input.is_empty() {
                            return;
                        }
                        let pattern = input.to_string();
                        cx.jobs.callback(async move {
                            Ok(crate::job::Callback::EditorCompositor(Box::new(
                                move |editor: &mut Editor, compositor: &mut crate::compositor::Compositor| {
                                    if let Some(view) =
                                        compositor.find_id::<TerminalView>(TerminalView::ID)
                                    {
                                        view.search(editor, &pattern, forward);
                                    }
                                },
                            )))
                        });
                    },
                );
                return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                    compositor.push(Box::new(prompt));
                })));
            }
            (KeyCode::Char(next @ ('n' | 'N')), false) => {
                drop(term);
                match state.search.clone() {
                    Some((pattern, forward)) => {
                        let forward = if next == 'n' { forward } else { !forward };
                        let remembered = state.search.clone();
                        self.search(cx.editor, &pattern, forward);
                        // `N` searches the other way without changing `n`.
                        if let Some(copy) = self.copy.as_mut() {
                            copy.search = remembered;
                        }
                    }
                    None => cx.editor.set_error("No search yet: / searches"),
                }
            }
            (KeyCode::Esc, _) | (KeyCode::Char('q'), false) | (KeyCode::Char('c'), true) => {
                // Esc drops a selection first; with none, it leaves.
                if term.selection.is_some() && key.code == KeyCode::Esc {
                    term.selection = None;
                } else {
                    drop(term);
                    self.leave_copy_mode(cx.editor);
                }
            }
            _ => {}
        }
        EventResult::Consumed(None)
    }
}

impl Default for TerminalView {
    fn default() -> Self {
        Self::new()
    }
}

/// Maps Helix's key codes onto the subset a terminal can express.
fn translate_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Tab => Key::Tab,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Escape,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::F(n) => Key::Function(n),
        // Media and modifier-only keys have no terminal encoding.
        _ => return None,
    })
}

/// Whether this is the first key of the `Ctrl-\ Ctrl-n` escape sequence.
///
/// A terminal sends `0x1c` for both `Ctrl-\` and `Ctrl-4`, and the decoder
/// reports the digit form, so both are accepted — no shell could tell them
/// apart either.
fn is_escape_prefix(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('\\') | KeyCode::Char('4'))
}

fn translate_modifiers(modifiers: KeyModifiers) -> Modifiers {
    Modifiers {
        ctrl: modifiers.contains(KeyModifiers::CONTROL),
        alt: modifiers.contains(KeyModifiers::ALT),
        shift: modifiers.contains(KeyModifiers::SHIFT),
    }
}

/// Converts one of Alacritty's colours into Helix's.
///
/// Named colours stay named so the user's own palette applies, rather than
/// being flattened into fixed RGB.
fn convert_color(color: AnsiColor, foreground: bool) -> Color {
    match color {
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
        AnsiColor::Indexed(index) => match index {
            0..=15 => named_color(index),
            _ => Color::Indexed(index),
        },
        AnsiColor::Named(named) => match named {
            NamedColor::Black => Color::Black,
            NamedColor::Red => Color::Red,
            NamedColor::Green => Color::Green,
            NamedColor::Yellow => Color::Yellow,
            NamedColor::Blue => Color::Blue,
            NamedColor::Magenta => Color::Magenta,
            NamedColor::Cyan => Color::Cyan,
            NamedColor::White => Color::Gray,
            NamedColor::BrightBlack => Color::LightGray,
            NamedColor::BrightRed => Color::LightRed,
            NamedColor::BrightGreen => Color::LightGreen,
            NamedColor::BrightYellow => Color::LightYellow,
            NamedColor::BrightBlue => Color::LightBlue,
            NamedColor::BrightMagenta => Color::LightMagenta,
            NamedColor::BrightCyan => Color::LightCyan,
            NamedColor::BrightWhite => Color::White,
            // The defaults fall through to the editor's own background and
            // foreground, so the terminal blends with the theme.
            NamedColor::Foreground if foreground => Color::Reset,
            NamedColor::Background if !foreground => Color::Reset,
            NamedColor::DimForeground | NamedColor::Foreground => Color::Reset,
            NamedColor::DimBlack => Color::Black,
            NamedColor::DimRed => Color::Red,
            NamedColor::DimGreen => Color::Green,
            NamedColor::DimYellow => Color::Yellow,
            NamedColor::DimBlue => Color::Blue,
            NamedColor::DimMagenta => Color::Magenta,
            NamedColor::DimCyan => Color::Cyan,
            NamedColor::DimWhite => Color::Gray,
            NamedColor::BrightForeground => Color::White,
            NamedColor::Cursor | NamedColor::Background => Color::Reset,
        },
    }
}

fn named_color(index: u8) -> Color {
    match index {
        0 => Color::Black,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::Gray,
        8 => Color::LightGray,
        9 => Color::LightRed,
        10 => Color::LightGreen,
        11 => Color::LightYellow,
        12 => Color::LightBlue,
        13 => Color::LightMagenta,
        14 => Color::LightCyan,
        _ => Color::White,
    }
}

/// The underline a cell asks for, if any.
fn convert_underline(flags: Flags) -> Option<UnderlineStyle> {
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Some(UnderlineStyle::DoubleLine)
    } else if flags.contains(Flags::UNDERCURL) {
        Some(UnderlineStyle::Curl)
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Some(UnderlineStyle::Dotted)
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Some(UnderlineStyle::Dashed)
    } else if flags.contains(Flags::UNDERLINE) {
        Some(UnderlineStyle::Line)
    } else {
        None
    }
}

/// Converts a cell's attributes into Helix's modifiers.
fn convert_flags(flags: Flags) -> Modifier {
    let mut modifier = Modifier::empty();
    if flags.contains(Flags::BOLD) {
        modifier |= Modifier::BOLD;
    }
    if flags.contains(Flags::DIM) {
        modifier |= Modifier::DIM;
    }
    if flags.contains(Flags::ITALIC) {
        modifier |= Modifier::ITALIC;
    }
    if flags.contains(Flags::INVERSE) {
        modifier |= Modifier::REVERSED;
    }
    if flags.contains(Flags::HIDDEN) {
        modifier |= Modifier::HIDDEN;
    }
    if flags.contains(Flags::STRIKEOUT) {
        modifier |= Modifier::CROSSED_OUT;
    }
    modifier
}

impl Component for TerminalView {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        // Leave the statusline alone so the editor's mode stays visible.
        let area = viewport.intersection(Rect::new(
            0,
            0,
            viewport.width,
            viewport.height.saturating_sub(1),
        ));
        if area.width == 0 || area.height == 0 {
            return;
        }

        if cx.editor.terminals.is_empty() {
            return;
        }
        self.take_notices(cx.editor);
        let theme = &cx.editor.theme;
        let flashing = self
            .bell_until
            .is_some_and(|until| std::time::Instant::now() < until);
        let bar_style = if flashing {
            theme.get("warning").add_modifier(Modifier::REVERSED)
        } else {
            theme.get("ui.statusline")
        };
        let rgb = |color: Option<Color>| match color {
            Some(Color::Rgb(r, g, b)) => Some((r, g, b)),
            _ => None,
        };
        let (foreground, background) = (
            rgb(theme.get("ui.text").fg),
            rgb(theme.get("ui.background").bg),
        );
        // The tabs: each terminal's number and label, the one shown in bold,
        // and `!` on one whose bell rang while it was not shown.
        let many = cx.editor.terminals.len() > 1;
        let current = cx
            .editor
            .terminals
            .current_entry()
            .map(|entry| entry.number);
        let tabs: Vec<(String, bool)> = cx
            .editor
            .terminals
            .entries()
            .iter()
            .map(|entry| {
                let shown = Some(entry.number) == current;
                let label = label(entry);
                let text = match (many, shown) {
                    (false, _) => label,
                    (true, true) => format!("{} {}", entry.number, shorten(&label, 32)),
                    (true, false) => format!(
                        "{}{} {}",
                        entry.number,
                        if entry.alert { "!" } else { "" },
                        shorten(&label, 12)
                    ),
                };
                (text, shown)
            })
            .collect();
        let Some(terminal) = cx.editor.terminals.current_mut() else {
            return;
        };
        // Programs asking for the default colours get the theme's.
        terminal.set_default_colors(foreground, background);

        // The title bar: the terminals, each by what the program in it says
        // it is, or by what it is.
        let title_area = Rect::new(area.x, area.y, area.width, 1);
        // Cleared, so that nothing of the editor underneath shows through.
        surface.clear_with(title_area, bar_style);
        let close_question;
        let (prefix, hint) = match &self.copy {
            _ if self.confirm_close => {
                close_question = format!(
                    " Close terminal {} and what runs in it? y/n ",
                    current.unwrap_or_default()
                );
                (String::new(), close_question.as_str())
            }
            Some(copy) => (
                match &copy.search {
                    Some((pattern, _)) => format!("[copy] /{pattern}  "),
                    None => "[copy] ".to_string(),
                },
                " v select  y copy  / search  q done ",
            ),
            None => (
                String::new(),
                " Ctrl-\\ Ctrl-n: back  [: copy  p: paste  c: new  w: list ",
            ),
        };
        let hint_width = hint.chars().count() as u16;
        let title_end = area.right().saturating_sub(hint_width + 1);
        let (mut x, _) = surface.set_stringn(
            area.x,
            area.y,
            &format!(" {prefix}"),
            title_end.saturating_sub(area.x) as usize,
            bar_style,
        );
        for (index, (text, shown)) in tabs.iter().enumerate() {
            if index > 0 && x < title_end {
                x = surface
                    .set_stringn(x, area.y, " │ ", (title_end - x) as usize, bar_style)
                    .0;
            }
            let style = if *shown && many {
                bar_style.add_modifier(Modifier::BOLD)
            } else {
                bar_style
            };
            if x < title_end {
                x = surface
                    .set_stringn(x, area.y, text, (title_end - x) as usize, style)
                    .0;
            }
        }
        if area.width > hint_width * 2 || self.confirm_close {
            let hint_x = area.right().saturating_sub(hint_width).max(area.x);
            surface.set_stringn(hint_x, area.y, hint, area.width as usize, bar_style);
        }
        let area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        if area.height == 0 {
            return;
        }
        self.grid_area = area;

        // The pane's size is only known at render time, so this is where the
        // shell learns about it. `resize` ignores a size it already has.
        terminal.resize(area.width, area.height);

        let term = terminal.term().lock();
        let grid = term.grid();
        let rows = grid.screen_lines().min(area.height as usize);
        let columns = grid.columns().min(area.width as usize);
        // Scrolled back, the screen shows lines above the live ones.
        let offset = grid.display_offset() as i32;
        let selection = term
            .selection
            .as_ref()
            .and_then(|selection| selection.to_range(&*term));
        let found = self.copy.as_ref().and_then(|copy| copy.found.clone());
        let selected_style = cx.editor.theme.get("ui.selection");
        let found_style = cx.editor.theme.get("ui.selection.primary");

        for row in 0..rows {
            for column in 0..columns {
                let point = Point::new(Line(row as i32 - offset), Column(column));
                let cell = &grid[point.line][point.column];
                // The second half of a wide character is not drawn; the
                // character itself already occupies both columns.
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }

                let mut style = Style::default()
                    .fg(convert_color(cell.fg, true))
                    .bg(convert_color(cell.bg, false))
                    .add_modifier(convert_flags(cell.flags));
                // Helix carries underlines as a style of their own rather
                // than as a modifier bit.
                if let Some(underline) = convert_underline(cell.flags) {
                    style = style.underline_style(underline);
                }
                if found.as_ref().is_some_and(|found| found.contains(&point)) {
                    style = style.patch(found_style);
                } else if selection
                    .as_ref()
                    .is_some_and(|range| range.contains(point))
                {
                    style = style.patch(selected_style);
                }

                surface.set_string(
                    area.x + column as u16,
                    area.y + row as u16,
                    &cell.c.to_string(),
                    style,
                );
            }
        }
    }

    fn cursor(
        &self,
        viewport: Rect,
        editor: &Editor,
    ) -> (Option<helix_core::Position>, CursorKind) {
        let Some(terminal) = editor.terminals.current() else {
            return (None, CursorKind::Hidden);
        };
        let term = terminal.term().lock();
        // In copy mode, the copy-mode cursor; otherwise the program's, when
        // it shows one.
        let copying = helix_pty::copy::is_active(&term);
        if !copying && !term.mode().contains(TermMode::SHOW_CURSOR) {
            return (None, CursorKind::Hidden);
        }
        let point = if copying {
            term.vi_mode_cursor.point
        } else {
            term.grid().cursor.point
        };
        let offset = term.grid().display_offset() as i32;
        let (line, column) = (point.line + offset, point.column);
        if line.0 < 0 || line.0 >= term.grid().screen_lines() as i32 {
            return (None, CursorKind::Hidden);
        }

        // Below the title bar.
        (
            Some(helix_core::Position::new(
                viewport.y as usize + 1 + line.0 as usize,
                viewport.x as usize + column.0,
            )),
            CursorKind::Block,
        )
    }

    fn handle_event(&mut self, event: &Event, cx: &mut Context) -> EventResult {
        let key = match event {
            Event::Key(key) => key,
            // The terminal's paste: bracketed when the program asked.
            Event::Paste(text) if self.copy.is_none() => {
                self.paste(cx.editor, text);
                return EventResult::Consumed(None);
            }
            Event::Mouse(mouse) => {
                self.mouse(mouse, cx.editor);
                return EventResult::Consumed(None);
            }
            // The terminal owns the keyboard; swallow everything else rather
            // than letting a stray event reach the editor underneath.
            _ => return EventResult::Consumed(None),
        };

        let close: crate::compositor::Callback = Box::new(|compositor, _| {
            compositor.remove(TerminalView::ID);
        });
        if cx.editor.terminals.is_empty() {
            return EventResult::Consumed(Some(close));
        }

        // `Ctrl-\ &` asked whether to close the terminal: `y` does.
        if self.confirm_close {
            self.confirm_close = false;
            if key.code == KeyCode::Char('y') && key.modifiers.is_empty() {
                let last = self.close_current(cx.editor, "closed");
                return EventResult::Consumed(last.then_some(close));
            }
            cx.editor.set_status("The terminal stays open");
            return EventResult::Consumed(None);
        }

        // `Ctrl-\ Ctrl-n` hands the keyboard back, as it does in Neovim. The
        // first key is held rather than sent, because it only means "leave"
        // if the second one follows.
        if self.pending_escape {
            self.pending_escape = false;
            if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return EventResult::Consumed(Some(close));
            }
            // `Ctrl-\ [`: copy mode, as tmux's prefix and `[`.
            if key.code == KeyCode::Char('[') && !key.modifiers.contains(KeyModifiers::CONTROL) {
                self.enter_copy_mode(cx.editor);
                return EventResult::Consumed(None);
            }
            // `Ctrl-\ p` pastes the default register, `Ctrl-\ P` the
            // clipboard.
            if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
                let register = match key.code {
                    KeyCode::Char('p') => Some(cx.editor.config().default_yank_register),
                    KeyCode::Char('P') => Some('+'),
                    _ => None,
                };
                if let Some(register) = register {
                    self.paste_register(cx.editor, register);
                    return EventResult::Consumed(None);
                }
                // The terminals, with tmux's keys for its windows.
                match key.code {
                    // `c`: another terminal.
                    KeyCode::Char('c') => {
                        if self.copy.is_some() {
                            self.leave_copy_mode(cx.editor);
                        }
                        if let Some(number) = crate::commands::spawn_terminal(cx.editor, None) {
                            cx.editor.set_status(format!("Terminal {number}"));
                        }
                        return EventResult::Consumed(None);
                    }
                    // `w`: the list of them.
                    KeyCode::Char('w') => {
                        let Some(picker) = crate::commands::terminal_picker(cx.editor) else {
                            return EventResult::Consumed(None);
                        };
                        return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                            compositor.push(picker);
                        })));
                    }
                    // `1` to `9`: that one.
                    KeyCode::Char(digit @ '1'..='9') => {
                        let number = digit.to_digit(10).unwrap_or(1) as usize;
                        self.select(cx.editor, number);
                        return EventResult::Consumed(None);
                    }
                    // `)` and `(`: the next one and the previous one.
                    KeyCode::Char(bracket @ (')' | '(')) => {
                        self.cycle(cx.editor, bracket == ')');
                        return EventResult::Consumed(None);
                    }
                    // `&`: close this one, once confirmed.
                    KeyCode::Char('&') => {
                        self.confirm_close = true;
                        return EventResult::Consumed(None);
                    }
                    // `,`: name it.
                    KeyCode::Char(',') => {
                        let prompt = crate::ui::Prompt::new(
                            "terminal name: ".into(),
                            None,
                            |_, _| Vec::new(),
                            |cx, input, event| {
                                if event != crate::ui::PromptEvent::Validate {
                                    return;
                                }
                                if let Some(entry) = cx.editor.terminals.current_entry_mut() {
                                    let name = input.trim();
                                    entry.name = (!name.is_empty()).then(|| name.to_string());
                                }
                            },
                        );
                        return EventResult::Consumed(Some(Box::new(move |compositor, _| {
                            compositor.push(Box::new(prompt));
                        })));
                    }
                    _ => {}
                }
            }
            // It was not the escape sequence after all, so the shell gets the
            // Ctrl-\ it should have had, followed by this key.
            if let Some(terminal) = cx.editor.terminals.current() {
                terminal.write(vec![0x1c]);
            }
        } else if is_escape_prefix(*key) {
            self.pending_escape = true;
            return EventResult::Consumed(None);
        } else if self.copy.is_some() {
            return self.copy_mode_key(*key, cx);
        }

        // Shift-PageUp and Shift-PageDown scroll back without copy mode, as
        // in most terminals.
        if key.modifiers.contains(KeyModifiers::SHIFT)
            && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            if let Some(terminal) = cx.editor.terminals.current() {
                terminal
                    .term()
                    .lock()
                    .scroll_display(if key.code == KeyCode::PageUp {
                        Scroll::PageUp
                    } else {
                        Scroll::PageDown
                    });
            }
            return EventResult::Consumed(None);
        }

        // A shell that exited leaves nothing to type into: the next
        // terminal is shown, or the editor when it was the last.
        if cx
            .editor
            .terminals
            .current_mut()
            .is_some_and(|terminal| terminal.has_exited())
        {
            let last = self.close_current(cx.editor, "exited");
            return EventResult::Consumed(last.then_some(close));
        }

        if let Some(bytes) = Self::encode(cx.editor, *key) {
            if let Some(terminal) = cx.editor.terminals.current() {
                // Typing is about what is happening now: back to the bottom.
                terminal.term().lock().scroll_display(Scroll::Bottom);
                terminal.write(bytes);
            }
        }

        EventResult::Consumed(None)
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::Rgb;

    #[test]
    fn named_colours_stay_named_so_the_theme_applies() {
        assert_eq!(
            convert_color(AnsiColor::Named(NamedColor::Red), true),
            Color::Red
        );
        assert_eq!(
            convert_color(AnsiColor::Named(NamedColor::BrightBlue), true),
            Color::LightBlue
        );
        // Alacritty's "White" is the dim one; bright white is separate.
        assert_eq!(
            convert_color(AnsiColor::Named(NamedColor::White), true),
            Color::Gray
        );
        assert_eq!(
            convert_color(AnsiColor::Named(NamedColor::BrightWhite), true),
            Color::White
        );
    }

    #[test]
    fn the_default_colours_fall_through_to_the_theme() {
        // Unset foreground and background must not be painted, or the
        // terminal would not blend with the editor.
        assert_eq!(
            convert_color(AnsiColor::Named(NamedColor::Foreground), true),
            Color::Reset
        );
        assert_eq!(
            convert_color(AnsiColor::Named(NamedColor::Background), false),
            Color::Reset
        );
    }

    #[test]
    fn true_colour_and_palette_indices_are_carried_through() {
        assert_eq!(
            convert_color(AnsiColor::Spec(Rgb { r: 1, g: 2, b: 3 }), true),
            Color::Rgb(1, 2, 3)
        );
        // The first sixteen indices are the named colours, so they keep their
        // names rather than becoming fixed palette entries.
        assert_eq!(convert_color(AnsiColor::Indexed(1), true), Color::Red);
        assert_eq!(convert_color(AnsiColor::Indexed(9), true), Color::LightRed);
        assert_eq!(
            convert_color(AnsiColor::Indexed(200), true),
            Color::Indexed(200)
        );
    }

    #[test]
    fn cell_attributes_become_modifiers() {
        assert_eq!(convert_flags(Flags::empty()), Modifier::empty());
        assert!(convert_flags(Flags::BOLD).contains(Modifier::BOLD));
        assert!(convert_flags(Flags::ITALIC).contains(Modifier::ITALIC));
        assert!(convert_flags(Flags::INVERSE).contains(Modifier::REVERSED));
        assert!(convert_flags(Flags::STRIKEOUT).contains(Modifier::CROSSED_OUT));

        let both = convert_flags(Flags::BOLD | Flags::DIM);
        assert!(both.contains(Modifier::BOLD) && both.contains(Modifier::DIM));
    }

    #[test]
    fn underlines_map_onto_helix_underline_styles() {
        assert_eq!(convert_underline(Flags::empty()), None);
        assert_eq!(
            convert_underline(Flags::UNDERLINE),
            Some(UnderlineStyle::Line)
        );
        assert_eq!(
            convert_underline(Flags::UNDERCURL),
            Some(UnderlineStyle::Curl)
        );
        assert_eq!(
            convert_underline(Flags::DOUBLE_UNDERLINE),
            Some(UnderlineStyle::DoubleLine)
        );
        // Underline is not a modifier bit in Helix, so it must not leak into
        // one.
        assert!(!convert_flags(Flags::UNDERLINE).contains(Modifier::BOLD));
    }

    #[test]
    fn helix_key_codes_map_onto_terminal_keys() {
        assert_eq!(translate_key(KeyCode::Char('x')), Some(Key::Char('x')));
        assert_eq!(translate_key(KeyCode::Enter), Some(Key::Enter));
        assert_eq!(translate_key(KeyCode::Up), Some(Key::Up));
        assert_eq!(translate_key(KeyCode::F(5)), Some(Key::Function(5)));
        assert_eq!(translate_key(KeyCode::PageDown), Some(Key::PageDown));
        // A key with no terminal encoding sends nothing at all.
        assert_eq!(translate_key(KeyCode::CapsLock), None);
    }

    #[test]
    fn both_spellings_of_the_escape_prefix_are_accepted() {
        // A terminal sends 0x1c for Ctrl-\ and Ctrl-4 alike.
        for code in [KeyCode::Char('\\'), KeyCode::Char('4')] {
            assert!(is_escape_prefix(KeyEvent {
                code,
                modifiers: KeyModifiers::CONTROL,
            }));
        }

        // Without Ctrl, or with any other key, it is ordinary input.
        assert!(!is_escape_prefix(KeyEvent {
            code: KeyCode::Char('4'),
            modifiers: KeyModifiers::NONE,
        }));
        assert!(!is_escape_prefix(KeyEvent {
            code: KeyCode::Char('n'),
            modifiers: KeyModifiers::CONTROL,
        }));
    }

    #[test]
    fn modifiers_carry_across() {
        let modifiers = translate_modifiers(KeyModifiers::CONTROL | KeyModifiers::ALT);
        assert!(modifiers.ctrl && modifiers.alt && !modifiers.shift);
        assert_eq!(translate_modifiers(KeyModifiers::NONE), Modifiers::NONE);
    }
}
