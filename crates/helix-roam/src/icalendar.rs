//! iCalendar (RFC 5545), from the entries the agenda reads: how an agenda
//! reaches a phone's calendar.
//!
//! As Org's `ox-icalendar` does, every active `SCHEDULED:` is an event
//! titled `S: …`, every active `DEADLINE:` one titled `DL: …`, and every
//! other active timestamp in an entry one of its own; an unfinished task is
//! also a to-do, due at its deadline and starting when it is scheduled. A
//! repeater becomes a recurrence rule. Finished tasks are left out.

use crate::entry::Entry;
use crate::node::{RepeaterUnit, Timestamp};

/// Escapes a text value: backslash, semicolon, comma and newline.
fn escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

/// Folds a content line to 75 octets, continuation lines starting with a
/// space, never inside a character.
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut length = 0;
    for c in line.chars() {
        let size = c.len_utf8();
        // The first line has 75 octets; the others 74 after their space.
        if length + size > 75 {
            out.push_str("\r\n ");
            length = 1;
        }
        out.push(c);
        length += size;
    }
    out.push_str("\r\n");
    out
}

fn date_value(stamp: &Timestamp) -> String {
    format!("{:04}{:02}{:02}", stamp.year, stamp.month, stamp.day)
}

/// `DTSTART` and `DTEND` for a timestamp: a whole day (or the days of a
/// range) without a time, an hour from its time with one.
fn span(stamp: &Timestamp) -> (String, String) {
    match (stamp.hour, stamp.minute) {
        (Some(hour), minute) => {
            let minute = minute.unwrap_or(0);
            let start = format!("{}T{hour:02}{minute:02}00", date_value(stamp));
            let (end_day, end_hour) = if hour >= 23 {
                (stamp.day().offset_by(1), hour + 1 - 24)
            } else {
                (stamp.day(), hour + 1)
            };
            let end = format!(
                "{}T{end_hour:02}{minute:02}00",
                end_day.to_iso().replace('-', "")
            );
            (format!("DTSTART:{start}"), format!("DTEND:{end}"))
        }
        (None, _) => {
            let last = stamp.range_end.unwrap_or(stamp.day());
            (
                format!("DTSTART;VALUE=DATE:{}", date_value(stamp)),
                format!(
                    "DTEND;VALUE=DATE:{}",
                    last.offset_by(1).to_iso().replace('-', "")
                ),
            )
        }
    }
}

fn rrule(stamp: &Timestamp) -> Option<String> {
    let repeater = stamp.repeater.filter(|r| r.count > 0)?;
    let frequency = match repeater.unit {
        RepeaterUnit::Hour => "HOURLY",
        RepeaterUnit::Day => "DAILY",
        RepeaterUnit::Week => "WEEKLY",
        RepeaterUnit::Month => "MONTHLY",
        RepeaterUnit::Year => "YEARLY",
    };
    Some(format!(
        "RRULE:FREQ={frequency};INTERVAL={}",
        repeater.count
    ))
}

