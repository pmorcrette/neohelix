//! `#+STARTUP:`, which is how a file says how it wants to open and what it
//! wants recorded.
//!
//! The parser has collected these options since Task 1.21 and nothing read
//! them. They fall in two groups: how much of the outline is showing when the
//! file opens, and which state changes leave a trace in the entry. Both live
//! here so that one reading of the options serves both.
//!
//! The defaults are Org's as of 9.4, where a file declaring nothing opens with
//! everything showing, with one deliberate difference noted on
//! [`Startup::log_into_drawer`].

use crate::logging::Log;
use crate::outline::{headings, line_starts, ranges_for};
use crate::parser::FileSettings;

/// How much of the outline shows when the file opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// Only the top-level headlines: `overview`, or `fold`.
    Overview,
    /// Every headline and no body: `content`.
    Content,
    /// Headlines down to this level and no body: `show2levels` to
    /// `show5levels`.
    Levels(usize),
    /// Every line, with drawers and archived subtrees still closed:
    /// `showall`, or `nofold`.
    ShowAll,
    /// Every line, drawers included: `showeverything`, Org's default.
    ShowEverything,
}

/// What a file's `#+STARTUP:` options amount to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Startup {
    pub opening: Opening,
    /// `hidedrawers` and `showdrawers`. Only takes effect when the opening is
    /// not [`Opening::ShowEverything`], which is what that option means.
    pub hide_drawers: bool,
    /// `hideblocks` and `nohideblocks`.
    pub hide_blocks: bool,
    /// `logdone`, `lognotedone`, `nologdone`: a `CLOSED:` stamp when an entry
    /// is marked done, and a note with it for the second.
    pub log_done: Option<Log>,
    /// `logrepeat`, `lognoterepeat`, `nologrepeat`: what marking a repeating
    /// entry done records, since it goes straight back to a TODO state.
    pub log_repeat: Option<Log>,
    /// `logreschedule`, `lognotereschedule`, `nologreschedule`.
    pub log_reschedule: Option<Log>,
    /// `logredeadline`, `lognoteredeadline`, `nologredeadline`.
    pub log_redeadline: Option<Log>,
    /// `logdrawer` and `nologdrawer`.
    ///
    /// Org's default is to write log lines straight into the body. The fork
    /// defaults to the `:LOGBOOK:` drawer instead, because that is what its
    /// logging commands have always written, and changing where notes go
    /// under a user's feet would be worse than differing from Emacs here.
    /// `nologdrawer` gets Org's behaviour.
    pub log_into_drawer: bool,
}

impl Default for Startup {
    fn default() -> Self {
        Self {
            opening: Opening::ShowEverything,
            hide_drawers: true,
            hide_blocks: false,
            log_done: None,
            log_repeat: Some(Log::Time),
            log_reschedule: None,
            log_redeadline: None,
            log_into_drawer: true,
        }
    }
}

impl Startup {
    /// Reads the options in order, so a later one overrides an earlier one.
    ///
    /// Options this does not know are ignored rather than reported: Org has
    /// seventy of them, several are about Emacs itself (`indent`,
    /// `inlineimages`), and a file written for Emacs should still open.
    pub fn from_options<S: AsRef<str>>(options: &[S]) -> Self {
        let mut startup = Self::default();

        for option in options {
            let option = option.as_ref().to_ascii_lowercase();
            match option.as_str() {
                "overview" | "fold" => startup.opening = Opening::Overview,
                "content" => startup.opening = Opening::Content,
                "showall" | "nofold" => startup.opening = Opening::ShowAll,
                "showeverything" => startup.opening = Opening::ShowEverything,
                "show2levels" => startup.opening = Opening::Levels(2),
                "show3levels" => startup.opening = Opening::Levels(3),
                "show4levels" => startup.opening = Opening::Levels(4),
                "show5levels" => startup.opening = Opening::Levels(5),
                "hidedrawers" => startup.hide_drawers = true,
                "showdrawers" => startup.hide_drawers = false,
                "hideblocks" => startup.hide_blocks = true,
                "nohideblocks" | "showblocks" => startup.hide_blocks = false,
                "logdone" => startup.log_done = Some(Log::Time),
                "lognotedone" => startup.log_done = Some(Log::Note),
                "nologdone" => startup.log_done = None,
                "logrepeat" => startup.log_repeat = Some(Log::Time),
                "lognoterepeat" => startup.log_repeat = Some(Log::Note),
                "nologrepeat" => startup.log_repeat = None,
                "logreschedule" => startup.log_reschedule = Some(Log::Time),
                "lognotereschedule" => startup.log_reschedule = Some(Log::Note),
                "nologreschedule" => startup.log_reschedule = None,
                "logredeadline" => startup.log_redeadline = Some(Log::Time),
                "lognoteredeadline" => startup.log_redeadline = Some(Log::Note),
                "nologredeadline" => startup.log_redeadline = None,
                "logdrawer" => startup.log_into_drawer = true,
                "nologdrawer" => startup.log_into_drawer = false,
                _ => {}
            }
        }

        startup
    }

