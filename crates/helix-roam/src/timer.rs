//! Org's timers: a relative timer counting up from zero, a countdown, and a
//! pomodoro that runs work and breaks in turn.
//!
//! Only arithmetic lives here, over an `Instant` the caller passes, so a test
//! can stand anywhere in time. Ticking the status line and ringing when a
//! countdown ends are the editor's.

use std::time::{Duration, Instant};

/// A pomodoro's lengths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pomodoro {
    pub work: Duration,
    pub short_break: Duration,
    pub long_break: Duration,
    /// Every how many work sessions the break is a long one.
    pub long_break_every: u32,
}

impl Default for Pomodoro {
    fn default() -> Self {
        Self {
            work: Duration::from_secs(25 * 60),
            short_break: Duration::from_secs(5 * 60),
            long_break: Duration::from_secs(15 * 60),
            long_break_every: 4,
        }
    }
}

/// Which part of a pomodoro is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Work,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Work => "work",
            Phase::ShortBreak => "break",
            Phase::LongBreak => "long break",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Counts up, from an offset.
    Relative,
    /// Counts down the length it was set to.
    Countdown(Duration),
    /// A phase of a pomodoro, and how many work sessions are done.
    Pomodoro {
        lengths: Pomodoro,
        phase: Phase,
        done: u32,
    },
}

/// A running, or paused, timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    pub kind: Kind,
    /// When it would have started had it never been paused.
    start: Instant,
    /// Since when it has been paused.
    paused: Option<Instant>,
}

impl Timer {
    /// A relative timer that already reads `offset`.
    pub fn relative(now: Instant, offset: Duration) -> Self {
        Self {
            kind: Kind::Relative,
            start: now.checked_sub(offset).unwrap_or(now),
            paused: None,
        }
    }

    pub fn countdown(now: Instant, length: Duration) -> Self {
        Self {
            kind: Kind::Countdown(length),
            start: now,
            paused: None,
        }
    }

    /// A pomodoro, at the start of its first work session.
    pub fn pomodoro(now: Instant, lengths: Pomodoro) -> Self {
        Self {
            kind: Kind::Pomodoro {
                lengths,
                phase: Phase::Work,
                done: 0,
            },
            start: now,
            paused: None,
        }
    }

    /// Time run, not counting pauses.
    pub fn elapsed(&self, now: Instant) -> Duration {
        self.paused
            .unwrap_or(now)
            .saturating_duration_since(self.start)
    }

    /// What a countdown or a pomodoro's phase runs for.
    pub fn length(&self) -> Option<Duration> {
        match self.kind {
            Kind::Relative => None,
            Kind::Countdown(length) => Some(length),
            Kind::Pomodoro { lengths, phase, .. } => Some(match phase {
                Phase::Work => lengths.work,
                Phase::ShortBreak => lengths.short_break,
                Phase::LongBreak => lengths.long_break,
            }),
        }
    }

    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.length()
            .map(|length| length.saturating_sub(self.elapsed(now)))
    }

    /// Whether a countdown or a phase has run out.
    pub fn expired(&self, now: Instant) -> bool {
        self.remaining(now).is_some_and(|left| left.is_zero())
    }

    pub fn is_paused(&self) -> bool {
        self.paused.is_some()
    }

    pub fn pause(&mut self, now: Instant) {
        self.paused.get_or_insert(now);
    }

    pub fn resume(&mut self, now: Instant) {
        if let Some(paused) = self.paused.take() {
            self.start += now.saturating_duration_since(paused);
        }
    }

    /// A pomodoro's next phase, starting now: a break after work, long
    /// every so many sessions, and work after a break. `None` for the other
    /// timers, which have no next.
    pub fn next_phase(&self, now: Instant) -> Option<Timer> {
        let Kind::Pomodoro {
            lengths,
            phase,
            done,
        } = self.kind
        else {
            return None;
        };
        let (phase, done) = match phase {
            Phase::Work => {
                let done = done + 1;
                let long = lengths.long_break_every > 0 && done % lengths.long_break_every == 0;
                (
                    if long {
                        Phase::LongBreak
                    } else {
                        Phase::ShortBreak
                    },
                    done,
                )
            }
            Phase::ShortBreak | Phase::LongBreak => (Phase::Work, done),
        };
        Some(Timer {
            kind: Kind::Pomodoro {
                lengths,
                phase,
                done,
            },
            start: now,
            paused: None,
        })
    }

    /// The timer as the status line shows it: `0:01:23` counting up,
    /// `-0:04:59` counting down, `work 24:59 (1)` for a pomodoro.
    pub fn display(&self, now: Instant) -> String {
        let mut text = match self.kind {
            Kind::Relative => hms(self.elapsed(now)),
            Kind::Countdown(_) => format!("-{}", hms(self.remaining(now).unwrap_or_default())),
            Kind::Pomodoro { phase, done, .. } => {
                let left = self.remaining(now).unwrap_or_default().as_secs();
                let minutes = left / 60;
                format!("{} {minutes:02}:{:02} ({done})", phase.name(), left % 60)
            }
        };
        if self.is_paused() {
            text.push_str(" paused");
        }
        text
    }
}

