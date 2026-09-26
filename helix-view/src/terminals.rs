//! The integrated terminals: every shell started with `:terminal` or
//! `:terminal-new`, and which one the terminal view shows.
//!
//! Terminals are numbered from 1, and a new one takes the lowest number no
//! other terminal has, as tmux numbers its windows, so that `Ctrl-\ 1` to
//! `Ctrl-\ 9` stay short. A number stays with its terminal until it closes.

/// One terminal and what the list knows about it.
#[derive(Debug)]
pub struct Entry<T> {
    pub number: usize,
    /// A name given with `:terminal-rename`; without one, the title the
    /// program running in it set is shown.
    pub name: Option<String>,
    /// The bell rang while another terminal was shown.
    pub alert: bool,
    pub terminal: T,
}

/// The terminals, in the order they were opened, and the one shown.
#[derive(Debug)]
pub struct Terminals<T = helix_pty::PtyTerminal> {
    entries: Vec<Entry<T>>,
    /// An index into `entries`, meaningless while it is empty.
    current: usize,
}

impl<T> Default for Terminals<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            current: 0,
        }
    }
}

impl<T> Terminals<T> {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(&self) -> &[Entry<T>] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [Entry<T>] {
        &mut self.entries
    }

    pub fn current_entry(&self) -> Option<&Entry<T>> {
        self.entries.get(self.current)
    }

    pub fn current_entry_mut(&mut self) -> Option<&mut Entry<T>> {
        self.entries.get_mut(self.current)
    }

    /// The terminal the view shows.
    pub fn current(&self) -> Option<&T> {
        self.current_entry().map(|entry| &entry.terminal)
    }

    pub fn current_mut(&mut self) -> Option<&mut T> {
        self.current_entry_mut().map(|entry| &mut entry.terminal)
    }

    /// Adds a terminal and shows it; returns its number.
    pub fn add(&mut self, terminal: T) -> usize {
        let number = (1..)
            .find(|number| self.entries.iter().all(|entry| entry.number != *number))
            .unwrap_or(1);
        self.entries.push(Entry {
            number,
            name: None,
            alert: false,
            terminal,
        });
        self.current = self.entries.len() - 1;
        number
    }

    /// Shows the terminal numbered `number`, if there is one.
    pub fn select(&mut self, number: usize) -> bool {
        match self.entries.iter().position(|entry| entry.number == number) {
            Some(index) => {
                self.current = index;
                self.entries[index].alert = false;
                true
            }
            None => false,
        }
    }

    /// Shows the next terminal in the list, or the previous one, wrapping
    /// around; returns the number of the one now shown.
    pub fn cycle(&mut self, forward: bool) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let count = self.entries.len();
        let index = if forward {
            (self.current + 1) % count
        } else {
            (self.current + count - 1) % count
        };
        let number = self.entries[index].number;
        self.select(number);
        Some(number)
    }

    /// Removes the terminal shown, which ends its shell; the one before it
    /// in the list is shown next.
    pub fn remove_current(&mut self) -> Option<Entry<T>> {
        if self.entries.is_empty() {
            return None;
        }
        let entry = self.entries.remove(self.current);
        self.current = self.current.saturating_sub(1);
        Some(entry)
    }

    /// Removes the terminals `gone` says are gone, keeping the one shown
    /// shown if it stays; returns the numbers removed.
    pub fn remove_where(&mut self, mut gone: impl FnMut(&mut Entry<T>) -> bool) -> Vec<usize> {
        let current = self.current_entry().map(|entry| entry.number);
        let mut removed = Vec::new();
        self.entries.retain_mut(|entry| {
            let keep = !gone(entry);
            if !keep {
                removed.push(entry.number);
            }
            keep
        });
        if !removed.is_empty() {
            match current
                .and_then(|number| self.entries.iter().position(|entry| entry.number == number))
            {
                Some(index) => self.current = index,
                None => self.current = self.current.min(self.entries.len().saturating_sub(1)),
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbers(terminals: &Terminals<&str>) -> Vec<usize> {
        terminals
            .entries()
            .iter()
            .map(|entry| entry.number)
            .collect()
    }

    #[test]
    fn a_new_terminal_takes_the_lowest_free_number_and_is_shown() {
        let mut terminals = Terminals::default();
        assert_eq!(terminals.add("a"), 1);
        assert_eq!(terminals.add("b"), 2);
        assert_eq!(terminals.add("c"), 3);
        assert_eq!(terminals.current(), Some(&"c"));

        assert!(terminals.select(2));
        assert_eq!(terminals.remove_current().unwrap().terminal, "b");
        // The one before it is shown next.
        assert_eq!(terminals.current(), Some(&"a"));
        assert_eq!(terminals.add("d"), 2);
        assert_eq!(numbers(&terminals), [1, 3, 2]);
        assert!(!terminals.select(7));
    }

    #[test]
    fn cycling_wraps_around() {
        let mut terminals = Terminals::default();
        terminals.add("a");
        terminals.add("b");
        terminals.add("c");
        assert_eq!(terminals.cycle(true), Some(1));
        assert_eq!(terminals.cycle(false), Some(3));
        assert_eq!(terminals.cycle(false), Some(2));
        assert_eq!(Terminals::<&str>::default().cycle(true), None);
    }

    #[test]
    fn removing_others_keeps_the_one_shown() {
        let mut terminals = Terminals::default();
        terminals.add("a");
        terminals.add("gone");
        terminals.add("c");
        assert_eq!(
            terminals.remove_where(|entry| entry.terminal == "gone"),
            [2]
        );
        assert_eq!(terminals.current(), Some(&"c"));

        // The one shown gone too: another is shown.
        assert_eq!(terminals.remove_where(|entry| entry.terminal == "c"), [3]);
        assert_eq!(terminals.current(), Some(&"a"));
        assert_eq!(terminals.remove_where(|_| true), [1]);
        assert_eq!(terminals.current(), None);
        assert!(terminals.remove_current().is_none());
    }
}
