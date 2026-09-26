//! The integrated terminal's view.
//!
//! The shell itself lives in [`Editor::terminal`], not here, so that hiding
//! this view leaves the session running: closing it with the escape sequence
//! and reopening `:terminal` comes back to the same shell.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor};
use helix_pty::{encode_key, Key, Modifiers};
use helix_view::graphics::{Color, CursorKind, Modifier, Rect, Style, UnderlineStyle};
use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
use helix_view::Editor;
use tui::buffer::Buffer as Surface;

use crate::compositor::{Component, Context, Event, EventResult};

/// The view showing the running shell, under a one-line title bar.
pub struct TerminalView {
    /// Set by `Ctrl-\`, which only means "leave" if `Ctrl-n` follows.
    pending_escape: bool,
    /// Last size the terminal was told about, to avoid resizing every frame.
    size: (u16, u16),
    /// Until when the title bar flashes for the bell.
    bell_until: Option<std::time::Instant>,
}

/// How long the title bar flashes when the bell rings: long enough to see,
/// short enough not to be mistaken for an error.
const BELL_FLASH: std::time::Duration = std::time::Duration::from_millis(200);

impl TerminalView {
    pub const ID: &'static str = "terminal";

    pub fn new() -> Self {
        Self {
            pending_escape: false,
            size: (0, 0),
            bell_until: None,
        }
    }

    /// The bytes a keypress sends to the shell.
    fn encode(editor: &Editor, key: KeyEvent) -> Option<Vec<u8>> {
        let terminal = editor.terminal.as_ref()?;
        // Programs like `vim` and `less` switch DECCKM on and then expect the
        // SS3 form of the arrow keys.
        let application_cursor = terminal.term().lock().mode().contains(TermMode::APP_CURSOR);

        let modifiers = translate_modifiers(key.modifiers);
        // Helix normalises Shift-Tab into Tab with the shift modifier, but a
        // terminal wants its own sequence for it.
        let key_code = match translate_key(key.code)? {
            Key::Tab if modifiers.shift => Key::BackTab,
            key_code => key_code,
        };

        Some(encode_key(key_code, modifiers, application_cursor))
    }
}

impl TerminalView {
    /// Acts on what the emulator kept for the view: text a program copied
    /// goes to the clipboard registers, and the bell starts a flash.
    fn take_notices(&mut self, editor: &mut Editor) {
        let Some(terminal) = editor.terminal.as_ref() else {
            return;
        };
        let copied = terminal.take_copied();
        let bell = terminal.take_bell();
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

        if cx.editor.terminal.is_none() {
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
        let Some(terminal) = cx.editor.terminal.as_mut() else {
            return;
        };
        // Programs asking for the default colours get the theme's.
        terminal.set_default_colors(foreground, background);

        // The title bar: what the program says it is, or what it is.
        let title_area = Rect::new(area.x, area.y, area.width, 1);
        surface.set_style(title_area, bar_style);
        let title = terminal
            .title()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Terminal".to_string());
        let hint = " Ctrl-\\ Ctrl-n: back ";
        let hint_width = hint.chars().count() as u16;
        let title_width = area.width.saturating_sub(hint_width + 1);
        surface.set_stringn(
            area.x,
            area.y,
            &format!(" {title}"),
            title_width as usize,
            bar_style,
        );
        if area.width > hint_width * 2 {
            surface.set_string(area.right() - hint_width, area.y, hint, bar_style);
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

        // The pane's size is only known at render time, so this is where the
        // shell learns about it. `resize` ignores a size it already has.
        if self.size != (area.width, area.height) {
            self.size = (area.width, area.height);
            terminal.resize(area.width, area.height);
        }

        let term = terminal.term().lock();
        let grid = term.grid();
        let rows = grid.screen_lines().min(area.height as usize);
        let columns = grid.columns().min(area.width as usize);

        for row in 0..rows {
            for column in 0..columns {
                let cell = &grid[Line(row as i32)][Column(column)];
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
        let Some(terminal) = editor.terminal.as_ref() else {
            return (None, CursorKind::Hidden);
        };
        let term = terminal.term().lock();
        if !term.mode().contains(TermMode::SHOW_CURSOR) {
            return (None, CursorKind::Hidden);
        }

        let Point { line, column } = term.grid().cursor.point;
        if line.0 < 0 {
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
        let Event::Key(key) = event else {
            // The terminal owns the keyboard; swallow everything else rather
            // than letting a stray event reach the editor underneath.
            return EventResult::Consumed(None);
        };

        let close: crate::compositor::Callback = Box::new(|compositor, _| {
            compositor.remove(TerminalView::ID);
        });

        // `Ctrl-\ Ctrl-n` hands the keyboard back, as it does in Neovim. The
        // first key is held rather than sent, because it only means "leave"
        // if the second one follows.
        if self.pending_escape {
            self.pending_escape = false;
            if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return EventResult::Consumed(Some(close));
            }
            // It was not the escape sequence after all, so the shell gets the
            // Ctrl-\ it should have had, followed by this key.
            if let Some(terminal) = cx.editor.terminal.as_ref() {
                terminal.write(vec![0x1c]);
            }
        } else if is_escape_prefix(*key) {
            self.pending_escape = true;
            return EventResult::Consumed(None);
        }

        // A shell that exited leaves nothing to type into.
        if cx
            .editor
            .terminal
            .as_mut()
            .is_some_and(|terminal| terminal.has_exited())
        {
            cx.editor.terminal = None;
            cx.editor.set_status("Shell exited");
            return EventResult::Consumed(Some(close));
        }

        if let Some(bytes) = Self::encode(cx.editor, *key) {
            if let Some(terminal) = cx.editor.terminal.as_ref() {
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
