//! What a `:var` can take besides a literal: a named table, list or
//! example, or another block's results, read from the file or from the
//! Library of Babel, and written as each language writes such a value.

use super::BabelError;
use crate::source;

/// A value a block is given.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// As written, a number.
    Number(String),
    Text(String),
    List(Vec<Value>),
    /// Rows of cells; horizontal rules left out.
    Table(Vec<Vec<Value>>),
}

impl Value {
    /// A cell or an item: a number when it reads as one.
    fn scalar(text: &str) -> Value {
        let text = text.trim();
        if !text.is_empty() && text.parse::<f64>().is_ok() {
            Value::Number(text.to_string())
        } else {
            Value::Text(text.to_string())
        }
    }

    /// A literal from the header: a number, or a quoted string.
    pub fn literal(text: &str) -> Option<Value> {
        let text = text.trim();
        if text.parse::<f64>().is_ok() {
            return Some(Value::Number(text.to_string()));
        }
        let inner = text.strip_prefix('"')?.strip_suffix('"')?;
        Some(Value::Text(inner.replace("\\\"", "\"")))
    }
}

/// The value of the element named `name` in `text`: a table, a list, an
/// example or fixed-width text, or a block's results (`#+NAME:` on the
/// block, or `#+RESULTS: name`). `Err` when it is a block that has no
/// results yet; `Ok(None)` when nothing has that name.
///
/// A table's first row, when a rule follows it, is its column names: left
/// out, as Org does, unless `colnames` keeps them (`:colnames no`).
pub fn named(text: &str, name: &str, colnames: bool) -> Result<Option<Value>, String> {
    let lines: Vec<&str> = text.lines().collect();
    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let Some((key, value)) = trimmed
            .strip_prefix("#+")
            .and_then(|rest| rest.split_once(':'))
        else {
            continue;
        };
        let key = key.to_ascii_lowercase();
        let value = value.trim();
        if key == "results" && value == name {
            return Ok(element(&lines, at + 1, colnames));
        }
        if key.starts_with("results[") && value == name {
            return Ok(element(&lines, at + 1, colnames));
        }
        if key != "name" || value != name {
            continue;
        }
        // Past other affiliated keywords to the element itself.
        let mut next = at + 1;
        while lines.get(next).is_some_and(|line| {
            let trimmed = line.trim_start().to_ascii_lowercase();
            trimmed.starts_with("#+") && !trimmed.starts_with("#+begin")
        }) {
            next += 1;
        }
        let head = lines
            .get(next)
            .map(|line| line.trim_start().to_ascii_lowercase());
        if head
            .as_deref()
            .is_some_and(|head| head.starts_with("#+begin_src"))
        {
            let blocks = source::blocks(text);
            let Some(block) = blocks.iter().find(|block| block.begin == next) else {
                return Ok(None);
            };
            let Some(results) = source::result_of(text, block) else {
                return Err(format!("{name} has no results yet: run it first"));
            };
            return Ok(element(&lines, results + 1, colnames));
        }
        return Ok(element(&lines, next, colnames));
    }
    Ok(None)
}