    /// The options a file declares.
    pub fn of(settings: &FileSettings) -> Self {
        Self::from_options(&settings.startup)
    }
}

/// The character ranges to hide so the file opens the way it asks to.
///
/// Hiding is decided per line and then turned into ranges, the same way a
/// sparse tree is, so a folded subtree here is the same range that folding
/// its headline by hand would produce — and cycling it afterwards works.
///
/// Three kinds of hiding are kept apart because they combine rather than
/// override: a `:VISIBILITY: all` property opens its subtree's outline, but a
/// drawer inside it stays closed.
pub fn opening_ranges(text: &str, startup: &Startup) -> Vec<(usize, usize)> {
    if startup.opening == Opening::ShowEverything {
        return Vec::new();
    }

    let lines: Vec<&str> = text.lines().collect();
    let starts = line_starts(text);
    let entries = headings(text);

    let mut outline = vec![false; lines.len()];
    let deepest = match startup.opening {
        Opening::Overview => Some(1),
        Opening::Content => None,
        Opening::Levels(level) => Some(level),
        Opening::ShowAll | Opening::ShowEverything => Some(usize::MAX),
    };
    if let Some(first) = entries.first() {
        if deepest != Some(usize::MAX) {
            // The preamble above the first headline always shows.
            for hidden in &mut outline[first.line..] {
                *hidden = true;
            }
            for entry in &entries {
                if deepest.is_none_or(|deepest| entry.level <= deepest) {
                    outline[entry.line] = false;
                }
            }
        }
    }

    // Per-entry `:VISIBILITY:`, then archived subtrees, both in document
    // order so that a parent is settled before its children are looked at.
    for (index, entry) in entries.iter().enumerate() {
        // An entry whose headline is hidden cannot open what is below it.
        if outline[entry.line] {
            continue;
        }
        let end = entries[index + 1..]
            .iter()
            .find(|other| other.level <= entry.level)
            .map_or(lines.len(), |other| other.line);
        let below = entries[index + 1..]
            .iter()
            .take_while(|other| other.line < end);

        if let Some(visibility) = property(&lines, entry.line, end, "VISIBILITY") {
            let body = entry.line + 1..end;
            match visibility.to_ascii_lowercase().as_str() {
                "folded" => outline[body].fill(true),
                "children" => {
                    outline[body].fill(true);
                    for child in below.clone().filter(|c| c.level == entry.level + 1) {
                        outline[child.line] = false;
                    }
                }
                "content" => {
                    outline[body].fill(true);
                    for child in below.clone() {
                        outline[child.line] = false;
                    }
                }
                "all" | "showall" => outline[body].fill(false),
                _ => {}
            }
        }

        if entry.tags.iter().any(|tag| tag == "ARCHIVE") {
            outline[entry.line + 1..end].fill(true);
        }
    }

    let mut hidden = outline;
    if startup.hide_drawers {
        hide_interiors(&mut hidden, &lines, drawer_end);
    }
    if startup.hide_blocks {
        hide_interiors(&mut hidden, &lines, block_end);
    }

    ranges_for(&hidden, &starts)
}

