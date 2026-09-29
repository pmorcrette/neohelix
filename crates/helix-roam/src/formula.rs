//! Org table formulas: the `#+TBLFM:` line under a table.
//!
//! Org hands its formulas to Emacs Calc, a computer algebra system; this is
//! the part of that language a table actually uses. Column formulas
//! (`$4=$2*$3`) and field formulas (`@>$4=vsum(@I..@II)`), Org's references
//! (absolute, relative, first and last, hlines, ranges, `@#` and `$#`), the
//! four operations with `^` and `%`, comparisons and `if`, the vector
//! functions (`vsum`, `vmean`, …), scalar and trigonometric ones, and:
//!
//! - dates: a timestamp in a field or a formula is a date; dates subtract
//!   to days and move by days, with `date`, `now`, `year`, `month`, `day`,
//!   `weekday`, `hour`, `minute`, `second`, `incmonth` and `incyear`;
//! - durations: with `;T`, `;t` or `;U`, `12:30` and `12:30:45` are times,
//!   and the result is written back as one (`02:30:00`, `2.50`, `02:30`);
//! - units: `3 m`, `12 km / hr`, kept through arithmetic, converted by
//!   `uconvert`, `ubase`, `uremove`, `uextract` ([`units`]);
//! - symbolic arithmetic: other names are variables, collected and expanded
//!   as polynomials, with `deriv`, `integ`, `subst` and `expand` ([`poly`]);
//! - Emacs Lisp formulas, `'(concat $1 $2)`, for what strings need ([`lisp`]).
//!
//! Not here: named columns and fields, `#+CONSTANTS`, the marking column
//! Org reads from a first column of `!`, `^`, `#` and the like, fractions,
//! and Calc's algebra beyond polynomials. A formula that needs one is
//! reported rather than half evaluated.

mod lisp;
pub mod poly;
pub mod units;

use crate::date::Date;
use crate::table::{cell_text, is_table_line, parse_table, rewrite, strip_leader, Row, Table};
use poly::Poly;
use units::Units;

/// What recalculating a table did.
#[derive(Debug, Clone, PartialEq)]
pub struct Recalculated {
    /// The buffer with the table's fields filled in and realigned.
    pub text: String,
    /// How many formulas the `#+TBLFM:` line holds.
    pub formulas: usize,
    /// How many fields could not be computed and hold `#ERROR`.
    pub errors: usize,
}

/// A row reference: the part after `@`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum RowRef {
    /// `@0`, or no `@` at all.
    Current,
    /// `@3`: the third row, counting rows of cells but not separators.
    Absolute(usize),
    /// `@-1`, `@+2`.
    Relative(isize),
    /// `@<` is the first row, `@<<` the second.
    First(usize),
    /// `@>` is the last row, `@>>` the one before it.
    Last(usize),
    /// `@I`, `@II` (from the top) or `@-I`, `@+I` (from the current row),
    /// optionally moved by a number of rows: `@I+1`.
    Hline {
        direction: Direction,
        nth: usize,
        offset: isize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Direction {
    FromTop,
    Above,
    Below,
}

/// A column reference: the part after `$`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ColRef {
    /// `$0`, or no `$` at all.
    Current,
    /// `$3`.
    Absolute(usize),
    /// `$-1`, `$+2`.
    Relative(isize),
    /// `$<`, `$<<`.
    First(usize),
    /// `$>`, `$>>`.
    Last(usize),
}

/// `@2$3`, `$3` or `@2`: either half may be missing, meaning the current one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Reference {
    row: Option<RowRef>,
    col: Option<ColRef>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Number(f64),
    Field(Reference),
    Range(Reference, Reference),
    /// A unit, a constant (`pi`, `e`) or a variable.
    Name(String),
    Date(Stamp),
    /// `@#`: the current row's number.
    RowNumber,
    /// `$#`: the current column's number.
    ColumnNumber,
    Neg(Box<Expr>),
    Not(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// A date, as Calc keeps one: a count of days, and how it was written.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Stamp {
    /// Days since 1970-01-01, the time of day as the fraction.
    days: f64,
    /// Whether it has a time of day.
    time: bool,
    /// `<…>` rather than `[…]`.
    active: bool,
}

impl Stamp {
    /// `<2026-09-29 Tue>`, `[2026-09-29 Tue 10:30]`, `<2026-09-29>`.
    fn parse(text: &str) -> Option<Stamp> {
        let text = text.trim();
        let (active, inner) = if let Some(inner) = text.strip_prefix('<') {
            (true, inner.strip_suffix('>')?)
        } else {
            (false, text.strip_prefix('[')?.strip_suffix(']')?)
        };
        let mut words = inner.split_whitespace();
        let date = Date::parse_iso(words.next()?)?;
        let mut days = date.to_days() as f64;
        let mut time = false;
        for word in words {
            if let Some((hour, minute)) = word.split_once(':') {
                let (hour, minute): (u32, u32) = (hour.parse().ok()?, minute.parse().ok()?);
                if hour > 23 || minute > 59 {
                    return None;
                }
                days += f64::from(hour * 60 + minute) / 1440.0;
                time = true;
            } else if word.starts_with(['+', '-', '.']) {
                // A repeater or a warning period says nothing of the date.
            } else if !word.chars().all(char::is_alphabetic) {
                return None;
            }
        }
        Some(Stamp { days, time, active })
    }

    fn date(&self) -> Date {
        Date::from_days(self.days.floor() as i64)
    }

    /// Seconds into the day.
    fn seconds(&self) -> i64 {
        ((self.days - self.days.floor()) * 86_400.0).round() as i64
    }

    fn text(&self) -> String {
        let date = self.date();
        let mut inner = format!("{} {}", date.to_iso(), date.weekday());
        if self.time {
            let minutes = self.seconds() / 60;
            inner.push_str(&format!(" {:02}:{:02}", minutes / 60, minutes % 60));
        }
        if self.active {
            format!("<{inner}>")
        } else {
            format!("[{inner}]")
        }
    }
}

/// How a result is written into its field.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Format {
    /// Integers as integers, anything else to eight significant digits, as
    /// Org's Calc settings do.
    Natural,
    /// `%.2f` or `f2`.
    Fixed(usize),
    /// `%d`.
    Integer,
    /// `%.3e`.
    Scientific(usize),
}

/// Where a formula writes.
#[derive(Debug, Clone, PartialEq)]
enum Target {
    /// `$3=…`: every row below the header.
    Column(ColRef),
    /// `@2$3=…`, or a range of fields `@2$1..@4$3=…`.
    Fields(Reference, Option<Reference>),
}

/// How a duration is written back: `;T`, `;U` or `;t`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Duration {
    /// `02:30:00`.
    Clock,
    /// `02:30`.
    Minutes,
    /// `2.50`, in hours.
    Hours,
}

/// What follows `;`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Modes {
    format: Format,
    /// `;N`: fields as numbers, text counting as zero.
    numbers: bool,
    /// `;L`: fields go into a Lisp formula as they are written.
    literal: bool,
    /// `;T`, `;t`, `;U`: `HH:MM[:SS]` fields are times.
    duration: Option<Duration>,
    /// `;R`: angles in radians rather than Calc's degrees.
    radians: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Body {
    Calc(Expr),
    /// The Lisp form, its references not yet filled in.
    Lisp(String),
}

#[derive(Debug, Clone, PartialEq)]
struct Formula {
    source: String,
    target: Target,
    body: Body,
    modes: Modes,
}