/// The element starting at `at`, as a value.
fn element(lines: &[&str], at: usize, colnames: bool) -> Option<Value> {
    let first = lines.get(at)?;
    let trimmed = first.trim_start();
    if trimmed.starts_with('|') {
        let named_columns = lines
            .get(at + 1)
            .is_some_and(|line| line.trim_start().starts_with("|-"));
        let skip = usize::from(named_columns && !colnames);
        let rows = lines[at + skip..]
            .iter()
            .map(|line| line.trim())
            .take_while(|line| line.starts_with('|'))
            .filter(|line| !line.starts_with("|-"))
            .map(|line| {
                line.trim_matches('|')
                    .split('|')
                    .map(Value::scalar)
                    .collect::<Vec<_>>()
            })
            .collect();
        return Some(Value::Table(rows));
    }
    if let Some(item) = list_item(trimmed) {
        let indent = first.len() - trimmed.len();
        let mut items = vec![Value::scalar(item)];
        for line in &lines[at + 1..] {
            let rest = line.trim_start();
            if rest.is_empty() {
                break;
            }
            match list_item(rest) {
                Some(item) if line.len() - rest.len() == indent => items.push(Value::scalar(item)),
                // Deeper items and continuation lines belong to the item.
                _ if line.len() - rest.len() > indent => {}
                _ => break,
            }
        }
        return Some(Value::List(items));
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("#+begin_") {
        let kind = lower
            .split_whitespace()
            .next()
            .unwrap_or("")
            .replacen("#+begin_", "#+end_", 1);
        let body: Vec<&str> = lines[at + 1..]
            .iter()
            .take_while(|line| !line.trim().eq_ignore_ascii_case(&kind))
            .map(|line| line.strip_prefix(',').unwrap_or(line))
            .collect();
        return Some(Value::Text(body.join("\n")));
    }
    if trimmed == ":" || trimmed.starts_with(": ") {
        let body: Vec<&str> = lines[at..]
            .iter()
            .map(|line| line.trim_start())
            .take_while(|line| *line == ":" || line.starts_with(": "))
            .map(|line| line.strip_prefix(": ").unwrap_or(""))
            .collect();
        // One line reads as what it is: `: 16` is a number.
        return Some(match body.as_slice() {
            [one] => Value::scalar(one),
            _ => Value::Text(body.join("\n")),
        });
    }
    // Anything else: the text down to a blank line.
    let body: Vec<&str> = lines[at..]
        .iter()
        .take_while(|line| !line.trim().is_empty())
        .map(|line| line.trim())
        .collect();
    Some(Value::Text(body.join("\n")))
}

/// An item's text: `- a`, `+ a`, `1. a`, `1) a`, checkbox left out.
fn list_item(line: &str) -> Option<&str> {
    let rest = if let Some(rest) = line.strip_prefix("- ").or_else(|| line.strip_prefix("+ ")) {
        rest
    } else {
        let digits = line.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))?
    };
    Some(
        rest.strip_prefix("[ ] ")
            .or_else(|| rest.strip_prefix("[X] "))
            .or_else(|| rest.strip_prefix("[-] "))
            .unwrap_or(rest),
    )
}