/// Hides everything after each opening line up to and including its closing
/// line, leaving the opening line to show that something is there.
fn hide_interiors(
    hidden: &mut [bool],
    lines: &[&str],
    end_of: fn(&[&str], usize) -> Option<usize>,
) {
    let mut at = 0;
    while at < lines.len() {
        match end_of(lines, at) {
            Some(end) => {
                hidden[at + 1..=end].fill(true);
                at = end + 1;
            }
            None => at += 1,
        }
    }
}

/// The `:END:` closing a drawer opened at `at`, if one is.
///
/// A drawer cannot span a headline, so an opener with no `:END:` before the
/// next one is just a line that looks like a drawer.
fn drawer_end(lines: &[&str], at: usize) -> Option<usize> {
    let name = lines[at].trim().strip_prefix(':')?.strip_suffix(':')?;
    if name.is_empty()
        || name.eq_ignore_ascii_case("END")
        || !name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }

    lines[at + 1..]
        .iter()
        .take_while(|line| crate::restructure::headline_level(line).is_none())
        .position(|line| line.trim().eq_ignore_ascii_case(":END:"))
        .map(|offset| at + 1 + offset)
}

/// The `#+END_…` closing a block opened at `at`, if one is.
fn block_end(lines: &[&str], at: usize) -> Option<usize> {
    let trimmed = lines[at].trim_start();
    let name = trimmed
        .get(..8)
        .filter(|head| head.eq_ignore_ascii_case("#+BEGIN_"))
        .map(|_| &trimmed[8..])?
        .split_whitespace()
        .next()?;
    let closing = format!("#+END_{name}");

    lines[at + 1..]
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case(&closing))
        .map(|offset| at + 1 + offset)
}