// ── Parsing ───────────────────────────────────────────────────────────────

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_space();
        self.pos >= self.chars.len()
    }

    fn run_of(&mut self, c: char) -> usize {
        let mut count = 0;
        while self.eat(c) {
            count += 1;
        }
        count
    }

    fn digits(&mut self) -> Option<usize> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        self.chars[start..self.pos]
            .iter()
            .collect::<String>()
            .parse()
            .ok()
    }

    fn sign(&mut self) -> Option<isize> {
        if self.eat('-') {
            Some(-1)
        } else if self.eat('+') {
            Some(1)
        } else {
            None
        }
    }

    /// The part after `@`.
    fn row(&mut self) -> Result<RowRef, String> {
        match self.peek() {
            Some('<') => Ok(RowRef::First(self.run_of('<') - 1)),
            Some('>') => Ok(RowRef::Last(self.run_of('>') - 1)),
            Some('I') => {
                let nth = self.run_of('I');
                Ok(RowRef::Hline {
                    direction: Direction::FromTop,
                    nth,
                    offset: self.hline_offset(),
                })
            }
            Some('-' | '+') => {
                let sign = self.sign().unwrap_or(1);
                if self.peek() == Some('I') {
                    let nth = self.run_of('I');
                    let direction = if sign < 0 {
                        Direction::Above
                    } else {
                        Direction::Below
                    };
                    Ok(RowRef::Hline {
                        direction,
                        nth,
                        offset: self.hline_offset(),
                    })
                } else {
                    let n = self.digits().ok_or("a number must follow `@-` or `@+`")?;
                    Ok(RowRef::Relative(sign * n as isize))
                }
            }
            Some(c) if c.is_ascii_digit() => match self.digits() {
                Some(0) => Ok(RowRef::Current),
                Some(n) => Ok(RowRef::Absolute(n)),
                None => Err("row number out of range".to_string()),
            },
            _ => Err("expected a row after `@`".to_string()),
        }
    }

    /// The `+1` of `@I+1`, when there is one.
    fn hline_offset(&mut self) -> isize {
        let digit_follows = self.peek_at(1).is_some_and(|c| c.is_ascii_digit());
        if matches!(self.peek(), Some('-' | '+')) && digit_follows {
            let sign = self.sign().unwrap_or(1);
            sign * self.digits().unwrap_or(0) as isize
        } else {
            0
        }
    }

    /// The part after `$`.
    fn col(&mut self) -> Result<ColRef, String> {
        match self.peek() {
            Some('<') => Ok(ColRef::First(self.run_of('<') - 1)),
            Some('>') => Ok(ColRef::Last(self.run_of('>') - 1)),
            Some('-' | '+') => {
                let sign = self.sign().unwrap_or(1);
                let n = self.digits().ok_or("a number must follow `$-` or `$+`")?;
                Ok(ColRef::Relative(sign * n as isize))
            }
            Some(c) if c.is_ascii_digit() => match self.digits() {
                Some(0) => Ok(ColRef::Current),
                Some(n) => Ok(ColRef::Absolute(n)),
                None => Err("column number out of range".to_string()),
            },
            Some(c) if c.is_alphabetic() => {
                Err("named columns and constants are not supported".to_string())
            }
            _ => Err("expected a column after `$`".to_string()),
        }
    }

    fn reference(&mut self) -> Result<Option<Reference>, String> {
        let row = if self.eat('@') {
            Some(self.row()?)
        } else {
            None
        };
        let col = if self.eat('$') {
            Some(self.col()?)
        } else {
            None
        };
        Ok((row.is_some() || col.is_some()).then_some(Reference { row, col }))
    }

    /// A reference or a range of them; `None` when there is neither.
    fn reference_or_range(&mut self) -> Result<Option<(Reference, Option<Reference>)>, String> {
        let Some(from) = self.reference()? else {
            return Ok(None);
        };
        if self.peek() == Some('.') && self.peek_at(1) == Some('.') {
            self.pos += 2;
            let to = self.reference()?.ok_or("a reference must follow `..`")?;
            Ok(Some((from, Some(to))))
        } else {
            Ok(Some((from, None)))
        }
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut left = self.conjunction()?;
        loop {
            self.skip_space();
            if self.peek() == Some('|') && self.peek_at(1) == Some('|') {
                self.pos += 2;
                let right = self.conjunction()?;
                left = Expr::Binary(Op::Or, Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn conjunction(&mut self) -> Result<Expr, String> {
        let mut left = self.negation()?;
        loop {
            self.skip_space();
            if self.peek() == Some('&') && self.peek_at(1) == Some('&') {
                self.pos += 2;
                let right = self.negation()?;
                left = Expr::Binary(Op::And, Box::new(left), Box::new(right));
            } else {
                return Ok(left);
            }
        }
    }

    fn negation(&mut self) -> Result<Expr, String> {
        self.skip_space();
        if self.peek() == Some('!') && self.peek_at(1) != Some('=') {
            self.pos += 1;
            return Ok(Expr::Not(Box::new(self.negation()?)));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        let left = self.sum()?;
        self.skip_space();
        let two = (self.peek(), self.peek_at(1));
        let (op, width) = match two {
            (Some('='), Some('=')) => (Op::Eq, 2),
            (Some('!'), Some('=')) => (Op::Ne, 2),
            (Some('<'), Some('=')) => (Op::Le, 2),
            (Some('>'), Some('=')) => (Op::Ge, 2),
            (Some('<'), _) => (Op::Lt, 1),
            (Some('>'), _) => (Op::Gt, 1),
            _ => return Ok(left),
        };
        self.pos += width;
        let right = self.sum()?;
        Ok(Expr::Binary(op, Box::new(left), Box::new(right)))
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut left = self.term()?;
        loop {
            self.skip_space();
            let op = if self.eat('+') {
                Op::Add
            } else if self.eat('-') {
                Op::Sub
            } else {
                return Ok(left);
            };
            let right = self.term()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn term(&mut self) -> Result<Expr, String> {
        let mut left = self.juxtaposed()?;
        loop {
            self.skip_space();
            let op = if self.eat('*') {
                Op::Mul
            } else if self.eat('/') {
                Op::Div
            } else if self.eat('%') {
                Op::Mod
            } else {
                return Ok(left);
            };
            let right = self.juxtaposed()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    /// Calc's implicit product, which binds tighter than `*` and `/`: so
    /// `12 km / 2 hr` is `(12 km) / (2 hr)`.
    fn juxtaposed(&mut self) -> Result<Expr, String> {
        let mut left = self.unary()?;
        loop {
            self.skip_space();
            let starts_atom = match self.peek() {
                Some(c) if c.is_alphabetic() || c.is_ascii_digit() => true,
                Some('(' | '@' | '$') => true,
                _ => false,
            };
            if !starts_atom {
                return Ok(left);
            }
            let right = self.unary()?;
            left = Expr::Binary(Op::Mul, Box::new(left), Box::new(right));
        }
    }

    /// Unary minus binds looser than `^`, so `-2^2` is `-4`, as in Calc.
    fn unary(&mut self) -> Result<Expr, String> {
        self.skip_space();
        if self.eat('-') {
            Ok(Expr::Neg(Box::new(self.unary()?)))
        } else if self.eat('+') {
            self.unary()
        } else {
            self.power()
        }
    }

    fn power(&mut self) -> Result<Expr, String> {
        let base = self.atom()?;
        self.skip_space();
        if self.eat('^') {
            let exponent = self.unary()?;
            Ok(Expr::Binary(Op::Pow, Box::new(base), Box::new(exponent)))
        } else {
            Ok(base)
        }
    }

    fn atom(&mut self) -> Result<Expr, String> {
        self.skip_space();
        match (self.peek(), self.peek_at(1)) {
            (Some('@'), Some('#')) => {
                self.pos += 2;
                return Ok(Expr::RowNumber);
            }
            (Some('$'), Some('#')) => {
                self.pos += 2;
                return Ok(Expr::ColumnNumber);
            }
            (Some('<' | '['), Some(c)) if c.is_ascii_digit() => {
                let close = if self.peek() == Some('<') { '>' } else { ']' };
                let start = self.pos;
                while self.peek().is_some_and(|c| c != close) {
                    self.pos += 1;
                }
                if !self.eat(close) {
                    return Err("an unclosed date".to_string());
                }
                let text: String = self.chars[start..self.pos].iter().collect();
                return Stamp::parse(&text)
                    .map(Expr::Date)
                    .ok_or_else(|| format!("`{text}` is not a date"));
            }
            _ => {}
        }
        if let Some((from, to)) = self.reference_or_range()? {
            return Ok(match to {
                Some(to) => Expr::Range(from, to),
                None => Expr::Field(from),
            });
        }

        match self.peek() {
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some(c) if c.is_alphabetic() => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    self.pos += 1;
                }
                let name: String = self.chars[start..self.pos].iter().collect();
                self.skip_space();
                if self.eat('(') {
                    let mut args = Vec::new();
                    self.skip_space();
                    if !self.eat(')') {
                        loop {
                            args.push(self.expr()?);
                            self.skip_space();
                            if self.eat(')') {
                                break;
                            }
                            if !self.eat(',') {
                                return Err(format!("expected `,` or `)` in `{name}(…)`"));
                            }
                        }
                    }
                    check_function(&name, args.len())?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Name(name))
                }
            }
            Some('(') => {
                self.pos += 1;
                let inner = self.expr()?;
                self.skip_space();
                if self.eat(')') {
                    Ok(inner)
                } else {
                    Err("missing `)`".to_string())
                }
            }
            Some('\'') => Err("an Emacs Lisp formula must be the whole formula".to_string()),
            Some('"') => Err("strings are not supported; use an Emacs Lisp formula".to_string()),
            Some(c) => Err(format!("unexpected `{c}`")),
            None => Err("the formula ends too early".to_string()),
        }
    }

    fn number(&mut self) -> Result<Expr, String> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == '.') {
            self.pos += 1;
        }
        // An exponent only when a digit follows, so `2e` is not swallowed.
        if matches!(self.peek(), Some('e' | 'E')) {
            let signed = matches!(self.peek_at(1), Some('-' | '+'));
            let digit_at = if signed { 2 } else { 1 };
            if self.peek_at(digit_at).is_some_and(|c| c.is_ascii_digit()) {
                self.pos += digit_at;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.pos += 1;
                }
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        text.parse()
            .map(Expr::Number)
            .map_err(|_| format!("`{text}` is not a number"))
    }
}

/// The functions there are, and how many arguments each takes.
fn check_function(name: &str, args: usize) -> Result<(), String> {
    let (min, max) = match name {
        "vsum" | "vmean" | "vmedian" | "vmin" | "vmax" | "vcount" | "vprod" | "vsdev" | "min"
        | "max" => (1, usize::MAX),
        "abs" | "sqrt" | "exp" | "ln" | "log10" | "floor" | "ceil" | "sin" | "cos" | "tan"
        | "arcsin" | "arccos" | "arctan" | "asin" | "acos" | "atan" => (1, 1),
        "round" => (1, 2),
        "if" => (3, 3),
        "now" => (0, 0),
        "date" => (1, 3),
        "year" | "month" | "day" | "weekday" | "hour" | "minute" | "second" => (1, 1),
        "incmonth" | "incyear" => (2, 2),
        "uconvert" => (2, 2),
        "ubase" | "usimplify" | "uremove" | "uextract" => (1, 1),
        "deriv" | "integ" => (2, 2),
        "subst" => (3, 3),
        "expand" | "simplify" => (1, 1),
        _ => return Err(format!("unknown function `{name}`")),
    };
    if args < min || args > max {
        return Err(format!("wrong number of arguments to `{name}`"));
    }
    Ok(())
}

/// The modes after `;`: `%.2f`, `%d`, `%.3e`, `f2`, `s3`, `N`, `L`, `T`,
/// `t`, `U`, `R`, `D`, and Calc's precision `p20`, which is let be.
fn parse_modes(modes: &str) -> Result<Modes, String> {
    let chars: Vec<char> = modes.trim().chars().collect();
    let mut format = Format::Natural;
    let mut numbers = false;
    let mut literal = false;
    let mut duration = None;
    let mut radians = false;
    let mut i = 0;

    let digits_from = |i: &mut usize| -> Option<usize> {
        let start = *i;
        while chars.get(*i).is_some_and(|c| c.is_ascii_digit()) {
            *i += 1;
        }
        chars[start..*i].iter().collect::<String>().parse().ok()
    };

    while i < chars.len() {
        match chars[i] {
            '%' => {
                i += 1;
                // Flags and width only pad, and aligning the table pads anyway.
                while chars
                    .get(i)
                    .is_some_and(|c| matches!(c, '-' | '+' | ' ' | '#' | '0'))
                {
                    i += 1;
                }
                digits_from(&mut i);
                let precision = if chars.get(i) == Some(&'.') {
                    i += 1;
                    Some(digits_from(&mut i).unwrap_or(0))
                } else {
                    None
                };
                format = match chars.get(i) {
                    Some('f') => Format::Fixed(precision.unwrap_or(6)),
                    Some('d') => Format::Integer,
                    Some('e') => Format::Scientific(precision.unwrap_or(6)),
                    _ => return Err(format!("unsupported format `{modes}`")),
                };
                i += 1;
            }
            'f' if chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => {
                i += 1;
                format = Format::Fixed(digits_from(&mut i).unwrap_or(0));
            }
            's' | 'e' if chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => {
                i += 1;
                let digits = digits_from(&mut i).unwrap_or(1);
                format = Format::Scientific(digits.saturating_sub(1));
            }
            'p' if chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) => {
                i += 1;
                digits_from(&mut i);
            }
            'N' => {
                numbers = true;
                i += 1;
            }
            'L' => {
                literal = true;
                i += 1;
            }
            'T' | 't' | 'U' => {
                duration = Some(match chars[i] {
                    'T' => Duration::Clock,
                    't' => Duration::Hours,
                    _ => Duration::Minutes,
                });
                i += 1;
            }
            'R' | 'D' => {
                radians = chars[i] == 'R';
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            c => return Err(format!("unsupported mode `{c}`")),
        }
    }
    Ok(Modes {
        format,
        numbers,
        literal,
        duration,
        radians,
    })
}

/// Reads one `lhs=rhs;modes` formula.
fn parse_formula(source: &str) -> Result<Formula, String> {
    // The modes follow the last `;`, which a Lisp string may also hold.
    let (assignment, modes) = match source.rsplit_once(';') {
        Some((assignment, modes)) if !modes.contains(['"', ')', '(']) => (assignment, modes),
        _ => (source, ""),
    };
    let (lhs, rhs) = assignment.split_once('=').ok_or("a formula needs `=`")?;
    let modes = parse_modes(modes)?;

    let mut left = Parser::new(lhs.trim());
    let Some((from, to)) = left.reference_or_range()? else {
        return Err("the left side must be a field or a column".to_string());
    };
    if !left.at_end() {
        return Err("the left side must be a field or a column".to_string());
    }
    let target = match (from, to) {
        (
            Reference {
                row: None,
                col: Some(col @ (ColRef::Absolute(_) | ColRef::First(_) | ColRef::Last(_))),
            },
            None,
        ) => Target::Column(col),
        (Reference { row: None, .. }, _) => {
            return Err("the left side must name an absolute column".to_string())
        }
        (from, to) => {
            for field in std::iter::once(from).chain(to) {
                let fixed_row = matches!(
                    field.row,
                    Some(
                        RowRef::Absolute(_)
                            | RowRef::First(_)
                            | RowRef::Last(_)
                            | RowRef::Hline {
                                direction: Direction::FromTop,
                                ..
                            }
                    )
                );
                let fixed_col = matches!(
                    field.col,
                    Some(ColRef::Absolute(_) | ColRef::First(_) | ColRef::Last(_))
                );
                if !fixed_row || !fixed_col {
                    return Err("the left side must name its row and column".to_string());
                }
            }
            Target::Fields(from, to)
        }
    };

    let body = match rhs.trim().strip_prefix('\'') {
        Some(form) => Body::Lisp(form.to_string()),
        None => {
            let mut right = Parser::new(rhs);
            let expr = right.expr()?;
            if !right.at_end() {
                let rest: String = right.chars[right.pos..].iter().collect();
                return Err(format!("unexpected `{}`", rest.trim()));
            }
            Body::Calc(expr)
        }
    };

    Ok(Formula {
        source: source.trim().to_string(),
        target,
        body,
        modes,
    })
}

/// Splits a `#+TBLFM:` line into its formulas.
fn parse_tblfm(line: &str) -> Result<Vec<Formula>, String> {
    let body = tblfm_body(line).unwrap_or("");
    body.split("::")
        .map(str::trim)
        .filter(|formula| !formula.is_empty())
        .map(|formula| parse_formula(formula).map_err(|error| format!("`{formula}`: {error}")))
        .collect()
}

/// The text after `#+TBLFM:`, when the line is one.
fn tblfm_body(line: &str) -> Option<&str> {
    let trimmed = strip_leader(line);
    let keyword = trimmed.get(..8)?;
    keyword
        .eq_ignore_ascii_case("#+TBLFM:")
        .then(|| &trimmed[8..])
}

// ── Evaluation ────────────────────────────────────────────────────────────

/// The table being computed: its rows with every cell present, and where
/// its rows of cells and its separators are.
struct Grid {
    rows: Vec<Row>,
    /// Indices into `rows` of the rows of cells.
    data: Vec<usize>,
    /// Indices into `rows` of the separators.
    hlines: Vec<usize>,
    columns: usize,
}

impl Grid {
    fn new(table: &Table) -> Self {
        let columns = table.columns();
        let rows: Vec<Row> = table
            .rows
            .iter()
            .map(|row| match row {
                Row::Cells(cells) => {
                    let mut cells = cells.clone();
                    cells.resize(columns, String::new());
                    Row::Cells(cells)
                }
                Row::Separator => Row::Separator,
            })
            .collect();
        let data = (0..rows.len())
            .filter(|&i| matches!(rows[i], Row::Cells(_)))
            .collect();
        let hlines = (0..rows.len())
            .filter(|&i| matches!(rows[i], Row::Separator))
            .collect();
        Self {
            rows,
            data,
            hlines,
            columns,
        }
    }

    /// The rows column formulas fill: everything below the first separator,
    /// or every row when there is none (or nothing below it).
    fn body(&self) -> Vec<usize> {
        let below: Vec<usize> = match self.hlines.first() {
            Some(&first) => self.data.iter().copied().filter(|&r| r > first).collect(),
            None => Vec::new(),
        };
        if below.is_empty() {
            self.data.clone()
        } else {
            below
        }
    }

    fn cell(&self, row: usize, col: usize) -> &str {
        match &self.rows[row] {
            Row::Cells(cells) => cells.get(col).map(String::as_str).unwrap_or(""),
            Row::Separator => "",
        }
    }

    fn set(&mut self, row: usize, col: usize, value: String) {
        if let Row::Cells(cells) = &mut self.rows[row] {
            cells[col] = value;
        }
    }

    /// Resolves a row reference from `current`, to an index into `rows`.
    /// It may land on a separator: only a range bound may.
    fn resolve_row(&self, row: Option<RowRef>, current: usize) -> Result<usize, String> {
        let nth_data = |n: usize| {
            self.data
                .get(n)
                .copied()
                .ok_or_else(|| "row reference outside the table".to_string())
        };
        match row.unwrap_or(RowRef::Current) {
            RowRef::Current => Ok(current),
            RowRef::Absolute(n) => nth_data(n - 1),
            RowRef::First(k) => nth_data(k),
            RowRef::Last(k) => self
                .data
                .len()
                .checked_sub(k + 1)
                .map(|at| self.data[at])
                .ok_or_else(|| "row reference outside the table".to_string()),
            RowRef::Relative(k) => {
                let at = self
                    .data
                    .iter()
                    .position(|&r| r == current)
                    .ok_or("relative row from a separator")?;
                let wanted = at as isize + k;
                usize::try_from(wanted)
                    .ok()
                    .and_then(|wanted| self.data.get(wanted).copied())
                    .ok_or_else(|| "row reference outside the table".to_string())
            }
            RowRef::Hline {
                direction,
                nth,
                offset,
            } => {
                let hline = match direction {
                    Direction::FromTop => self.hlines.get(nth - 1).copied(),
                    Direction::Above => self
                        .hlines
                        .iter()
                        .rev()
                        .filter(|&&h| h < current)
                        .nth(nth - 1)
                        .copied(),
                    Direction::Below => self
                        .hlines
                        .iter()
                        .filter(|&&h| h > current)
                        .nth(nth - 1)
                        .copied(),
                }
                .ok_or("no such separator")?;
                if offset == 0 {
                    return Ok(hline);
                }
                // Counted in rows of cells from the separator, which is
                // what `@I+1` means: the first row under it.
                let found = if offset > 0 {
                    self.data
                        .iter()
                        .filter(|&&r| r > hline)
                        .nth(offset as usize - 1)
                } else {
                    self.data
                        .iter()
                        .rev()
                        .filter(|&&r| r < hline)
                        .nth(offset.unsigned_abs() - 1)
                };
                found
                    .copied()
                    .ok_or_else(|| "row reference outside the table".to_string())
            }
        }
    }

    fn resolve_col(&self, col: Option<ColRef>, current: usize) -> Result<usize, String> {
        let wanted = match col.unwrap_or(ColRef::Current) {
            ColRef::Current => Some(current),
            ColRef::Absolute(n) => Some(n - 1),
            ColRef::First(k) => Some(k),
            ColRef::Last(k) => self.columns.checked_sub(k + 1),
            ColRef::Relative(k) => usize::try_from(current as isize + k).ok(),
        };
        wanted
            .filter(|&c| c < self.columns)
            .ok_or_else(|| "column reference outside the table".to_string())
    }

    /// Every field a range covers, row by row, skipping separators.
    fn range(
        &self,
        from: Reference,
        to: Reference,
        at: (usize, usize),
    ) -> Result<Vec<(usize, usize)>, String> {
        let (r1, r2) = (
            self.resolve_row(from.row, at.0)?,
            self.resolve_row(to.row, at.0)?,
        );
        let (c1, c2) = (
            self.resolve_col(from.col, at.1)?,
            self.resolve_col(to.col, at.1)?,
        );
        let (r1, r2) = (r1.min(r2), r1.max(r2));
        let (c1, c2) = (c1.min(c2), c1.max(c2));
        Ok(self
            .data
            .iter()
            .filter(|&&r| r >= r1 && r <= r2)
            .flat_map(|&r| (c1..=c2).map(move |c| (r, c)))
            .collect())
    }

    fn field(&self, reference: Reference, at: (usize, usize)) -> Result<(usize, usize), String> {
        let row = self.resolve_row(reference.row, at.0)?;
        if matches!(self.rows[row], Row::Separator) {
            return Err("a separator is not a field".to_string());
        }
        Ok((row, self.resolve_col(reference.col, at.1)?))
    }
}

/// A number as Org writes one in a table, or `None`.
fn read_number(text: &str) -> Option<f64> {
    let text = text.trim();
    let plausible = text.chars().any(|c| c.is_ascii_digit())
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'));
    plausible.then(|| text.parse().ok()).flatten()
}

/// What a formula computes.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    /// A number, with units or without.
    Quantity(f64, Units),
    Date(Stamp),
    Symbolic(Poly),
    List(Vec<Value>),
}

fn number(n: f64) -> Value {
    Value::Quantity(n, Units::default())
}

fn truth(holds: bool) -> Value {
    number(if holds { 1.0 } else { 0.0 })
}

/// A polynomial without variables is a number again.
fn settle(poly: Poly) -> Value {
    match poly.as_constant() {
        Some(n) => number(n),
        None => Value::Symbolic(poly),
    }
}

impl Value {
    /// The number, when this is a plain one.
    fn plain(&self) -> Option<f64> {
        match self {
            Value::Quantity(n, units) if units.is_empty() => Some(*n),
            _ => None,
        }
    }

    fn describe(&self) -> &'static str {
        match self {
            Value::Quantity(_, units) if units.is_empty() => "a number",
            Value::Quantity(..) => "a quantity with units",
            Value::Date(_) => "a date",
            Value::Symbolic(_) => "a symbolic expression",
            Value::List(_) => "a range",
        }
    }

    fn poly(&self) -> Result<Poly, String> {
        match self {
            Value::Symbolic(poly) => Ok(poly.clone()),
            other => match other.plain() {
                Some(n) => Ok(Poly::constant(n)),
                None => Err(format!(
                    "{} cannot go into symbolic arithmetic",
                    other.describe()
                )),
            },
        }
    }

    /// A number to compare by: a quantity in SI, a date in days.
    fn rank(&self) -> Result<(f64, units::Dims, bool), String> {
        match self {
            Value::Quantity(n, units) => {
                let (factor, dims) = units.si();
                Ok((n * factor, dims, false))
            }
            Value::Date(stamp) => Ok((stamp.days, [0; 7], true)),
            other => Err(format!("{} cannot be compared", other.describe())),
        }
    }
}

fn compare(a: &Value, b: &Value) -> Result<std::cmp::Ordering, String> {
    let (a, a_dims, a_date) = a.rank()?;
    let (b, b_dims, b_date) = b.rank()?;
    if a_dims != b_dims || a_date != b_date {
        return Err("comparing things that are not alike".to_string());
    }
    Ok(a.total_cmp(&b))
}

/// Two values under an operator.
fn binary(op: Op, a: Value, b: Value) -> Result<Value, String> {
    if matches!(a, Value::List(_)) || matches!(b, Value::List(_)) {
        return Err("a range can only be used inside a function".to_string());
    }
    match op {
        Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            if let (Value::Symbolic(x), Value::Symbolic(y)) = (&a, &b) {
                return match op {
                    Op::Eq => Ok(truth(x == y)),
                    Op::Ne => Ok(truth(x != y)),
                    _ => Err("symbolic expressions cannot be ordered".to_string()),
                };
            }
            let order = compare(&a, &b)?;
            use std::cmp::Ordering::*;
            return Ok(truth(match op {
                Op::Eq => order == Equal,
                Op::Ne => order != Equal,
                Op::Lt => order == Less,
                Op::Le => order != Greater,
                Op::Gt => order == Greater,
                _ => order != Less,
            }));
        }
        Op::And | Op::Or => {
            let (x, y) = (truthy(&a)?, truthy(&b)?);
            return Ok(truth(if op == Op::And { x && y } else { x || y }));
        }
        _ => {}
    }

    // Dates: a difference is days, and days move a date.
    match (&a, &b) {
        (Value::Date(x), Value::Date(y)) if op == Op::Sub => return Ok(number(x.days - y.days)),
        (Value::Date(stamp), other) | (other, Value::Date(stamp))
            if op == Op::Add || (op == Op::Sub && matches!(a, Value::Date(_))) =>
        {
            let days = days_of(other)?;
            let days = if op == Op::Sub { -days } else { days };
            let mut moved = *stamp;
            moved.days = within_calendar(moved.days + days)?;
            // A move by part of a day gives the date a time.
            moved.time |= days.fract() != 0.0;
            return Ok(Value::Date(moved));
        }
        (Value::Date(_), _) | (_, Value::Date(_)) => {
            return Err("dates only subtract, and move by days".to_string())
        }
        _ => {}
    }

    if matches!(a, Value::Symbolic(_)) || matches!(b, Value::Symbolic(_)) {
        let (x, y) = (a.poly()?, b.poly()?);
        return match op {
            Op::Add => Ok(settle(x.add(&y))),
            Op::Sub => Ok(settle(x.add(&y.scale(-1.0)))),
            Op::Mul => Ok(settle(x.mul(&y))),
            Op::Div => match y.as_constant() {
                Some(0.0) => Err("division by zero".to_string()),
                Some(n) => Ok(settle(x.scale(1.0 / n))),
                None => Err("division by a symbolic expression is not supported".to_string()),
            },
            Op::Pow => match y.as_constant() {
                Some(n) if n >= 0.0 && n.fract() == 0.0 && n <= 64.0 => Ok(settle(x.pow(n as u32))),
                _ => Err("a symbolic power must be a whole number".to_string()),
            },
            _ => Err("`%` of a symbolic expression".to_string()),
        };
    }

    let (Value::Quantity(x, xu), Value::Quantity(y, yu)) = (a, b) else {
        unreachable!("dates, lists and symbols are handled above");
    };
    match op {
        Op::Add | Op::Sub | Op::Mod => {
            // A plain zero (an empty field) joins any units.
            let (x, y, units) = if xu == yu || (yu.is_empty() && y == 0.0) {
                (x, y, xu)
            } else if xu.is_empty() && x == 0.0 {
                (x, y, yu)
            } else {
                let factor = units::conversion(&yu, &xu)?;
                (x, y * factor, xu)
            };
            let value = match op {
                Op::Add => x + y,
                Op::Sub => x - y,
                _ if y == 0.0 => return Err("division by zero".to_string()),
                // Calc's `%` takes the sign of the divisor.
                _ => x - y * (x / y).floor(),
            };
            Ok(Value::Quantity(value, units))
        }
        Op::Mul => {
            let (units, scale) = xu.times(&yu);
            Ok(Value::Quantity(x * y * scale, units))
        }
        Op::Div => {
            if y == 0.0 {
                return Err("division by zero".to_string());
            }
            let (units, scale) = xu.times(&yu.powi(-1));
            Ok(Value::Quantity(x / y * scale, units))
        }
        Op::Pow => {
            if !yu.is_empty() {
                return Err("a power cannot have units".to_string());
            }
            if xu.is_empty() {
                return Ok(number(x.powf(y)));
            }
            if y.fract() != 0.0 {
                return Err("units to a power that is not whole".to_string());
            }
            Ok(Value::Quantity(x.powf(y), xu.powi(y as i32)))
        }
        _ => unreachable!("comparisons are handled above"),
    }
}

fn truthy(value: &Value) -> Result<bool, String> {
    value
        .plain()
        .map(|n| n != 0.0)
        .ok_or_else(|| format!("{} is neither true nor false", value.describe()))
}

/// A day count a date can have: some thousands of years either way, where
/// the calendar arithmetic holds.
fn within_calendar(days: f64) -> Result<f64, String> {
    if days.is_finite() && days.abs() <= 3_000_000.0 {
        Ok(days)
    } else {
        Err("a date out of the calendar's range".to_string())
    }
}

/// How far a number or a time moves a date, in days.
fn days_of(value: &Value) -> Result<f64, String> {
    match value {
        Value::Quantity(n, units) if units.is_empty() => Ok(*n),
        Value::Quantity(n, units) => Ok(n * units::conversion(units, &Units::one("day"))?),
        other => Err(format!("a date cannot move by {}", other.describe())),
    }
}

/// `12:30` or `12:30:45` in seconds, for the duration modes.
fn read_duration(text: &str) -> Option<f64> {
    let text = text.trim();
    let (negative, text) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let parts: Vec<&str> = text.split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let numbers: Vec<u64> = parts
        .iter()
        .map(|p| p.parse().ok())
        .collect::<Option<_>>()?;
    let seconds = numbers[0] * 3600 + numbers[1] * 60 + numbers.get(2).copied().unwrap_or(0);
    Some(if negative {
        -(seconds as f64)
    } else {
        seconds as f64
    })
}

/// The number at the start of `text`, or zero: what `;N` makes of a field.
fn leading_number(text: &str) -> f64 {
    let text = text.trim();
    let end = text
        .char_indices()
        .take_while(|(at, c)| {
            c.is_ascii_digit()
                || matches!(c, '.' | 'e' | 'E')
                || (*at == 0 && matches!(c, '-' | '+'))
        })
        .map(|(at, c)| at + c.len_utf8())
        .last()
        .unwrap_or(0);
    (0..=end)
        .rev()
        .find_map(|end| text[..end].parse::<f64>().ok())
        .unwrap_or(0.0)
}

struct Eval<'a> {
    grid: &'a Grid,
    at: (usize, usize),
    modes: Modes,
}

