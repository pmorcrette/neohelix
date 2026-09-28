//! Org table formulas: the `#+TBLFM:` line under a table.
//!
//! Org hands its formulas to Emacs Calc, a computer algebra system; this is
//! the part of that language a table of figures actually uses. Column
//! formulas (`$4=$2*$3`) and field formulas (`@>$4=vsum(@I..@II)`), Org's
//! references (absolute, relative, first and last, hlines, ranges), the four
//! operations with `^` and `%`, the vector functions (`vsum`, `vmean`, …),
//! a few scalar ones, and the `;%.2f`, `;f2` and `;N` modes.
//!
//! Not here: Emacs Lisp formulas (`'(…)`), Calc's symbolic algebra, `if`,
//! dates and durations, named columns and fields, `#+CONSTANTS`, and the
//! marking column Org reads from a first column of `!`, `^`, `#` and the
//! like. A formula that needs one is reported rather than half evaluated.

use crate::table::{parse_table, rewrite, Row, Table};

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
}

#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Number(f64),
    Field(Reference),
    Range(Reference, Reference),
    Neg(Box<Expr>),
    Binary(Op, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
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

#[derive(Debug, Clone, PartialEq)]
struct Formula {
    source: String,
    target: Target,
    expr: Expr,
    format: Format,
    /// `;N`: text counts as zero instead of being an error.
    numbers: bool,
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
        let mut left = self.unary()?;
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
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
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
        if let Some((from, to)) = self.reference_or_range()? {
            return Ok(match to {
                Some(to) => Expr::Range(from, to),
                None => Expr::Field(from),
            });
        }

        match self.peek() {
            Some(c) if c.is_ascii_digit() || c == '.' => self.number(),
            Some(c) if c.is_ascii_alphabetic() => {
                let start = self.pos;
                while self.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
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
                } else if name == "pi" {
                    Ok(Expr::Number(std::f64::consts::PI))
                } else {
                    Err(format!("unknown name `{name}`"))
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
            Some('\'') => Err("Emacs Lisp formulas are not supported".to_string()),
            Some('"') => Err("strings are not supported".to_string()),
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
        "abs" | "sqrt" | "exp" | "ln" | "log10" | "floor" | "ceil" => (1, 1),
        "round" => (1, 2),
        _ => return Err(format!("unknown function `{name}`")),
    };
    if args < min || args > max {
        return Err(format!("wrong number of arguments to `{name}`"));
    }
    Ok(())
}

/// The modes after `;`: `%.2f`, `%d`, `%.3e`, `f2`, `N`.
fn parse_modes(modes: &str) -> Result<(Format, bool), String> {
    let chars: Vec<char> = modes.trim().chars().collect();
    let mut format = Format::Natural;
    let mut numbers = false;
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
            'N' => {
                numbers = true;
                i += 1;
            }
            c if c.is_whitespace() => i += 1,
            c => return Err(format!("unsupported mode `{c}`")),
        }
    }
    Ok((format, numbers))
}

