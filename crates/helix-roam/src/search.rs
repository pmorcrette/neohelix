//! Org's two ways of looking for entries across the notes: the match
//! language of tags, properties and states (the agenda's `m`), and the
//! search of words in the text (its `s`).
//!
//! A match reads the index only: `+work-boss|urgent/TODO|WAITING`. A search
//! needs the entries' text, which the index does not keep, so it is given
//! the text to look through.

use regex::{Regex, RegexBuilder};

use crate::entry::Entry;
use crate::Date;

/// `+work-boss|urgent`, `Effort>0:30&PRIORITY="A"`, `/!TODO|WAITING`.
///
/// Alternatives are separated by `|`; each is a run of terms that must all
/// hold. A term is a tag (`work`, `+work`, `-boss`, `{^proj}`) or a
/// property compared with `=`, `<>`, `<`, `<=`, `>`, `>=` to a `"string"`,
/// a `{regexp}`, a number or duration (`3`, `1:30`) or a date (`<today>`,
/// `<+3d>`, `<2026-10-01>`). Special properties are `TODO`, `LEVEL`,
/// `PRIORITY`, `CATEGORY`, `ITEM` (the title), `TAGS`, `SCHEDULED`,
/// `DEADLINE` and `CLOSED`. After a `/`, a list of TODO keywords
/// (`TODO|WAITING`, or `-DONE-CANCELLED` to leave some out, which keeps
/// the entries without a state), with `!` first for unfinished states only.
#[derive(Debug, Clone)]
pub struct Match {
    alternatives: Vec<Vec<(bool, Term)>>,
    todo: Option<TodoPart>,
}

#[derive(Debug, Clone)]
enum Term {
    Tag(String),
    TagRegex(Regex),
    Property { name: String, op: Op, value: Value },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone)]
enum Value {
    Text(String),
    Regex(Regex),
    Number(f64),
    Date(Date),
}

#[derive(Debug, Clone, Default)]
struct TodoPart {
    /// Unfinished states only.
    open: bool,
    wanted: Vec<String>,
    unwanted: Vec<String>,
}

fn regex(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|err| format!("{{{pattern}}} is not a valid regexp: {err}"))
}

/// Reads `3`, `2.5` or a duration `1:30` (as minutes).
fn number(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some((hours, minutes)) = text.split_once(':') {
        let hours: f64 = hours.parse().ok()?;
        let minutes: f64 = minutes.parse().ok()?;
        return Some(hours * 60.0 + minutes);
    }
    text.parse().ok()
}

/// Reads `today`, `tomorrow`, `yesterday`, `+3d`, `-2w`, `+1m` or an ISO
/// date, relative to `today`.
pub fn relative_date(text: &str, today: Date) -> Option<Date> {
    let text = text.trim();
    match text.to_ascii_lowercase().as_str() {
        "today" | "now" => return Some(today),
        "tomorrow" => return Some(today.offset_by(1)),
        "yesterday" => return Some(today.offset_by(-1)),
        _ => {}
    }
    if let Some(sign @ ('+' | '-')) = text.chars().next() {
        let rest = &text[1..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let count: i64 = digits.parse().ok()?;
        let count = if sign == '-' { -count } else { count };
        let days = match &rest[digits.len()..] {
            "" | "d" => count,
            "w" => count * 7,
            "m" => count * 30,
            "y" => count * 365,
            _ => return None,
        };
        return Some(today.offset_by(days));
    }
    Date::parse_iso(text)
}

impl Match {
    /// Reads a match string; dates in it are relative to `today`.
    pub fn parse(input: &str, today: Date) -> Result<Self, String> {
        let parts = split_outside(input, '/');
        let (terms, todo) = match parts.as_slice() {
            [terms] => (*terms, None),
            [terms, ..] => (*terms, Some(parse_todo(&input[terms.len() + 1..]))),
            [] => (input, None),
        };
        let alternatives = split_outside(terms, '|')
            .into_iter()
            .map(|alternative| parse_terms(alternative.trim(), today))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { alternatives, todo })
    }

    pub fn matches(&self, entry: &Entry) -> bool {
        let todo_ok = self.todo.as_ref().is_none_or(|part| part.matches(entry));
        todo_ok
            && self.alternatives.iter().any(|terms| {
                terms
                    .iter()
                    .all(|(wanted, term)| term.holds(entry) == *wanted)
            })
    }
}