impl Eval<'_> {
    /// A field's text as a value: a date, a time in the duration modes, a
    /// number, or else what Calc would read it as (`3 m`, `x + 1`).
    fn value_of(&self, text: &str) -> Result<Value, String> {
        let trimmed = text.trim();
        if self.modes.numbers {
            return Ok(number(leading_number(trimmed)));
        }
        if let Some(seconds) = self.modes.duration.and_then(|_| read_duration(trimmed)) {
            return Ok(number(seconds));
        }
        if let Some(stamp) = Stamp::parse(trimmed) {
            return Ok(Value::Date(stamp));
        }
        if let Some(n) = read_number(trimmed) {
            return Ok(number(n));
        }
        let not_a_number = || format!("`{trimmed}` is not a number");
        // A field holds no references; one that seems to is text.
        if trimmed.contains(['@', '$', '\'', '"']) {
            return Err(not_a_number());
        }
        let mut parser = Parser::new(trimmed);
        let expr = parser.expr().map_err(|_| not_a_number())?;
        if !parser.at_end() {
            return Err(not_a_number());
        }
        self.eval(&expr).map_err(|_| not_a_number())
    }

    fn eval(&self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::Number(n) => Ok(number(*n)),
            Expr::Date(stamp) => Ok(Value::Date(*stamp)),
            Expr::RowNumber => {
                let row = self.grid.data.iter().position(|&r| r == self.at.0);
                Ok(number(row.map_or(0, |at| at + 1) as f64))
            }
            Expr::ColumnNumber => Ok(number((self.at.1 + 1) as f64)),
            Expr::Name(name) => Ok(match name.as_str() {
                "pi" => number(std::f64::consts::PI),
                "e" => number(std::f64::consts::E),
                name if units::lookup(name).is_some() => Value::Quantity(1.0, Units::one(name)),
                name => Value::Symbolic(Poly::variable(name)),
            }),
            Expr::Field(reference) => {
                let (row, col) = self.grid.field(*reference, self.at)?;
                let text = self.grid.cell(row, col);
                // A lone empty field is zero, as Org has it.
                if text.trim().is_empty() {
                    Ok(number(0.0))
                } else {
                    self.value_of(text)
                }
            }
            Expr::Range(from, to) => {
                // Empty fields drop out of a range, so `vmean` averages what
                // is there.
                let mut values = Vec::new();
                for (row, col) in self.grid.range(*from, *to, self.at)? {
                    let text = self.grid.cell(row, col);
                    if !text.trim().is_empty() {
                        values.push(self.value_of(text)?);
                    }
                }
                Ok(Value::List(values))
            }
            Expr::Neg(inner) => binary(Op::Mul, number(-1.0), self.eval(inner)?),
            Expr::Not(inner) => Ok(truth(!truthy(&self.eval(inner)?)?)),
            Expr::Binary(op, left, right) => binary(*op, self.eval(left)?, self.eval(right)?),
            Expr::Call(name, args) => self.call(name, args),
        }
    }

    /// Every argument, ranges spread out.
    fn spread(&self, args: &[Expr]) -> Result<Vec<Value>, String> {
        let mut values = Vec::new();
        for arg in args {
            match self.eval(arg)? {
                Value::List(list) => values.extend(list),
                value => values.push(value),
            }
        }
        Ok(values)
    }

    fn plain(&self, expr: &Expr, name: &str) -> Result<f64, String> {
        let value = self.eval(expr)?;
        value
            .plain()
            .ok_or_else(|| format!("`{name}` of {}", value.describe()))
    }

    fn date(&self, expr: &Expr, name: &str) -> Result<Stamp, String> {
        match self.eval(expr)? {
            Value::Date(stamp) => Ok(stamp),
            other => Err(format!("`{name}` of {}", other.describe())),
        }
    }

    fn call(&self, name: &str, args: &[Expr]) -> Result<Value, String> {
        let nonempty = |values: Vec<Value>| -> Result<Vec<Value>, String> {
            if values.is_empty() {
                Err(format!("`{name}` of nothing"))
            } else {
                Ok(values)
            }
        };
        let angle = |n: f64| {
            if self.modes.radians {
                n
            } else {
                n.to_radians()
            }
        };
        let from_angle = |n: f64| {
            if self.modes.radians {
                n
            } else {
                n.to_degrees()
            }
        };
        // Calc's functions keep a number's units where that makes sense.
        let keep_units = |f: fn(f64) -> f64| -> Result<Value, String> {
            match self.eval(&args[0])? {
                Value::Quantity(n, units) => Ok(Value::Quantity(f(n), units)),
                other => Err(format!("`{name}` of {}", other.describe())),
            }
        };

        match name {
            "if" => {
                let test = self.eval(&args[0])?;
                if truthy(&test)? {
                    self.eval(&args[1])
                } else {
                    self.eval(&args[2])
                }
            }
            "vsum" => self
                .spread(args)?
                .into_iter()
                .try_fold(number(0.0), |acc, v| binary(Op::Add, acc, v)),
            "vprod" => self
                .spread(args)?
                .into_iter()
                .try_fold(number(1.0), |acc, v| binary(Op::Mul, acc, v)),
            "vcount" => Ok(number(self.spread(args)?.len() as f64)),
            "vmean" => {
                let values = nonempty(self.spread(args)?)?;
                let count = values.len() as f64;
                let sum = values
                    .into_iter()
                    .try_fold(number(0.0), |acc, v| binary(Op::Add, acc, v))?;
                binary(Op::Div, sum, number(count))
            }
            "vmedian" => {
                let mut values = nonempty(self.spread(args)?)?;
                let mut failed = None;
                values.sort_by(|a, b| {
                    compare(a, b).unwrap_or_else(|error| {
                        failed = Some(error);
                        std::cmp::Ordering::Equal
                    })
                });
                if let Some(error) = failed {
                    return Err(error);
                }
                let mid = values.len() / 2;
                if values.len() % 2 == 0 {
                    let sum = binary(Op::Add, values[mid - 1].clone(), values[mid].clone())?;
                    binary(Op::Div, sum, number(2.0))
                } else {
                    Ok(values[mid].clone())
                }
            }
            "vsdev" => {
                // The sample standard deviation, which is Calc's.
                let values = self.spread(args)?;
                if values.len() < 2 {
                    return Err("`vsdev` needs two values".to_string());
                }
                let count = values.len() as f64;
                let mean = binary(
                    Op::Div,
                    values
                        .iter()
                        .cloned()
                        .try_fold(number(0.0), |acc, v| binary(Op::Add, acc, v))?,
                    number(count),
                )?;
                let mut squares = number(0.0);
                for value in values {
                    let deviation = binary(Op::Sub, value, mean.clone())?;
                    squares = binary(Op::Add, squares, binary(Op::Pow, deviation, number(2.0))?)?;
                }
                let variance = binary(Op::Div, squares, number(count - 1.0))?;
                sqrt(variance)
            }
            "vmin" | "min" | "vmax" | "max" => {
                let values = nonempty(self.spread(args)?)?;
                let want = if name.ends_with("min") {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
                let mut best = values[0].clone();
                for value in values.into_iter().skip(1) {
                    if compare(&value, &best)? == want {
                        best = value;
                    }
                }
                Ok(best)
            }
            "abs" => keep_units(f64::abs),
            "floor" => keep_units(f64::floor),
            "ceil" => keep_units(f64::ceil),
            "round" => {
                let places = match args.get(1) {
                    Some(places) => self.plain(places, name)?,
                    None => 0.0,
                };
                let scale = 10f64.powi(places as i32);
                match self.eval(&args[0])? {
                    Value::Quantity(n, units) => {
                        Ok(Value::Quantity((n * scale).round() / scale, units))
                    }
                    other => Err(format!("`round` of {}", other.describe())),
                }
            }
            "sqrt" => sqrt(self.eval(&args[0])?),
            "exp" => Ok(number(self.plain(&args[0], name)?.exp())),
            "ln" | "log10" => {
                let n = self.plain(&args[0], name)?;
                if n <= 0.0 {
                    return Err("logarithm of a number that is not positive".to_string());
                }
                Ok(number(if name == "ln" { n.ln() } else { n.log10() }))
            }
            "sin" => Ok(number(angle(self.plain(&args[0], name)?).sin())),
            "cos" => Ok(number(angle(self.plain(&args[0], name)?).cos())),
            "tan" => Ok(number(angle(self.plain(&args[0], name)?).tan())),
            "arcsin" | "asin" => Ok(number(from_angle(self.plain(&args[0], name)?.asin()))),
            "arccos" | "acos" => Ok(number(from_angle(self.plain(&args[0], name)?.acos()))),
            "arctan" | "atan" => Ok(number(from_angle(self.plain(&args[0], name)?.atan()))),

            // Dates.
            "now" => {
                let seconds = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_secs())
                    .unwrap_or(0);
                // To the minute, as a timestamp shows it.
                let minutes = seconds / 60;
                Ok(Value::Date(Stamp {
                    days: minutes as f64 / 1440.0,
                    time: true,
                    active: true,
                }))
            }
            "date" => match args {
                [only] => match self.eval(only)? {
                    Value::Date(stamp) => Ok(Value::Date(stamp)),
                    other => match other.plain() {
                        // Calc's day numbers count from the year 1.
                        Some(n) => Ok(Value::Date(Stamp {
                            days: within_calendar(n - 719_163.0)?,
                            time: n.fract() != 0.0,
                            active: true,
                        })),
                        None => Err(format!("`date` of {}", other.describe())),
                    },
                },
                [year, month, day] => {
                    let year = self.plain(year, name)?;
                    if !(-9999.0..=9999.0).contains(&year) {
                        return Err("a date out of the calendar's range".to_string());
                    }
                    let date = Date {
                        year: year as i32,
                        month: self.plain(month, name)? as u32,
                        day: 1,
                    };
                    // The day counts on from the first, so `date(2026, 1, 32)`
                    // is the first of February.
                    let days =
                        within_calendar(date.to_days() as f64 + self.plain(day, name)? - 1.0)?;
                    Ok(Value::Date(Stamp {
                        days,
                        time: false,
                        active: true,
                    }))
                }
                _ => Err("`date` takes a date, a day number or a year, month and day".to_string()),
            },
            "year" => Ok(number(f64::from(self.date(&args[0], name)?.date().year))),
            "month" => Ok(number(f64::from(self.date(&args[0], name)?.date().month))),
            "day" => Ok(number(f64::from(self.date(&args[0], name)?.date().day))),
            "weekday" => {
                // Calc counts from Sunday, 0.
                let days = self.date(&args[0], name)?.days.floor() as i64;
                Ok(number((days + 4).rem_euclid(7) as f64))
            }
            "hour" => Ok(number((self.date(&args[0], name)?.seconds() / 3600) as f64)),
            "minute" => Ok(number(
                (self.date(&args[0], name)?.seconds() / 60 % 60) as f64,
            )),
            "second" => Ok(number((self.date(&args[0], name)?.seconds() % 60) as f64)),
            "incmonth" | "incyear" => {
                let stamp = self.date(&args[0], name)?;
                let by = self.plain(&args[1], name)?;
                if by.abs() > 100_000.0 {
                    return Err("a date out of the calendar's range".to_string());
                }
                let by = by as i64;
                let months = if name == "incyear" { by * 12 } else { by };
                let date = stamp.date();
                let total = i64::from(date.year) * 12 + i64::from(date.month) - 1 + months;
                let (year, month) = (total.div_euclid(12) as i32, total.rem_euclid(12) as u32 + 1);
                // The same day, or the month's last when it has no such day.
                let last = (28..=31)
                    .rev()
                    .find(|&day| {
                        Date::parse_iso(&format!("{year:04}-{month:02}-{day:02}")).is_some()
                    })
                    .unwrap_or(28);
                let moved = Date {
                    year,
                    month,
                    day: date.day.min(last),
                };
                let mut stamp = stamp;
                stamp.days = moved.to_days() as f64 + (stamp.days - stamp.days.floor());
                Ok(Value::Date(stamp))
            }

            // Units.
            "uconvert" => {
                let Value::Quantity(n, from) = self.eval(&args[0])? else {
                    return Err("`uconvert` of something that is not a quantity".to_string());
                };
                let Value::Quantity(_, to) = self.eval(&args[1])? else {
                    return Err("`uconvert` needs units to convert to".to_string());
                };
                Ok(Value::Quantity(n * units::conversion(&from, &to)?, to))
            }
            "ubase" => match self.eval(&args[0])? {
                Value::Quantity(n, units) => {
                    let (factor, dims) = units.si();
                    Ok(Value::Quantity(n * factor, units::base_units(dims)))
                }
                other => Err(format!("`ubase` of {}", other.describe())),
            },
            "usimplify" => self.eval(&args[0]),
            "uremove" => match self.eval(&args[0])? {
                Value::Quantity(n, _) => Ok(number(n)),
                other => Err(format!("`uremove` of {}", other.describe())),
            },
            "uextract" => match self.eval(&args[0])? {
                Value::Quantity(_, units) => Ok(Value::Quantity(1.0, units)),
                other => Err(format!("`uextract` of {}", other.describe())),
            },

            // Symbolic.
            "expand" | "simplify" => self.eval(&args[0]),
            "deriv" | "integ" => {
                let poly = self.eval(&args[0])?.poly()?;
                let variable = self.variable(&args[1], name)?;
                Ok(settle(if name == "deriv" {
                    poly.derivative(&variable)
                } else {
                    poly.integral(&variable)
                }))
            }
            "subst" => {
                let poly = self.eval(&args[0])?.poly()?;
                let variable = self.variable(&args[1], name)?;
                let by = self.eval(&args[2])?.poly()?;
                Ok(settle(poly.substitute(&variable, &by)))
            }
            _ => Err(format!("unknown function `{name}`")),
        }
    }

    /// The variable an argument names, for `deriv` and the like.
    fn variable(&self, expr: &Expr, name: &str) -> Result<String, String> {
        match expr {
            Expr::Name(variable) => Ok(variable.clone()),
            _ => Err(format!("`{name}` needs a variable")),
        }
    }
}

