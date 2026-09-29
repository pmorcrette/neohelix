//! Emacs Lisp formulas, `'(concat $1 " " $2)`: the string and number
//! functions such formulas use, not a Lisp. The fields are already in the
//! source when it gets here, as strings, numbers or literally, as the
//! formula's mode says.

#[derive(Debug, Clone, PartialEq)]
pub enum Lisp {
    Int(i64),
    Float(f64),
    Str(String),
    Sym(String),
    List(Vec<Lisp>),
}

use Lisp::*;

const NIL: Lisp = List(Vec::new());

fn t() -> Lisp {
    Sym("t".to_string())
}

fn truth(value: bool) -> Lisp {
    if value {
        t()
    } else {
        NIL
    }
}

fn is_nil(value: &Lisp) -> bool {
    match value {
        List(items) => items.is_empty(),
        Sym(name) => name == "nil",
        _ => false,
    }
}

// ── Reading ───────────────────────────────────────────────────────────────

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn skip(&mut self) {
        while let Some(&c) = self.chars.get(self.at) {
            if c.is_whitespace() {
                self.at += 1;
            } else if c == ';' {
                while self.chars.get(self.at).is_some_and(|&c| c != '\n') {
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    fn read(&mut self) -> Result<Lisp, String> {
        self.skip();
        match self.chars.get(self.at).copied() {
            None => Err("the form ends too early".to_string()),
            Some('(') => {
                self.at += 1;
                let mut items = Vec::new();
                loop {
                    self.skip();
                    match self.chars.get(self.at) {
                        Some(')') => {
                            self.at += 1;
                            return Ok(List(items));
                        }
                        None => return Err("missing `)`".to_string()),
                        _ => items.push(self.read()?),
                    }
                }
            }
            Some(')') => Err("unexpected `)`".to_string()),
            Some('\'') => {
                self.at += 1;
                Ok(List(vec![Sym("quote".to_string()), self.read()?]))
            }
            Some('"') => {
                self.at += 1;
                let mut text = String::new();
                loop {
                    match self.chars.get(self.at).copied() {
                        None => return Err("unterminated string".to_string()),
                        Some('"') => {
                            self.at += 1;
                            return Ok(Str(text));
                        }
                        Some('\\') => {
                            self.at += 1;
                            match self.chars.get(self.at).copied() {
                                Some('n') => text.push('\n'),
                                Some('t') => text.push('\t'),
                                Some(c) => text.push(c),
                                None => return Err("unterminated string".to_string()),
                            }
                            self.at += 1;
                        }
                        Some(c) => {
                            text.push(c);
                            self.at += 1;
                        }
                    }
                }
            }
            Some(_) => {
                let start = self.at;
                while self
                    .chars
                    .get(self.at)
                    .is_some_and(|&c| !c.is_whitespace() && !matches!(c, '(' | ')' | '"' | '\''))
                {
                    self.at += 1;
                }
                let word: String = self.chars[start..self.at].iter().collect();
                if let Ok(n) = word.parse::<i64>() {
                    Ok(Int(n))
                } else if let Ok(n) = word.parse::<f64>() {
                    Ok(Float(n))
                } else if word == "nil" {
                    Ok(NIL)
                } else {
                    Ok(Sym(word))
                }
            }
        }
    }
}

// ── Evaluating ────────────────────────────────────────────────────────────

fn number(value: &Lisp) -> Result<f64, String> {
    match value {
        Int(n) => Ok(*n as f64),
        Float(n) => Ok(*n),
        other => Err(format!("{} is not a number", print(other))),
    }
}

fn string(value: &Lisp) -> Result<String, String> {
    match value {
        Str(text) => Ok(text.clone()),
        Sym(name) => Ok(name.clone()),
        other => Err(format!("{} is not a string", print(other))),
    }
}

fn int(value: &Lisp) -> Result<i64, String> {
    match value {
        Int(n) => Ok(*n),
        other => Err(format!("{} is not an integer", print(other))),
    }
}

/// Integer arithmetic while every argument is an integer, as in Lisp.
fn arithmetic(name: &str, args: &[Lisp]) -> Result<Lisp, String> {
    if args.iter().all(|arg| matches!(arg, Int(_))) {
        let ints: Vec<i64> = args.iter().map(int).collect::<Result<_, _>>()?;
        let result = match name {
            "+" => ints.iter().sum(),
            "*" => ints.iter().product(),
            "-" => match ints.as_slice() {
                [] => 0,
                [only] => -only,
                [first, rest @ ..] => first - rest.iter().sum::<i64>(),
            },
            _ => {
                let [first, rest @ ..] = ints.as_slice() else {
                    return Err("`/` needs an argument".to_string());
                };
                let mut out = *first;
                for n in rest {
                    if *n == 0 {
                        return Err("division by zero".to_string());
                    }
                    out /= n;
                }
                out
            }
        };
        return Ok(Int(result));
    }
    let floats: Vec<f64> = args.iter().map(number).collect::<Result<_, _>>()?;
    let result = match name {
        "+" => floats.iter().sum(),
        "*" => floats.iter().product(),
        "-" => match floats.as_slice() {
            [] => 0.0,
            [only] => -only,
            [first, rest @ ..] => first - rest.iter().sum::<f64>(),
        },
        _ => {
            let [first, rest @ ..] = floats.as_slice() else {
                return Err("`/` needs an argument".to_string());
            };
            rest.iter().fold(*first, |acc, n| acc / n)
        }
    };
    Ok(Float(result))
}

fn compare(name: &str, args: &[Lisp]) -> Result<Lisp, String> {
    let values: Vec<f64> = args.iter().map(number).collect::<Result<_, _>>()?;
    let holds = values.windows(2).all(|pair| match name {
        "=" => pair[0] == pair[1],
        "<" => pair[0] < pair[1],
        ">" => pair[0] > pair[1],
        "<=" => pair[0] <= pair[1],
        ">=" => pair[0] >= pair[1],
        _ => pair[0] != pair[1],
    });
    Ok(truth(holds))
}

/// `format`'s `%s`, `%d`, `%f`, `%.2f`, `%S` and `%%`.
fn format_string(control: &str, args: &[Lisp]) -> Result<String, String> {
    let mut out = String::new();
    let mut args = args.iter();
    let mut chars = control.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let mut spec = String::new();
        while let Some(&c) = chars.peek() {
            spec.push(c);
            chars.next();
            if c.is_ascii_alphabetic() || c == '%' {
                break;
            }
        }
        let kind = spec.chars().last().unwrap_or('s');
        if kind == '%' {
            out.push('%');
            continue;
        }
        let arg = args.next().ok_or("format: not enough arguments")?;
        let precision = spec
            .split_once('.')
            .and_then(|(_, rest)| rest.trim_end_matches(kind).parse::<usize>().ok());
        match kind {
            's' => out.push_str(&display(arg)),
            'S' => out.push_str(&print(arg)),
            'd' => out.push_str(&format!("{}", number(arg)?.trunc() as i64)),
            'f' => out.push_str(&format!("{:.*}", precision.unwrap_or(6), number(arg)?)),
            'e' => out.push_str(&format!("{:.*e}", precision.unwrap_or(6), number(arg)?)),
            other => return Err(format!("format: %{other} is not supported")),
        }
    }
    Ok(out)
}

fn capitalize(text: &str) -> String {
    let mut out = String::new();
    let mut start = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

fn eval(form: &Lisp) -> Result<Lisp, String> {
    match form {
        Int(_) | Float(_) | Str(_) => Ok(form.clone()),
        Sym(name) if name == "t" || name == "nil" || name.starts_with(':') => Ok(form.clone()),
        Sym(name) => Err(format!("`{name}` has no value here")),
        List(items) if items.is_empty() => Ok(NIL),
        List(items) => {
            let Sym(name) = &items[0] else {
                return Err(format!("{} is not a function", print(&items[0])));
            };
            let rest = &items[1..];
            // The special forms, which do not evaluate every argument.
            match name.as_str() {
                "quote" => return rest.first().cloned().ok_or("quote of nothing".to_string()),
                "if" => {
                    let test = eval(rest.first().ok_or("if needs a test")?)?;
                    return if !is_nil(&test) {
                        rest.get(1).map_or(Ok(NIL), eval)
                    } else {
                        let mut value = NIL;
                        for form in rest.iter().skip(2) {
                            value = eval(form)?;
                        }
                        Ok(value)
                    };
                }
                "when" | "unless" => {
                    let test = !is_nil(&eval(rest.first().ok_or("needs a test")?)?);
                    if test != (name == "when") {
                        return Ok(NIL);
                    }
                    let mut value = NIL;
                    for form in &rest[1..] {
                        value = eval(form)?;
                    }
                    return Ok(value);
                }
                "and" => {
                    let mut value = t();
                    for form in rest {
                        value = eval(form)?;
                        if is_nil(&value) {
                            return Ok(NIL);
                        }
                    }
                    return Ok(value);
                }
                "or" => {
                    for form in rest {
                        let value = eval(form)?;
                        if !is_nil(&value) {
                            return Ok(value);
                        }
                    }
                    return Ok(NIL);
                }
                "cond" => {
                    for clause in rest {
                        let List(parts) = clause else {
                            return Err("cond: a clause is not a list".to_string());
                        };
                        let Some(test) = parts.first() else { continue };
                        let value = eval(test)?;
                        if !is_nil(&value) {
                            let mut result = value;
                            for form in &parts[1..] {
                                result = eval(form)?;
                            }
                            return Ok(result);
                        }
                    }
                    return Ok(NIL);
                }
                _ => {}
            }
            let args: Vec<Lisp> = rest.iter().map(eval).collect::<Result<_, _>>()?;
            call(name, &args)
        }
    }
}

fn call(name: &str, args: &[Lisp]) -> Result<Lisp, String> {
    let arg = |n: usize| {
        args.get(n)
            .ok_or_else(|| format!("{name}: not enough arguments"))
    };
    match name {
        "+" | "-" | "*" | "/" => arithmetic(name, args),
        "%" | "mod" => {
            let (a, b) = (number(arg(0)?)?, number(arg(1)?)?);
            if b == 0.0 {
                return Err("division by zero".to_string());
            }
            let result = if name == "%" { a % b } else { a.rem_euclid(b) };
            Ok(match (arg(0)?, arg(1)?) {
                (Int(_), Int(_)) => Int(result as i64),
                _ => Float(result),
            })
        }
        "1+" => arithmetic("+", &[arg(0)?.clone(), Int(1)]),
        "1-" => arithmetic("-", &[arg(0)?.clone(), Int(1)]),
        "max" | "min" => {
            let values: Vec<f64> = args.iter().map(number).collect::<Result<_, _>>()?;
            let pick = if name == "max" { f64::max } else { f64::min };
            let best = values
                .iter()
                .copied()
                .reduce(pick)
                .ok_or(format!("{name} of nothing"))?;
            Ok(if args.iter().all(|a| matches!(a, Int(_))) {
                Int(best as i64)
            } else {
                Float(best)
            })
        }
        "abs" => Ok(match arg(0)? {
            Int(n) => Int(n.abs()),
            other => Float(number(other)?.abs()),
        }),
        "float" => Ok(Float(number(arg(0)?)?)),
        "round" | "floor" | "ceiling" | "truncate" => {
            let n = number(arg(0)?)?;
            let n = match args.get(1) {
                Some(divisor) => n / number(divisor)?,
                None => n,
            };
            Ok(Int(match name {
                // Lisp rounds halves to even.
                "round" => {
                    let rounded = n.round();
                    if (n - n.trunc()).abs() == 0.5 && rounded % 2.0 != 0.0 {
                        (rounded - n.signum()) as i64
                    } else {
                        rounded as i64
                    }
                }
                "floor" => n.floor() as i64,
                "ceiling" => n.ceil() as i64,
                _ => n.trunc() as i64,
            }))
        }
        "expt" => match (arg(0)?, arg(1)?) {
            (Int(a), Int(b)) if *b >= 0 => Ok(Int(a.pow(*b as u32))),
            (a, b) => Ok(Float(number(a)?.powf(number(b)?))),
        },
        "sqrt" => Ok(Float(number(arg(0)?)?.sqrt())),
        "=" | "<" | ">" | "<=" | ">=" | "/=" => compare(name, args),
        "not" | "null" => Ok(truth(is_nil(arg(0)?))),
        "string=" | "string-equal" => Ok(truth(string(arg(0)?)? == string(arg(1)?)?)),
        "string<" | "string-lessp" => Ok(truth(string(arg(0)?)? < string(arg(1)?)?)),
        "equal" | "eq" | "eql" => Ok(truth(arg(0)? == arg(1)?)),
        "concat" => Ok(Str(args
            .iter()
            .map(|a| match a {
                List(items) => items.iter().map(display).collect(),
                other => display(other),
            })
            .collect())),
        "substring" => {
            let text: Vec<char> = string(arg(0)?)?.chars().collect();
            let len = text.len() as i64;
            let index = |value: Option<&Lisp>, default: i64| -> Result<usize, String> {
                let n = match value {
                    Some(value) if !is_nil(value) => int(value)?,
                    _ => default,
                };
                let n = if n < 0 { len + n } else { n };
                if n < 0 || n > len {
                    return Err(format!("substring: {n} is outside the string"));
                }
                Ok(n as usize)
            };
            let from = index(args.get(1), 0)?;
            let to = index(args.get(2), len)?;
            if from > to {
                return Err("substring: the start is after the end".to_string());
            }
            Ok(Str(text[from..to].iter().collect()))
        }
        "upcase" => Ok(Str(string(arg(0)?)?.to_uppercase())),
        "downcase" => Ok(Str(string(arg(0)?)?.to_lowercase())),
        "capitalize" => Ok(Str(capitalize(&string(arg(0)?)?))),
        "string-trim" => Ok(Str(string(arg(0)?)?.trim().to_string())),
        "length" => Ok(Int(match arg(0)? {
            Str(text) => text.chars().count() as i64,
            List(items) => items.len() as i64,
            other => return Err(format!("length of {}", print(other))),
        })),
        "string-to-number" => {
            let text = string(arg(0)?)?;
            let text = text.trim();
            // Like Lisp, the number at the start, or zero.
            let end = text
                .char_indices()
                .take_while(|(at, c)| {
                    c.is_ascii_digit() || *c == '.' || (*at == 0 && matches!(c, '-' | '+'))
                })
                .map(|(at, c)| at + c.len_utf8())
                .last()
                .unwrap_or(0);
            let head = &text[..end];
            Ok(if let Ok(n) = head.parse::<i64>() {
                Int(n)
            } else {
                Float(head.parse::<f64>().unwrap_or(0.0))
            })
        }
        "number-to-string" => Ok(Str(display(arg(0)?))),
        "format" => {
            let control = string(arg(0)?)?;
            Ok(Str(format_string(&control, &args[1..])?))
        }
        "list" => Ok(List(args.to_vec())),
        "car" => Ok(match arg(0)? {
            List(items) => items.first().cloned().unwrap_or(NIL),
            other => return Err(format!("car of {}", print(other))),
        }),
        "cdr" => Ok(match arg(0)? {
            List(items) => List(items.iter().skip(1).cloned().collect()),
            other => return Err(format!("cdr of {}", print(other))),
        }),
        "nth" => {
            let n = int(arg(0)?)?;
            Ok(match arg(1)? {
                List(items) => items.get(n as usize).cloned().unwrap_or(NIL),
                other => return Err(format!("nth of {}", print(other))),
            })
        }
        "apply" => {
            let Sym(function) = arg(0)? else {
                return Err("apply: the first argument must be a quoted function".to_string());
            };
            let mut spread: Vec<Lisp> = args[1..args.len() - 1].to_vec();
            match args.last() {
                Some(List(items)) => spread.extend(items.iter().cloned()),
                Some(other) => spread.push(other.clone()),
                None => {}
            }
            call(function, &spread)
        }
        _ => Err(format!("`{name}` is not a function this knows")),
    }
}

/// How `princ` shows a value: strings without quotes.
pub fn display(value: &Lisp) -> String {
    match value {
        Str(text) => text.clone(),
        other => print(other),
    }
}

/// How `prin1` shows a value.
fn print(value: &Lisp) -> String {
    match value {
        Int(n) => n.to_string(),
        Float(n) if n.fract() == 0.0 && n.abs() < 1e16 => format!("{n:.1}"),
        Float(n) => n.to_string(),
        Str(text) => format!("{text:?}"),
        Sym(name) => name.clone(),
        List(items) if items.is_empty() => "nil".to_string(),
        List(items) => format!(
            "({})",
            items.iter().map(print).collect::<Vec<_>>().join(" ")
        ),
    }
}

/// Reads and evaluates one form.
pub fn evaluate(source: &str) -> Result<Lisp, String> {
    let mut reader = Reader {
        chars: source.chars().collect(),
        at: 0,
    };
    let form = reader.read()?;
    reader.skip();
    if reader.at < reader.chars.len() {
        return Err("more than one form".to_string());
    }
    eval(&form)
}

/// A string as Lisp reads it back.
pub fn quote_string(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> String {
        display(&evaluate(source).unwrap())
    }

    #[test]
    fn strings_numbers_and_forms() {
        assert_eq!(run("(concat \"a\" \"-\" \"b\")"), "a-b");
        assert_eq!(run("(substring \"Hello\" 1 3)"), "el");
        assert_eq!(run("(substring \"Hello\" -2)"), "lo");
        assert_eq!(run("(upcase \"x\")"), "X");
        assert_eq!(run("(capitalize \"hello world\")"), "Hello World");
        assert_eq!(run("(+ 1 2 3)"), "6");
        assert_eq!(run("(/ 7 2)"), "3");
        assert_eq!(run("(/ 7.0 2)"), "3.5");
        assert_eq!(run("(apply '+ '(1 2 3))"), "6");
        assert_eq!(run("(format \"%s is %.2f\" \"pi\" 3.14159)"), "pi is 3.14");
        assert_eq!(run("(if (> 3 2) \"yes\" \"no\")"), "yes");
        assert_eq!(run("(length \"abc\")"), "3");
        assert_eq!(run("(string-to-number \"12 apples\")"), "12");
        assert_eq!(run("(round 2.5)"), "2");
        assert_eq!(run("(* 1.5 2)"), "3.0");
        assert!(evaluate("(shell-command \"ls\")").is_err());
        assert!(evaluate("(concat \"a\"").is_err());
    }
}