/// Splits `text` at `separator`, except inside a `{regexp}`, a `"string"`
/// or a `<date>`, where it is part of the value.
fn split_outside(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut closing = None;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        match closing {
            Some(close) if c == close => closing = None,
            Some(_) => {}
            None => match c {
                '{' => closing = Some('}'),
                '"' => closing = Some('"'),
                // After an operator, `<` opens a date; elsewhere it is one.
                '<' if text[at + 1..]
                    .starts_with(|c: char| c == '+' || c == '-' || c.is_alphanumeric())
                    && text[..at].ends_with(['=', '<', '>']) =>
                {
                    closing = Some('>')
                }
                c if c == separator => {
                    parts.push(&text[start..at]);
                    start = at + c.len_utf8();
                }
                _ => {}
            },
        }
    }
    parts.push(&text[start..]);
    parts
}

fn parse_todo(text: &str) -> TodoPart {
    let mut part = TodoPart::default();
    let mut text = text.trim();
    if let Some(rest) = text.strip_prefix('!') {
        part.open = true;
        text = rest;
    }
    // `TODO|WAITING` names the ones wanted, `-DONE-CANCELLED` the others.
    let mut word = String::new();
    let mut sign = '+';
    let mut flush = |word: &mut String, sign: char| {
        if !word.is_empty() {
            let keyword = std::mem::take(word);
            if sign == '-' {
                part.unwanted.push(keyword);
            } else {
                part.wanted.push(keyword);
            }
        }
    };
    for c in text.chars() {
        match c {
            '|' | '+' | '-' | ' ' => {
                flush(&mut word, sign);
                sign = if c == '-' { '-' } else { '+' };
            }
            c => word.push(c),
        }
    }
    flush(&mut word, sign);
    part
}

impl TodoPart {
    fn matches(&self, entry: &Entry) -> bool {
        // Leaving states out says nothing of entries without one, as in
        // Org: `+LEVEL=2/-DONE` takes the plain headlines too.
        let Some(state) = &entry.todo else {
            return !self.open && self.wanted.is_empty();
        };
        (!self.open || !state.done)
            && (self.wanted.is_empty() || self.wanted.contains(&state.keyword))
            && !self.unwanted.contains(&state.keyword)
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '%')
}

/// Reads one alternative: terms, each with an optional `+` or `-`,
/// optionally joined by `&`.
fn parse_terms(text: &str, today: Date) -> Result<Vec<(bool, Term)>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut terms = Vec::new();
    let mut at = 0;
    let read_until = |from: usize, close: char| -> Result<(String, usize), String> {
        let end = chars[from..]
            .iter()
            .position(|&c| c == close)
            .map(|offset| from + offset)
            .ok_or_else(|| format!("`{text}` has an unclosed {}", chars[from - 1]))?;
        Ok((chars[from..end].iter().collect(), end + 1))
    };

    while at < chars.len() {
        match chars[at] {
            '&' | ' ' => {
                at += 1;
                continue;
            }
            _ => {}
        }
        let wanted = match chars[at] {
            '+' => {
                at += 1;
                true
            }
            '-' => {
                at += 1;
                false
            }
            _ => true,
        };
        if chars.get(at) == Some(&'{') {
            let (pattern, next) = read_until(at + 1, '}')?;
            terms.push((wanted, Term::TagRegex(regex(&pattern)?)));
            at = next;
            continue;
        }
        let start = at;
        while at < chars.len() && is_word_char(chars[at]) {
            at += 1;
        }
        let name: String = chars[start..at].iter().collect();
        if name.is_empty() {
            return Err(format!("`{text}`: nothing to match at `{}`", chars[at]));
        }

        let op = match (chars.get(at), chars.get(at + 1)) {
            (Some('<'), Some('>')) => Some((Op::Ne, 2)),
            (Some('<'), Some('=')) => Some((Op::Le, 2)),
            (Some('>'), Some('=')) => Some((Op::Ge, 2)),
            (Some('='), _) => Some((Op::Eq, 1)),
            (Some('<'), _) => Some((Op::Lt, 1)),
            (Some('>'), _) => Some((Op::Gt, 1)),
            _ => None,
        };
        let Some((op, length)) = op else {
            terms.push((wanted, Term::Tag(name)));
            continue;
        };
        at += length;
        let value = match chars.get(at) {
            Some('"') => {
                let (text, next) = read_until(at + 1, '"')?;
                at = next;
                // A quoted duration still compares as one: `Effort<"1:00"`.
                match number(&text).filter(|_| text.contains(':')) {
                    Some(minutes) => Value::Number(minutes),
                    None => Value::Text(text),
                }
            }
            Some('{') => {
                let (pattern, next) = read_until(at + 1, '}')?;
                at = next;
                Value::Regex(regex(&pattern)?)
            }
            Some('<') => {
                let (date, next) = read_until(at + 1, '>')?;
                at = next;
                Value::Date(
                    relative_date(&date, today).ok_or_else(|| format!("<{date}> is not a date"))?,
                )
            }
            _ => {
                let start = at;
                while at < chars.len() && (chars[at].is_ascii_digit() || ".:".contains(chars[at])) {
                    at += 1;
                }
                let raw: String = chars[start..at].iter().collect();
                Value::Number(
                    number(&raw).ok_or_else(|| format!("`{name}` is compared to nothing"))?,
                )
            }
        };
        if matches!(value, Value::Regex(_)) && !matches!(op, Op::Eq | Op::Ne) {
            return Err(format!("`{name}` can only be = or <> a regexp"));
        }
        terms.push((wanted, Term::Property { name, op, value }));
    }
    Ok(terms)
}