fn sqrt(value: Value) -> Result<Value, String> {
    match value {
        Value::Quantity(n, _) if n < 0.0 => Err("square root of a negative number".to_string()),
        Value::Quantity(n, units) => {
            if units.0.iter().any(|(_, power)| power % 2 != 0) {
                return Err("square root of units to an odd power".to_string());
            }
            Ok(Value::Quantity(
                n.sqrt(),
                Units(
                    units
                        .0
                        .into_iter()
                        .map(|(name, power)| (name, power / 2))
                        .collect(),
                ),
            ))
        }
        other => Err(format!("square root of {}", other.describe())),
    }
}

/// Writes a result the way `format` asks.
fn render(value: f64, format: Format) -> String {
    if !value.is_finite() {
        return "#ERROR".to_string();
    }
    match format {
        Format::Fixed(places) => format!("{value:.places$}"),
        Format::Integer => format!("{}", value.trunc() as i64),
        Format::Scientific(places) => {
            let formatted = format!("{value:.places$e}");
            // C writes `1.50e+03` where Rust writes `1.50e3`.
            match formatted.split_once('e') {
                Some((mantissa, exponent)) => {
                    let exponent: i32 = exponent.parse().unwrap_or(0);
                    let sign = if exponent < 0 { '-' } else { '+' };
                    format!("{mantissa}e{sign}{:02}", exponent.abs())
                }
                None => formatted,
            }
        }
        Format::Natural => {
            if value == value.trunc() && value.abs() < 1e15 {
                return format!("{}", value as i64);
            }
            // Eight significant digits, trailing zeros dropped.
            let exponent = value.abs().log10().floor() as i32;
            if (-5..8).contains(&exponent) {
                let places = (7 - exponent).max(0) as usize;
                let fixed = format!("{value:.places$}");
                let fixed = if fixed.contains('.') {
                    fixed.trim_end_matches('0').trim_end_matches('.')
                } else {
                    &fixed
                };
                fixed.to_string()
            } else {
                let formatted = format!("{value:.7e}");
                match formatted.split_once('e') {
                    Some((mantissa, exponent)) => {
                        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
                        format!("{mantissa}e{exponent}")
                    }
                    None => formatted,
                }
            }
        }
    }
}