/// A property from the drawer of the headline at `headline`.
fn property(lines: &[&str], headline: usize, end: usize, key: &str) -> Option<String> {
    let mut at = headline + 1;
    if lines
        .get(at)
        .is_some_and(|line| crate::restructure::is_planning_line(line))
    {
        at += 1;
    }
    if !lines
        .get(at)
        .is_some_and(|line| line.trim().eq_ignore_ascii_case(":PROPERTIES:"))
    {
        return None;
    }

    lines[at + 1..end]
        .iter()
        .take_while(|line| !line.trim().eq_ignore_ascii_case(":END:"))
        .find_map(|line| {
            let (name, value) = line.trim().strip_prefix(':')?.split_once(':')?;
            name.eq_ignore_ascii_case(key)
                .then(|| value.trim().to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What survives the hiding: each visible line, with `…` where something
    /// is folded into it.
    fn opened(text: &str, options: &[&str]) -> String {
        let ranges = opening_ranges(text, &Startup::from_options(options));
        let chars: Vec<char> = text.chars().collect();

        let mut out = String::new();
        let mut at = 0;
        for (start, end) in ranges {
            out.extend(&chars[at..start]);
            out.push('…');
            at = end;
        }
        out.extend(&chars[at..]);
        out
    }

    const FILE: &str = "#+STARTUP: overview\n\
                        Preamble\n\
                        * One\n\
                        :PROPERTIES:\n\
                        :ID: a\n\
                        :END:\n\
                        Body one\n\
                        ** One.one\n\
                        Deep body\n\
                        *** One.one.one\n\
                        * Two\n\
                        Body two\n";

    #[test]
    fn a_file_that_declares_nothing_opens_with_everything_showing() {
        assert_eq!(opened(FILE, &[]), FILE);
        assert_eq!(opened(FILE, &["showeverything"]), FILE);
    }

    #[test]
    fn overview_shows_the_top_level_headlines_and_the_preamble() {
        assert_eq!(
            opened(FILE, &["overview"]),
            "#+STARTUP: overview\nPreamble\n* One…\n* Two…\n"
        );
        assert_eq!(opened(FILE, &["fold"]), opened(FILE, &["overview"]));
    }

    #[test]
    fn content_shows_every_headline_and_no_body() {
        assert_eq!(
            opened(FILE, &["content"]),
            "#+STARTUP: overview\nPreamble\n* One…\n** One.one…\n*** One.one.one\n* Two…\n"
        );
    }

    #[test]
    fn levels_stop_at_the_depth_asked_for() {
        assert_eq!(
            opened(FILE, &["show2levels"]),
            "#+STARTUP: overview\nPreamble\n* One…\n** One.one…\n* Two…\n"
        );
    }

    #[test]
    fn showall_still_closes_drawers_unless_told_not_to() {
        let shown = opened(FILE, &["showall"]);
        assert!(shown.contains(":PROPERTIES:…\nBody one"), "{shown}");
        assert_eq!(opened(FILE, &["showall", "showdrawers"]), FILE);
    }

    #[test]
    fn a_later_option_overrides_an_earlier_one() {
        assert_eq!(opened(FILE, &["overview", "showeverything"]), FILE);
        assert_eq!(
            opened(FILE, &["showeverything", "overview"]),
            opened(FILE, &["overview"])
        );
    }

    #[test]
    fn blocks_close_only_when_asked() {
        let text = "* A\n#+begin_src rust\nfn main() {}\n#+end_src\nafter\n";
        assert_eq!(opened(text, &["showall"]), text);
        assert_eq!(
            opened(text, &["showall", "hideblocks"]),
            "* A\n#+begin_src rust…\nafter\n"
        );
    }

    #[test]
    fn a_drawer_without_an_end_is_not_a_drawer() {
        let text = "* A\n:NOTES:\nstill body\n* B\n:END:\n";
        assert_eq!(opened(text, &["showall"]), text);
    }

    #[test]
    fn a_visibility_property_overrides_the_file_for_its_subtree() {
        let text =
            "* Open me\n:PROPERTIES:\n:VISIBILITY: all\n:END:\nBody\n** Child\n* Closed\nHidden\n";
        assert_eq!(
            opened(text, &["overview"]),
            "* Open me\n:PROPERTIES:…\nBody\n** Child\n* Closed…\n"
        );

        let text = "* Parent\n:PROPERTIES:\n:VISIBILITY: children\n:END:\nBody\n** Child\nChild body\n*** Grandchild\n";
        assert_eq!(opened(text, &["showall"]), "* Parent…\n** Child…\n");
    }

    #[test]
    fn a_visibility_property_under_a_hidden_headline_opens_nothing() {
        let text = "* Top\n** Inner\n:PROPERTIES:\n:VISIBILITY: all\n:END:\nBody\n";
        assert_eq!(opened(text, &["overview"]), "* Top…\n");
    }

    #[test]
    fn archived_subtrees_stay_closed() {
        let text = "* Old :ARCHIVE:\nGone\n* Live\nHere\n";
        assert_eq!(
            opened(text, &["showall"]),
            "* Old :ARCHIVE:…\n* Live\nHere\n"
        );
        assert_eq!(opened(text, &["showeverything"]), text);
    }

    #[test]
    fn a_planning_line_does_not_hide_the_drawer_from_the_property_lookup() {
        let text =
            "* Task\nSCHEDULED: <2026-09-23 Wed>\n:PROPERTIES:\n:VISIBILITY: folded\n:END:\nBody\n";
        assert_eq!(opened(text, &["showall"]), "* Task…\n");
    }

    #[test]
    fn logging_options_are_read() {
        let startup = Startup::from_options(&["lognotedone", "nologrepeat", "nologdrawer"]);
        assert_eq!(startup.log_done, Some(Log::Note));
        assert_eq!(startup.log_repeat, None);
        assert!(!startup.log_into_drawer);

        let default = Startup::default();
        assert_eq!(default.log_done, None);
        assert_eq!(default.log_repeat, Some(Log::Time));
    }
}