/// Reads one `lhs=rhs;modes` formula.
fn parse_formula(source: &str) -> Result<Formula, String> {
    let (assignment, modes) = match source.split_once(';') {
        Some((assignment, modes)) => (assignment, modes),
        None => (source, ""),
    };
    let (lhs, rhs) = assignment.split_once('=').ok_or("a formula needs `=`")?;
    let (format, numbers) = parse_modes(modes)?;

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

    let mut right = Parser::new(rhs);
    let expr = right.expr()?;
    if !right.at_end() {
        let rest: String = right.chars[right.pos..].iter().collect();
        return Err(format!("unexpected `{}`", rest.trim()));
    }

    Ok(Formula {
        source: source.trim().to_string(),
        target,
        expr,
        format,
        numbers,
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
    let trimmed = line.trim_start();
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

enum Value {
    Number(f64),
    List(Vec<f64>),
}

struct Eval<'a> {
    grid: &'a Grid,
    at: (usize, usize),
    numbers: bool,
}

impl Eval<'_> {
    fn value_of(&self, text: &str) -> Result<f64, String> {
        match read_number(text) {
            Some(n) => Ok(n),
            None if self.numbers => Ok(0.0),
            None => Err(format!("`{}` is not a number", text.trim())),
        }
    }

    fn eval(&self, expr: &Expr) -> Result<Value, String> {
        match expr {
            Expr::Number(n) => Ok(Value::Number(*n)),
            Expr::Field(reference) => {
                let (row, col) = self.grid.field(*reference, self.at)?;
                let text = self.grid.cell(row, col);
                // A lone empty field is zero, as Org has it.
                if text.trim().is_empty() {
                    Ok(Value::Number(0.0))
                } else {
                    Ok(Value::Number(self.value_of(text)?))
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
            Expr::Neg(inner) => Ok(Value::Number(-self.scalar(inner)?)),
            Expr::Binary(op, left, right) => {
                let (a, b) = (self.scalar(left)?, self.scalar(right)?);
                let result = match op {
                    Op::Add => a + b,
                    Op::Sub => a - b,
                    Op::Mul => a * b,
                    Op::Div if b == 0.0 => return Err("division by zero".to_string()),
                    Op::Div => a / b,
                    Op::Mod if b == 0.0 => return Err("division by zero".to_string()),
                    // Calc's `%` takes the sign of the divisor.
                    Op::Mod => a - b * (a / b).floor(),
                    Op::Pow => a.powf(b),
                };
                Ok(Value::Number(result))
            }
            Expr::Call(name, args) => self.call(name, args).map(Value::Number),
        }
    }

    fn scalar(&self, expr: &Expr) -> Result<f64, String> {
        match self.eval(expr)? {
            Value::Number(n) => Ok(n),
            Value::List(_) => Err("a range can only be used inside a function".to_string()),
        }
    }

    /// Every argument, ranges spread out.
    fn spread(&self, args: &[Expr]) -> Result<Vec<f64>, String> {
        let mut values = Vec::new();
        for arg in args {
            match self.eval(arg)? {
                Value::Number(n) => values.push(n),
                Value::List(list) => values.extend(list),
            }
        }
        Ok(values)
    }

    fn call(&self, name: &str, args: &[Expr]) -> Result<f64, String> {
        let one = |f: fn(f64) -> f64| -> Result<f64, String> { Ok(f(self.scalar(&args[0])?)) };
        let nonempty = |values: Vec<f64>| -> Result<Vec<f64>, String> {
            if values.is_empty() {
                Err(format!("`{name}` of nothing"))
            } else {
                Ok(values)
            }
        };

        match name {
            "vsum" => Ok(self.spread(args)?.iter().sum()),
            "vprod" => Ok(self.spread(args)?.iter().product()),
            "vcount" => Ok(self.spread(args)?.len() as f64),
            "vmean" => {
                let values = nonempty(self.spread(args)?)?;
                Ok(values.iter().sum::<f64>() / values.len() as f64)
            }
            "vmedian" => {
                let mut values = nonempty(self.spread(args)?)?;
                values.sort_by(f64::total_cmp);
                let mid = values.len() / 2;
                Ok(if values.len() % 2 == 0 {
                    (values[mid - 1] + values[mid]) / 2.0
                } else {
                    values[mid]
                })
            }
            "vsdev" => {
                // The sample standard deviation, which is Calc's.
                let values = self.spread(args)?;
                if values.len() < 2 {
                    return Err("`vsdev` needs two values".to_string());
                }
                let mean = values.iter().sum::<f64>() / values.len() as f64;
                let squares: f64 = values.iter().map(|v| (v - mean).powi(2)).sum();
                Ok((squares / (values.len() - 1) as f64).sqrt())
            }
            "vmin" | "min" => Ok(nonempty(self.spread(args)?)?
                .into_iter()
                .fold(f64::INFINITY, f64::min)),
            "vmax" | "max" => Ok(nonempty(self.spread(args)?)?
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)),
            "abs" => one(f64::abs),
            "floor" => one(f64::floor),
            "ceil" => one(f64::ceil),
            "exp" => one(f64::exp),
            "sqrt" => {
                let n = self.scalar(&args[0])?;
                if n < 0.0 {
                    return Err("square root of a negative number".to_string());
                }
                Ok(n.sqrt())
            }
            "ln" | "log10" => {
                let n = self.scalar(&args[0])?;
                if n <= 0.0 {
                    return Err("logarithm of a number that is not positive".to_string());
                }
                Ok(if name == "ln" { n.ln() } else { n.log10() })
            }
            "round" => {
                let n = self.scalar(&args[0])?;
                let places = match args.get(1) {
                    Some(places) => self.scalar(places)?,
                    None => 0.0,
                };
                let scale = 10f64.powi(places as i32);
                Ok((n * scale).round() / scale)
            }
            _ => Err(format!("unknown function `{name}`")),
        }
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

impl Formula {
    /// Computes this formula for the field `at` and writes the result there,
    /// returning whether it failed.
    fn fill(&self, grid: &mut Grid, at: (usize, usize)) -> bool {
        let result = Eval {
            grid,
            at,
            numbers: self.numbers,
        }
        .scalar(&self.expr);
        let text = result
            .map(|value| render(value, self.format))
            .unwrap_or_else(|_| "#ERROR".to_string());
        let failed = text == "#ERROR";
        grid.set(at.0, at.1, text);
        failed
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
        while end < lines.len() && lines[end].trim_start().starts_with('|') {
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
            .filter(|&i| {
                tblfm_body(lines[i]).is_some() && lines[i - 1].trim_start().starts_with('|')
            })
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