/// `H:MM:SS`, the way Org writes a timer.
pub fn hms(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

/// A length as a person types one: `25` (minutes), `1:30` (hours and
/// minutes), `1:30:00`, or units, `1h30m`, `90s`, `25min`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(minutes) = text.parse::<u64>() {
        return Some(Duration::from_secs(minutes * 60));
    }
    if text.contains(':') {
        let parts: Vec<u64> = text
            .split(':')
            .map(|part| part.trim().parse().ok())
            .collect::<Option<_>>()?;
        let secs = match parts.as_slice() {
            [hours, minutes] => hours * 3600 + minutes * 60,
            [hours, minutes, secs] => hours * 3600 + minutes * 60 + secs,
            _ => return None,
        };
        return Some(Duration::from_secs(secs));
    }
    let mut secs = 0;
    let mut number = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return None;
        }
        number.clear();
        number.push_str(&rest[..digits]);
        rest = rest[digits..].trim_start();
        let unit_len = rest.len() - rest.trim_start_matches(|c: char| c.is_alphabetic()).len();
        let unit = &rest[..unit_len];
        rest = rest[unit_len..].trim_start();
        let scale = match unit {
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            _ => return None,
        };
        secs += number.parse::<u64>().ok()? * scale;
    }
    Some(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn a_relative_timer_counts_up_and_pauses() {
        let now = Instant::now();
        let mut timer = Timer::relative(now, Duration::from_secs(10));
        assert_eq!(timer.display(now + MIN), "0:01:10");
        timer.pause(now + MIN);
        assert_eq!(timer.display(now + 5 * MIN), "0:01:10 paused");
        timer.resume(now + 5 * MIN);
        assert_eq!(timer.display(now + 6 * MIN), "0:02:10");
        assert!(!timer.expired(now + 600 * MIN));
    }

    #[test]
    fn a_countdown_runs_out() {
        let now = Instant::now();
        let timer = Timer::countdown(now, 5 * MIN);
        assert_eq!(timer.display(now + Duration::from_secs(1)), "-0:04:59");
        assert!(!timer.expired(now + 4 * MIN));
        assert!(timer.expired(now + 5 * MIN));
        assert_eq!(timer.display(now + 9 * MIN), "-0:00:00");
    }

    #[test]
    fn a_pomodoro_takes_a_long_break_every_fourth() {
        let now = Instant::now();
        let mut timer = Timer::pomodoro(now, Pomodoro::default());
        assert_eq!(timer.display(now), "work 25:00 (0)");
        let mut phases = Vec::new();
        for _ in 0..8 {
            timer = timer.next_phase(now).unwrap();
            let Kind::Pomodoro { phase, .. } = timer.kind else {
                unreachable!()
            };
            phases.push(phase);
        }
        use Phase::*;
        assert_eq!(
            phases,
            [ShortBreak, Work, ShortBreak, Work, ShortBreak, Work, LongBreak, Work]
        );
        assert_eq!(timer.display(now), "work 25:00 (4)");
        assert!(Timer::countdown(now, MIN).next_phase(now).is_none());
    }

    #[test]
    fn durations_read_as_typed() {
        assert_eq!(parse_duration("25"), Some(25 * MIN));
        assert_eq!(parse_duration("1:30"), Some(90 * MIN));
        assert_eq!(parse_duration("0:00:45"), Some(Duration::from_secs(45)));
        assert_eq!(parse_duration("1h30m"), Some(90 * MIN));
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("25 min"), Some(25 * MIN));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(hms(Duration::from_secs(3723)), "1:02:03");
    }
}
