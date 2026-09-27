//! What the menus remember: Magit's transient values, history and levels.
//!
//! A menu's arguments can be saved as its defaults, which later sessions
//! start from; the values typed into its options are kept, newest first,
//! to be recalled; and the actions hidden from a menu stay hidden. All of
//! it is one small text file, a line per entry, its fields separated by
//! tabs (shown here as `→`):
//!
//! ```text
//! value→Commit→--signoff→--gpg-sign=ABC
//! history→--author=→ann→bob
//! hidden→Log→s→W
//! ```
//!
//! A tab, a newline or a backslash inside a value is written `\t`, `\n` or
//! `\\`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::transient::{MenuKind, TransientArgument, TransientMenu};

/// How many values an option's history keeps.
pub const HISTORY_LENGTH: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TransientState {
    /// A menu's saved arguments, by menu.
    pub values: BTreeMap<String, Vec<String>>,
    /// The values typed into an option, newest first, by the option's flag.
    pub history: BTreeMap<String, Vec<String>>,
    /// The keys of the actions hidden from a menu.
    pub hidden: BTreeMap<String, BTreeSet<char>>,
}

/// The name a menu is saved under.
pub fn menu_name(kind: MenuKind) -> String {
    format!("{kind:?}")
}

fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

impl TransientState {
    /// Reads the file's text; lines it does not understand are skipped, so
    /// a newer or damaged file costs its unknown lines only.
    pub fn parse(text: &str) -> Self {
        let mut state = Self::default();
        for line in text.lines() {
            let mut fields = line.split('\t');
            let (Some(kind), Some(name)) = (fields.next(), fields.next()) else {
                continue;
            };
            let name = unescape(name);
            let rest: Vec<String> = fields.map(unescape).collect();
            match kind {
                "value" => {
                    state.values.insert(name, rest);
                }
                "history" => {
                    state.history.insert(name, rest);
                }
                "hidden" => {
                    let keys = rest.iter().filter_map(|key| key.chars().next()).collect();
                    state.hidden.insert(name, keys);
                }
                _ => {}
            }
        }
        state
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut line = |kind: &str, name: &str, rest: &mut dyn Iterator<Item = String>| {
            out.push_str(kind);
            out.push('\t');
            out.push_str(&escape(name));
            for value in rest {
                out.push('\t');
                out.push_str(&escape(&value));
            }
            out.push('\n');
        };
        for (menu, args) in &self.values {
            line("value", menu, &mut args.iter().cloned());
        }
        for (flag, values) in &self.history {
            line("history", flag, &mut values.iter().cloned());
        }
        for (menu, keys) in &self.hidden {
            if !keys.is_empty() {
                line("hidden", menu, &mut keys.iter().map(char::to_string));
            }
        }
        out
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, self.render())
    }

    /// Keeps `value` as the newest of the option `flag`'s history.
    pub fn remember(&mut self, flag: &str, value: &str) {
        let history = self.history.entry(flag.to_string()).or_default();
        history.retain(|old| old != value);
        history.insert(0, value.to_string());
        history.truncate(HISTORY_LENGTH);
    }

    /// Hides the action `key` of a menu, or shows it again; returns
    /// whether it is hidden now.
    pub fn toggle_hidden(&mut self, kind: MenuKind, key: char) -> bool {
        let keys = self.hidden.entry(menu_name(kind)).or_default();
        if keys.remove(&key) {
            false
        } else {
            keys.insert(key);
            true
        }
    }

    pub fn hidden_in(&self, kind: MenuKind) -> BTreeSet<char> {
        self.hidden
            .get(&menu_name(kind))
            .cloned()
            .unwrap_or_default()
    }
}

impl TransientMenu {
    /// Sets every argument from `args`, as [`TransientMenu::args`] gives
    /// them: a switch is on when its flag is there, an option has the
    /// value that follows its flag. Arguments not there are turned off.
    pub fn set_args(&mut self, args: &[String]) {
        for argument in self
            .groups
            .iter_mut()
            .flat_map(|group| &mut group.arguments)
        {
            match argument {
                TransientArgument::Switch(switch) => {
                    switch.enabled = args.contains(&switch.flag);
                }
                TransientArgument::Option(option) => {
                    option.value = args
                        .iter()
                        .find_map(|arg| arg.strip_prefix(option.flag.as_str()))
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                }
            }
        }
    }

    /// Whether the menu has arguments to save at all.
    pub fn has_arguments(&self) -> bool {
        self.groups.iter().any(|group| !group.arguments.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transient::{commit_menu, log_menu};

    #[test]
    fn the_state_goes_to_text_and_back() {
        let mut state = TransientState::default();
        state.values.insert(
            "Commit".into(),
            vec!["--signoff".into(), "--gpg-sign=A\tB\\C".into()],
        );
        state.remember("--author=", "ann");
        state.remember("--author=", "bob");
        state.remember("--author=", "ann");
        state.toggle_hidden(MenuKind::Log, 'W');
        let text = state.render();
        assert!(text.contains("history\t--author=\tann\tbob\n"), "{text}");
        assert_eq!(TransientState::parse(&text), state);
        assert_eq!(
            TransientState::parse("junk\nvalue\nvalue\tLog\t--no-merges\n").values["Log"],
            ["--no-merges"]
        );
        assert!(!state.toggle_hidden(MenuKind::Log, 'W'));
        assert!(state.hidden_in(MenuKind::Log).is_empty());
    }

    #[test]
    fn the_history_keeps_the_newest_twenty_once_each() {
        let mut state = TransientState::default();
        for n in 0..30 {
            state.remember("-S", &format!("v{}", n % 25));
        }
        let history = &state.history["-S"];
        assert_eq!(history.len(), HISTORY_LENGTH);
        assert_eq!(history[0], "v4");
    }

    #[test]
    fn a_menu_takes_saved_arguments_back() {
        let mut menu = log_menu();
        menu.set_args(&["--no-merges".into(), "--author=ann".into()]);
        assert_eq!(menu.args(), ["--author=ann", "--no-merges"]);
        // What is not there is turned off.
        menu.set_args(&[]);
        assert!(menu.args().is_empty());
        assert!(commit_menu().has_arguments());
    }
}