/// A string in double quotes, escaped as C, Python and most others read it.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A string in single quotes, as a shell reads it.
fn shell_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// A value's text for a shell: a table as tab-separated rows, a list one
/// item a line, as Org passes them.
fn shell_text(value: &Value) -> String {
    match value {
        Value::Number(n) => n.clone(),
        Value::Text(text) => text.clone(),
        Value::List(items) => items.iter().map(shell_text).collect::<Vec<_>>().join("\n"),
        Value::Table(rows) => rows
            .iter()
            .map(|row| row.iter().map(shell_text).collect::<Vec<_>>().join("\t"))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// A value in bracket syntax: `[1, "a"]`, as Python, Ruby, JavaScript,
/// Julia and Perl (as a reference) read it; `{…}` for Lua.
fn bracketed(value: &Value, open: &str, close: &str) -> String {
    match value {
        Value::Number(n) => n.clone(),
        Value::Text(text) => quoted(text),
        Value::List(items) => format!(
            "{open}{}{close}",
            items
                .iter()
                .map(|item| bracketed(item, open, close))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Table(rows) => format!(
            "{open}{}{close}",
            rows.iter()
                .map(|row| bracketed(&Value::List(row.clone()), open, close))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn unsupported(name: &str, language: &str, what: &str) -> BabelError {
    BabelError::Variable(format!(
        ":var {name} is {what}, which is not passed to {language} blocks"
    ))
}

/// The line that sets `name` to `value` in `language`.
pub fn assignment(language: &str, name: &str, value: &Value) -> Result<String, BabelError> {
    let kind = match value {
        Value::Number(_) => "a number",
        Value::Text(_) => "text",
        Value::List(_) => "a list",
        Value::Table(_) => "a table",
    };
    Ok(match language {
        "sh" | "shell" | "fish" => match value {
            Value::Number(n) if language != "fish" => format!("{name}={n}"),
            other if language == "fish" => {
                format!("set {name} {}", shell_quoted(&shell_text(other)))
            }
            other => format!("{name}={}", shell_quoted(&shell_text(other))),
        },
        "bash" | "zsh" => match value {
            Value::Number(n) => format!("{name}={n}"),
            // An array, as Org gives bash a list.
            Value::List(items) => format!(
                "{name}=({})",
                items
                    .iter()
                    .map(|item| shell_quoted(&shell_text(item)))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            other => format!("{name}={}", shell_quoted(&shell_text(other))),
        },
        "python" | "python3" | "ruby" | "julia" => {
            format!("{name} = {}", bracketed(value, "[", "]"))
        }
        "js" | "javascript" | "node" => format!("const {name} = {};", bracketed(value, "[", "]")),
        "perl" => format!("my ${name} = {};", bracketed(value, "[", "]")),
        "lua" => format!("local {name} = {}", bracketed(value, "{", "}")),
        "R" => match value {
            Value::Table(_) => return Err(unsupported(name, language, kind)),
            other => format!("{name} <- {}", bracketed(other, "c(", ")")),
        },
        "C" | "c" | "C++" | "cpp" => match value {
            Value::Number(n) if n.parse::<i64>().is_ok() => format!("int {name} = {n};"),
            Value::Number(n) => format!("double {name} = {n};"),
            Value::Text(text) => format!("const char *{name} = {};", quoted(text)),
            _ => return Err(unsupported(name, language, kind)),
        },
        "rust" => match value {
            Value::Number(n) if n.parse::<i64>().is_ok() => format!("let {name}: i64 = {n};"),
            Value::Number(n) => format!("let {name}: f64 = {n};"),
            Value::Text(text) => format!("let {name}: &str = {};", quoted(text)),
            _ => return Err(unsupported(name, language, kind)),
        },
        // `_ =` because Go refuses a variable nothing uses.
        "go" => match value {
            Value::Number(n) | Value::Text(n) => {
                let literal = match value {
                    Value::Text(_) => quoted(n),
                    _ => n.clone(),
                };
                format!("{name} := {literal}\n_ = {name}")
            }
            _ => return Err(unsupported(name, language, kind)),
        },
        other => {
            return Err(BabelError::Variable(format!(
                ":var is not passed to {other} blocks"
            )))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
#+NAME: fruits
| name  | n |
|-------+---|
| apple | 3 |
| pear  | 5 |

#+NAME: todo
- [ ] eggs
- milk
  more about milk
- 2

#+NAME: square
#+begin_src python
return 4
#+end_src

#+RESULTS: square
: 16

#+NAME: fresh
#+begin_src sh
echo
#+end_src
";

    #[test]
    fn named_tables_lists_and_results() {
        assert_eq!(
            named(TEXT, "fruits", false).unwrap(),
            Some(Value::Table(vec![
                vec![Value::Text("apple".into()), Value::Number("3".into())],
                vec![Value::Text("pear".into()), Value::Number("5".into())],
            ]))
        );
        assert_eq!(
            named(TEXT, "todo", false).unwrap(),
            Some(Value::List(vec![
                Value::Text("eggs".into()),
                Value::Text("milk".into()),
                Value::Number("2".into()),
            ]))
        );
        assert_eq!(
            named(TEXT, "square", false).unwrap(),
            Some(Value::Number("16".into()))
        );
        assert!(named(TEXT, "fresh", false).is_err());
        assert_eq!(named(TEXT, "nothing", false).unwrap(), None);
    }

    #[test]
    fn each_language_writes_the_value_its_way() {
        let table = named(TEXT, "fruits", true).unwrap().unwrap();
        assert_eq!(
            assignment("python", "t", &table).unwrap(),
            "t = [[\"name\", \"n\"], [\"apple\", 3], [\"pear\", 5]]"
        );
        assert_eq!(
            assignment("sh", "t", &table).unwrap(),
            "t='name\tn\napple\t3\npear\t5'"
        );
        let list = Value::List(vec![Value::Text("a b".into()), Value::Number("2".into())]);
        assert_eq!(assignment("bash", "l", &list).unwrap(), "l=('a b' '2')");
        assert_eq!(
            assignment("lua", "l", &list).unwrap(),
            "local l = {\"a b\", 2}"
        );
        assert_eq!(
            assignment("C", "s", &Value::Text("it's \"x\"".into())).unwrap(),
            "const char *s = \"it's \\\"x\\\"\";"
        );
        assert_eq!(
            assignment("rust", "n", &Value::Number("3".into())).unwrap(),
            "let n: i64 = 3;"
        );
        assert_eq!(
            assignment("go", "n", &Value::Number("3.5".into())).unwrap(),
            "n := 3.5\n_ = n"
        );
        assert!(assignment("C", "t", &table).is_err());
    }
}