/// What a property is for an entry: text, or a date for the planning ones.
enum Found {
    Text(String),
    Date(Date),
    Missing,
}

fn property_of(entry: &Entry, name: &str) -> Found {
    let stamp =
        |stamp: Option<crate::Timestamp>| stamp.map_or(Found::Missing, |s| Found::Date(s.day()));
    match name.to_ascii_uppercase().as_str() {
        "TODO" => entry
            .todo
            .as_ref()
            .map_or(Found::Missing, |state| Found::Text(state.keyword.clone())),
        "LEVEL" => Found::Text(entry.level.to_string()),
        "PRIORITY" => entry
            .priority
            .map_or(Found::Missing, |letter| Found::Text(letter.to_string())),
        "CATEGORY" => Found::Text(entry.category.clone()),
        "ITEM" => Found::Text(entry.title.clone()),
        "TAGS" => Found::Text(if entry.tags.is_empty() {
            String::new()
        } else {
            format!(":{}:", entry.tags.join(":"))
        }),
        "ALLTAGS" => {
            let all: Vec<&str> = entry.all_tags().map(String::as_str).collect();
            Found::Text(if all.is_empty() {
                String::new()
            } else {
                format!(":{}:", all.join(":"))
            })
        }
        "SCHEDULED" => stamp(entry.scheduled),
        "DEADLINE" => stamp(entry.deadline),
        "CLOSED" => stamp(entry.closed),
        _ => entry
            .property(name)
            .map_or(Found::Missing, |value| Found::Text(value.to_string())),
    }
}

fn compare<T: PartialOrd>(op: Op, left: T, right: T) -> bool {
    match op {
        Op::Eq => left == right,
        Op::Ne => left != right,
        Op::Lt => left < right,
        Op::Le => left <= right,
        Op::Gt => left > right,
        Op::Ge => left >= right,
    }
}

impl Term {
    fn holds(&self, entry: &Entry) -> bool {
        match self {
            Term::Tag(tag) => entry.all_tags().any(|own| own == tag),
            Term::TagRegex(pattern) => entry.all_tags().any(|own| pattern.is_match(own)),
            Term::Property { name, op, value } => {
                match (property_of(entry, name), value) {
                    (Found::Text(found), Value::Text(wanted)) => {
                        compare(*op, found.as_str(), wanted.as_str())
                    }
                    // Org reads a missing property as empty.
                    (Found::Missing, Value::Text(wanted)) => compare(*op, "", wanted.as_str()),
                    (Found::Text(found), Value::Regex(pattern)) => {
                        pattern.is_match(&found) == (*op == Op::Eq)
                    }
                    (Found::Missing, Value::Regex(_)) => *op == Op::Ne,
                    (Found::Text(found), Value::Number(wanted)) => {
                        number(&found).is_some_and(|found| compare(*op, found, *wanted))
                    }
                    (Found::Date(found), Value::Date(wanted)) => compare(*op, found, *wanted),
                    _ => false,
                }
            }
        }
    }
}

/// Words to find in an entry's text: `rust async` (both), `-draft`,
/// `"a phrase"`, `{regexp}`; a leading `+` is allowed. Case does not
/// matter.
#[derive(Debug, Clone)]
pub struct Search {
    terms: Vec<(bool, SearchTerm)>,
}

#[derive(Debug, Clone)]
enum SearchTerm {
    Words(String),
    Regex(Regex),
}

impl Search {
    pub fn parse(input: &str) -> Result<Self, String> {
        let chars: Vec<char> = input.chars().collect();
        let mut terms = Vec::new();
        let mut at = 0;
        while at < chars.len() {
            if chars[at].is_whitespace() {
                at += 1;
                continue;
            }
            let wanted = match chars[at] {
                '+' => {
                    at += 1;
                    true
                }
                '-' => {
                    at += 1;
                    false
                }
                _ => true,
            };
            let (close, open) = match chars.get(at) {
                Some('"') => ('"', true),
                Some('{') => ('}', true),
                _ => (' ', false),
            };
            let start = if open { at + 1 } else { at };
            let end = chars[start..]
                .iter()
                .position(|&c| if open { c == close } else { c.is_whitespace() })
                .map_or(chars.len(), |offset| start + offset);
            if open && end == chars.len() {
                return Err(format!("`{input}` has an unclosed {}", chars[at]));
            }
            let text: String = chars[start..end].iter().collect();
            at = end + usize::from(open);
            if text.is_empty() {
                continue;
            }
            let term = if close == '}' {
                SearchTerm::Regex(regex(&text)?)
            } else {
                SearchTerm::Words(text.to_lowercase())
            };
            terms.push((wanted, term));
        }
        if terms.is_empty() {
            return Err("Nothing to search for".to_string());
        }
        Ok(Self { terms })
    }