/// A time in seconds, as the duration modes write it.
fn render_duration(seconds: f64, duration: Duration, format: Format) -> String {
    let sign = if seconds < 0.0 { "-" } else { "" };
    let total = seconds.abs().round() as u64;
    match duration {
        Duration::Clock => format!(
            "{sign}{:02}:{:02}:{:02}",
            total / 3600,
            total / 60 % 60,
            total % 60
        ),
        Duration::Minutes => format!("{sign}{:02}:{:02}", total / 3600, total / 60 % 60),
        Duration::Hours => match format {
            Format::Natural => format!("{:.2}", seconds / 3600.0),
            format => render(seconds / 3600.0, format),
        },
    }
}

/// A value as it goes into its field.
fn render_value(value: &Value, modes: Modes) -> String {
    match value {
        Value::Quantity(n, units) if units.is_empty() => match modes.duration {
            Some(duration) if n.is_finite() => render_duration(*n, duration, modes.format),
            _ => render(*n, modes.format),
        },
        Value::Quantity(n, units) => {
            let n = render(*n, modes.format);
            if n == "#ERROR" {
                n
            } else {
                format!("{n} {}", units.display())
            }
        }
        Value::Date(stamp) => stamp.text(),
        Value::Symbolic(poly) => poly.display(|n| render(n, modes.format)),
        Value::List(values) => format!(
            "[{}]",
            values
                .iter()
                .map(|value| render_value(value, modes))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

impl Formula {
    /// Computes this formula for the field `at` and writes the result there,
    /// returning whether it failed.
    fn fill(&self, grid: &mut Grid, at: (usize, usize)) -> bool {
        let text = match &self.body {
            Body::Calc(expr) => Eval {
                grid,
                at,
                modes: self.modes,
            }
            .eval(expr)
            .and_then(|value| match value {
                Value::List(_) => Err("a range can only be used inside a function".to_string()),
                value => Ok(render_value(&value, self.modes)),
            }),
            Body::Lisp(form) => self.lisp(form, grid, at),
        };
        // A result holds no line break or `|`: it must stay one field.
        let text = text.map_or_else(|_| "#ERROR".to_string(), |text| cell_text(&text));
        let failed = text == "#ERROR";
        grid.set(at.0, at.1, text);
        failed
    }

    /// Evaluates a Lisp formula: its references are replaced by the fields,
    /// as strings, or as numbers with `;N`, or as written with `;L`.
    fn lisp(&self, form: &str, grid: &Grid, at: (usize, usize)) -> Result<String, String> {
        let modes = self.modes;
        let as_lisp = |text: &str| -> String {
            if modes.literal {
                text.to_string()
            } else if modes.numbers {
                let n = leading_number(text);
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", n as i64)
                } else {
                    format!("{n}")
                }
            } else {
                lisp::quote_string(text.trim())
            }
        };

        let chars: Vec<char> = form.chars().collect();
        let mut source = String::new();
        let mut i = 0;
        let mut in_string = false;
        while i < chars.len() {
            let c = chars[i];
            if in_string {
                source.push(c);
                if c == '\\' && i + 1 < chars.len() {
                    source.push(chars[i + 1]);
                    i += 1;
                } else if c == '"' {
                    in_string = false;
                }
                i += 1;
                continue;
            }
            match (c, chars.get(i + 1)) {
                ('"', _) => {
                    in_string = true;
                    source.push(c);
                    i += 1;
                }
                ('@', Some('#')) => {
                    let row = grid
                        .data
                        .iter()
                        .position(|&r| r == at.0)
                        .map_or(0, |r| r + 1);
                    source.push_str(&row.to_string());
                    i += 2;
                }
                ('$', Some('#')) => {
                    source.push_str(&(at.1 + 1).to_string());
                    i += 2;
                }
                ('@' | '$', _) => {
                    let mut parser = Parser {
                        chars: chars.clone(),
                        pos: i,
                    };
                    let (from, to) = parser
                        .reference_or_range()?
                        .ok_or("a reference that is not one")?;
                    let fields = match to {
                        None => vec![grid.field(from, at)?],
                        Some(to) => grid.range(from, to, at)?,
                    };
                    let range = to.is_some();
                    let parts: Vec<String> = fields
                        .into_iter()
                        .map(|(row, col)| grid.cell(row, col))
                        // Empty fields drop out of a range, as in Calc.
                        .filter(|text| !range || !text.trim().is_empty())
                        .map(as_lisp)
                        .collect();
                    source.push_str(&parts.join(" "));
                    i = parser.pos;
                }
                _ => {
                    source.push(c);
                    i += 1;
                }
            }
        }

        let value = lisp::evaluate(&source)?;
        Ok(match value {
            lisp::Lisp::Int(n) if modes.format != Format::Natural => render(n as f64, modes.format),
            lisp::Lisp::Float(n) if modes.format != Format::Natural => render(n, modes.format),
            other => lisp::display(&other),
        })
    }
}

// ── Recalculating ─────────────────────────────────────────────────────────

/// Finds the table `line` is in or under, and the `#+TBLFM:` line to use:
/// the one `line` is on, or else the first under the table.
fn locate(lines: &[&str], line: usize) -> Option<(usize, usize)> {
    let mut table_line = line;
    // From a `#+TBLFM:` line, up past the others to the table's last row.
    while table_line < lines.len() && tblfm_body(lines[table_line]).is_some() {
        table_line = table_line.checked_sub(1)?;
    }
    let first_tblfm = {
        let mut end = table_line;
        while end < lines.len() && is_table_line(lines[end]) {
            end += 1;
        }
        end
    };
    let formulas = if line >= first_tblfm && tblfm_body(lines.get(line)?).is_some() {
        line
    } else {
        first_tblfm
    };
    tblfm_body(lines.get(formulas)?)?;
    Some((table_line, formulas))
}

/// Recalculates the table at or above `line` from its `#+TBLFM:` line.
///
/// Column formulas go first, row by row from the top, so a running total
/// (`$3=$2+@-1$3`) reads the row above once it is done. Field formulas go
/// after, in the order they are written, and so win over a column formula
/// on the same field. An error in a formula's syntax stops everything and
/// says which formula; a field that cannot be computed gets `#ERROR`.
pub fn recalculate(text: &str, line: usize) -> Result<Recalculated, String> {
    let lines: Vec<&str> = text.lines().collect();
    let (table_line, tblfm_line) = match locate(&lines, line) {
        Some(found) => found,
        None if parse_table(text, line).is_some() => {
            return Err("The table has no #+TBLFM: line".to_string())
        }
        None => return Err("No Org table at the cursor".to_string()),
    };
    let table = parse_table(text, table_line).ok_or("No Org table at the cursor")?;
    let formulas = parse_tblfm(lines[tblfm_line])?;

    let mut grid = Grid::new(&table);
    let mut errors = 0;

    let (columns, fields): (Vec<&Formula>, Vec<&Formula>) = formulas
        .iter()
        .partition(|formula| matches!(formula.target, Target::Column(_)));

    for row in grid.body() {
        for formula in &columns {
            let Target::Column(col) = formula.target else {
                continue;
            };
            let col = grid
                .resolve_col(Some(col), 0)
                .map_err(|error| format!("`{}`: {error}", formula.source))?;
            errors += usize::from(formula.fill(&mut grid, (row, col)));
        }
    }
    for formula in fields {
        let Target::Fields(from, to) = formula.target else {
            continue;
        };
        let targets = match to {
            None => grid.field(from, (0, 0)).map(|field| vec![field]),
            Some(to) => grid.range(from, to, (0, 0)),
        }
        .map_err(|error| format!("`{}`: {error}", formula.source))?;
        for at in targets {
            errors += usize::from(formula.fill(&mut grid, at));
        }
    }

    Ok(Recalculated {
        text: rewrite(text, &table, grid.rows),
        formulas: formulas.len(),
        errors,
    })
}

/// Recalculates until nothing changes, for formulas that read fields other
/// formulas write further down. Gives up after ten passes, as Org does.
pub fn iterate(text: &str, line: usize) -> Result<Recalculated, String> {
    let mut current = recalculate(text, line)?;
    for _ in 0..10 {
        let next = recalculate(&current.text, line)?;
        if next.text == current.text {
            return Ok(next);
        }
        current = next;
    }
    Err("The table did not settle after ten passes".to_string())
}

/// Recalculates every table in the buffer that has a `#+TBLFM:` line,
/// returning the buffer, how many tables were computed and how many fields
/// failed.
pub fn recalculate_all(text: &str) -> Result<(String, usize, usize), String> {
    let mut current = text.to_string();
    let mut tables = 0;
    let mut errors = 0;

    // Recalculating never adds or removes a line, so the positions found
    // up front stay good.
    let starts: Vec<usize> = {
        let lines: Vec<&str> = text.lines().collect();
        (1..lines.len())
            .filter(|&i| tblfm_body(lines[i]).is_some() && is_table_line(lines[i - 1]))
            .collect()
    };
    for line in starts {
        let done = recalculate(&current, line - 1)?;
        current = done.text;
        errors += done.errors;
        tables += 1;
    }
    Ok((current, tables, errors))
}