/// A stable identity for an entry: its `:ID:`, or a hash of where it is
/// and what it is called.
fn identity(entry: &Entry) -> String {
    if let Some(id) = entry.id {
        return id.to_string();
    }
    // FNV-1a: stable across runs, unlike the standard library's hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let key = format!("{}\u{0}{}", entry.file_path.display(), entry.title);
    for byte in key.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn categories(entry: &Entry) -> String {
    let mut all = vec![escape(&entry.category)];
    all.extend(entry.all_tags().map(|tag| escape(tag)));
    all.dedup();
    format!("CATEGORIES:{}", all.join(","))
}

/// The calendar of `entries`, named `name`; `stamp` is the `DTSTAMP` of
/// every component, the export's own time in UTC (`20260928T120000Z`).
pub fn calendar<'a>(
    entries: impl IntoIterator<Item = &'a Entry>,
    name: &str,
    stamp: &str,
) -> String {
    let mut out = String::new();
    let mut line = |text: String| out.push_str(&fold(&text));
    line("BEGIN:VCALENDAR".to_string());
    line("VERSION:2.0".to_string());
    line("PRODID:-//neohelix//Org export//EN".to_string());
    line("CALSCALE:GREGORIAN".to_string());
    line(format!("X-WR-CALNAME:{}", escape(name)));

    for entry in entries {
        if entry.todo.as_ref().is_some_and(|state| state.done) {
            continue;
        }
        let id = identity(entry);
        let mut event = |kind: &str, prefix: &str, stamp_: &Timestamp, index: usize| {
            let (start, end) = span(stamp_);
            line("BEGIN:VEVENT".to_string());
            let suffix = if index > 0 {
                format!("-{index}")
            } else {
                String::new()
            };
            line(format!("UID:{kind}-{id}{suffix}"));
            line(format!("DTSTAMP:{stamp}"));
            line(start);
            line(end);
            if let Some(rule) = rrule(stamp_) {
                line(rule);
            }
            line(format!("SUMMARY:{prefix}{}", escape(&entry.title)));
            line(categories(entry));
            line("END:VEVENT".to_string());
        };
        if let Some(scheduled) = entry.scheduled.filter(|s| s.active) {
            event("SC", "S: ", &scheduled, 0);
        }
        if let Some(deadline) = entry.deadline.filter(|s| s.active) {
            event("DL", "DL: ", &deadline, 0);
        }
        for (index, timestamp) in entry.timestamps.iter().enumerate() {
            event("TS", "", timestamp, index);
        }

        if entry.is_open_task() {
            line("BEGIN:VTODO".to_string());
            line(format!("UID:TODO-{id}"));
            line(format!("DTSTAMP:{stamp}"));
            if let Some(scheduled) = entry.scheduled.filter(|s| s.active) {
                line(span(&scheduled).0);
            }
            if let Some(deadline) = entry.deadline.filter(|s| s.active) {
                line(span(&deadline).0.replacen("DTSTART", "DUE", 1));
            }
            line(format!("SUMMARY:{}", escape(&entry.title)));
            line("STATUS:NEEDS-ACTION".to_string());
            if let Some(priority) = entry.priority {
                // iCalendar's 1 is the highest, 9 the lowest.
                let value = match priority {
                    'A' => 1,
                    'B' => 5,
                    _ => 9,
                };
                line(format!("PRIORITY:{value}"));
            }
            line(categories(entry));
            line("END:VTODO".to_string());
        }
    }
    line("END:VCALENDAR".to_string());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::entries;
    use crate::FileSettings;
    use std::path::Path;

    #[test]
    fn events_and_todos_as_org_writes_them() {
        let text = "\
* TODO [#A] Rapport :work:
DEADLINE: <2026-10-01 Thu> SCHEDULED: <2026-09-29 Tue 09:30 +1w>
:PROPERTIES:
:ID: 11111111-2222-3333-4444-555555555555
:END:
* Dentiste
Rendez-vous <2026-09-30 Wed 15:00>, puis <2026-10-02 Fri>--<2026-10-04 Sun>.
* DONE Fini
SCHEDULED: <2026-09-28 Mon>
";
        let found = entries(text, Path::new("/n/a.org"), &FileSettings::scan(text));
        let ics = calendar(&found, "Notes", "20260928T120000Z");
        let lines: Vec<&str> = ics.split("\r\n").collect();
        let has = |line: &str| lines.contains(&line);

        assert!(has("BEGIN:VCALENDAR") && has("END:VCALENDAR"));
        assert!(has("UID:SC-11111111-2222-3333-4444-555555555555"));
        assert!(has("DTSTART:20260929T093000"));
        assert!(has("DTEND:20260929T103000"));
        assert!(has("RRULE:FREQ=WEEKLY;INTERVAL=1"));
        assert!(has("SUMMARY:S: Rapport"));
        assert!(has("SUMMARY:DL: Rapport"));
        assert!(has("DTSTART;VALUE=DATE:20261001"));
        assert!(has("DUE;VALUE=DATE:20261001"));
        assert!(has("PRIORITY:1"));
        assert!(has("CATEGORIES:a,work"));
        // An appointment, and a range of days ending the day after its last.
        assert!(has("DTSTART:20260930T150000"));
        assert!(has("DTEND;VALUE=DATE:20261005"));
        assert!(!ics.contains("Fini"));
        assert_eq!(ics.matches("BEGIN:VTODO").count(), 1);
    }

    #[test]
    fn long_lines_are_folded_and_text_escaped() {
        let title = format!("{}, é;", "x".repeat(80));
        let text = format!("* {title}\n<2026-09-30 Wed>\n");
        let found = entries(&text, Path::new("/n/a.org"), &FileSettings::scan(&text));
        let ics = calendar(&found, "N", "20260928T120000Z");
        assert!(ics.split("\r\n").all(|line| line.len() <= 75), "{ics}");
        let unfolded = ics.replace("\r\n ", "");
        assert!(
            unfolded.contains(&format!("SUMMARY:{}\\, é\\;", "x".repeat(80))),
            "{unfolded}"
        );
    }
}