    /// Whether `text` (an entry's headline and body) has what is searched.
    pub fn matches(&self, text: &str) -> bool {
        let lower = text.to_lowercase();
        self.terms.iter().all(|(wanted, term)| {
            let found = match term {
                SearchTerm::Words(words) => lower.contains(words.as_str()),
                SearchTerm::Regex(pattern) => pattern.is_match(text),
            };
            found == *wanted
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::entries;
    use crate::FileSettings;
    use std::path::Path;

    const TEXT: &str = "\
#+TODO: TODO NEXT WAITING | DONE CANCELLED
* Travail :work:
** TODO [#A] Rapport :urgent:
DEADLINE: <2026-10-01 Thu>
:PROPERTIES:
:Effort: 1:30
:END:
** WAITING Réponse du chef :boss:
** DONE Fini
* Maison :home:
** NEXT Tondre
:PROPERTIES:
:Effort: 0:20
:END:
";

    fn titles(query: &str) -> Vec<String> {
        let today = Date {
            year: 2026,
            month: 9,
            day: 28,
        };
        let found = entries(TEXT, Path::new("/n/a.org"), &FileSettings::scan(TEXT));
        let matcher = Match::parse(query, today).unwrap();
        found
            .iter()
            .filter(|entry| matcher.matches(entry))
            .map(|entry| entry.title.clone())
            .collect()
    }

    #[test]
    fn tags_are_matched_with_what_is_inherited() {
        assert_eq!(
            titles("work"),
            ["Travail", "Rapport", "Réponse du chef", "Fini"]
        );
        assert_eq!(titles("work-boss-urgent"), ["Travail", "Fini"]);
        assert_eq!(titles("urgent|home"), ["Rapport", "Maison", "Tondre"]);
        assert_eq!(titles("{^ur}"), ["Rapport"]);
    }

    #[test]
    fn properties_and_states_are_compared() {
        assert_eq!(titles("Effort>0:30"), ["Rapport"]);
        assert_eq!(titles("Effort<\"1:00\""), ["Tondre"]);
        assert_eq!(titles("PRIORITY=\"A\""), ["Rapport"]);
        assert_eq!(titles("DEADLINE<=<+3d>"), ["Rapport"]);
        assert_eq!(titles("LEVEL=1"), ["Travail", "Maison"]);
        assert_eq!(titles("ITEM={^R}"), ["Rapport", "Réponse du chef"]);
        // A `|` or a `/` in a value belongs to it.
        assert_eq!(titles("ITEM={^(Fini|Tondre)$}"), ["Fini", "Tondre"]);
        assert_eq!(titles("ITEM=\"a/b\"|ITEM={^Fini$}/DONE"), ["Fini"]);
        assert_eq!(
            titles("DEADLINE<=<+3d>|LEVEL=1"),
            ["Travail", "Rapport", "Maison"]
        );
        assert_eq!(titles("work/!"), ["Rapport", "Réponse du chef"]);
        assert_eq!(titles("/TODO|NEXT"), ["Rapport", "Tondre"]);
        assert_eq!(
            titles("/-DONE-WAITING"),
            ["Travail", "Rapport", "Maison", "Tondre"]
        );
        assert_eq!(
            titles("TODO<>\"DONE\"&work"),
            ["Travail", "Rapport", "Réponse du chef"]
        );
    }

    #[test]
    fn a_bad_match_says_why() {
        let today = Date::today();
        assert!(Match::parse("Effort>", today).is_err());
        assert!(Match::parse("{unclosed", today).is_err());
        assert!(Match::parse("D<<nope>", today).is_err());
    }

    #[test]
    fn a_search_wants_every_word_and_none_of_the_others() {
        let search = Search::parse("rust -draft \"async fn\" {v[0-9]}").unwrap();
        assert!(search.matches("Rust notes: ASYNC FN in v2"));
        assert!(!search.matches("Rust notes: async fn in v2, draft"));
        assert!(!search.matches("Rust notes: async fn"));
        assert!(Search::parse("  ").is_err());
        assert!(Search::parse("\"open").is_err());
    }
}
