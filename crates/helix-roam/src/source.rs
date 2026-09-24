//! Source blocks as code: finding them, taking their body out to edit it with
//! the language's own tooling and putting it back, moving between a block and
//! its result, and tangling.
//!
//! Tangling is the half of literate programming that runs nothing: it writes
//! each block's body to the file its `:tangle` header names. It needs no
//! trust decision, which is why it is here and Babel (Task 1.9) is not.
//!
//! A block's body is stored escaped: Org puts a comma before any line that
//! would otherwise read as a headline or a keyword (`,* not a headline`,
//! `,#+end_src`). What an editor or a tangled file sees is the unescaped
//! code; what goes back into the Org file is escaped again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::parser::FileSettings;

/// A `#+begin_src` … `#+end_src` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceBlock {
    /// The `#+begin_src` line.
    pub begin: usize,
    /// The `#+end_src` line.
    pub end: usize,
    pub language: Option<String>,
    /// `#+NAME:`, which results and noweb references find it by.
    pub name: Option<String>,
    /// Header arguments from `#+HEADER:` lines and the begin line, in the
    /// order written, so a later one overrides an earlier one.
    pub header: Vec<(String, String)>,
}

impl SourceBlock {
    /// The last value given for a header argument, `:tangle` asked as
    /// `"tangle"`.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.header
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

/// Every source block in `text`, in order.
///
/// A `#+begin_src` without an `#+end_src` is not a block: it is text that
/// happens to look like the start of one, and treating it as a block would
/// swallow the rest of the file.
pub fn blocks(text: &str) -> Vec<SourceBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    let mut at = 0;

    while at < lines.len() {
        let Some(params) = keyword_rest(lines[at], "#+begin_src") else {
            at += 1;
            continue;
        };
        let Some(end) =
            (at + 1..lines.len()).find(|&line| keyword_rest(lines[line], "#+end_src").is_some())
        else {
            at += 1;
            continue;
        };

        let (language, mut header) = parse_parameters(params);

        // Affiliated keywords sit directly above, in any order.
        let mut name = None;
        let mut above = Vec::new();
        for line in lines[..at].iter().rev() {
            if let Some(value) = keyword_rest(line, "#+name:") {
                name = Some(value.trim().to_string());
            } else if let Some(value) =
                keyword_rest(line, "#+header:").or_else(|| keyword_rest(line, "#+headers:"))
            {
                above.push(parse_header_args(value));
            } else if !is_affiliated(line) {
                // Only affiliated keywords attach to what follows. Walking
                // past any `#+` line would reach the `#+end_src` of the block
                // above and take that block's name.
                break;
            }
        }
        // Written top to bottom, collected bottom to top.
        let mut from_above: Vec<(String, String)> = above.into_iter().rev().flatten().collect();
        from_above.append(&mut header);

        found.push(SourceBlock {
            begin: at,
            end,
            language,
            name,
            header: from_above,
        });
        at = end + 1;
    }

    found
}

/// `#+CAPTION:`, `#+ATTR_HTML:` and the like: keywords that belong to the
/// element below them rather than standing on their own.
fn is_affiliated(line: &str) -> bool {
    [
        "#+caption:",
        "#+attr_",
        "#+plot:",
        "#+results:",
        "#+name:",
        "#+header",
    ]
    .iter()
    .any(|keyword| {
        keyword_rest(line, keyword).is_some() || {
            let trimmed = line.trim_start();
            trimmed.len() >= keyword.len() && trimmed[..keyword.len()].eq_ignore_ascii_case(keyword)
        }
    })
}

/// The block containing `line`, its begin and end lines included.
pub fn block_at(blocks: &[SourceBlock], line: usize) -> Option<&SourceBlock> {
    blocks
        .iter()
        .find(|block| (block.begin..=block.end).contains(&line))
}

/// The first block starting after `line`.
pub fn next_block(blocks: &[SourceBlock], line: usize) -> Option<&SourceBlock> {
    blocks.iter().find(|block| block.begin > line)
}

/// The last block starting above `line`.
///
/// From inside a block that is the block's own `#+begin_src`, as in Org,
/// where this is a search backwards for a begin line.
pub fn previous_block(blocks: &[SourceBlock], line: usize) -> Option<&SourceBlock> {
    blocks.iter().rev().find(|block| block.begin < line)
}

/// The `#+RESULTS:` line belonging to `block`.
///
/// Either directly after it, past blank lines, or — for a named block —
/// anywhere, under `#+RESULTS: name`.
pub fn result_of(text: &str, block: &SourceBlock) -> Option<usize> {
    let lines: Vec<&str> = text.lines().collect();

    let after = (block.end + 1..lines.len()).find(|&line| !lines[line].trim().is_empty());
    if let Some(line) = after {
        if let Some(name) = results_keyword(lines[line]) {
            if name.is_empty() || Some(name) == block.name.as_deref() {
                return Some(line);
            }
        }
    }

    let name = block.name.as_deref()?;
    lines
        .iter()
        .position(|line| results_keyword(line) == Some(name))
}

/// The block a `#+RESULTS:` line at `line` belongs to.
///
/// `line` may be the keyword or any line of the output under it: the output
/// runs down to the first blank line.
pub fn block_of_result<'a>(
    text: &str,
    blocks: &'a [SourceBlock],
    line: usize,
) -> Option<&'a SourceBlock> {
    let lines: Vec<&str> = text.lines().collect();
    let line = (0..=line.min(lines.len().checked_sub(1)?))
        .rev()
        .take_while(|&at| !lines[at].trim().is_empty())
        .find(|&at| results_keyword(lines[at]).is_some())?;
    let name = results_keyword(lines[line])?;

    if !name.is_empty() {
        if let Some(block) = blocks
            .iter()
            .find(|block| block.name.as_deref() == Some(name))
        {
            return Some(block);
        }
    }

    // Unnamed: the block ending just above, past blank lines.
    let above = (0..line).rev().find(|&at| !lines[at].trim().is_empty())?;
    blocks.iter().find(|block| block.end == above)
}

/// `#+RESULTS:`, `#+RESULTS[hash]:` or `#+RESULTS: name`, giving the name.
fn results_keyword(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let head = trimmed.get(..10)?;
    if !head.eq_ignore_ascii_case("#+results:") && !head.eq_ignore_ascii_case("#+results[") {
        return None;
    }
    let (_, name) = trimmed.split_once(':')?;
    Some(name.trim())
}

/// The block's code: unescaped, with the indentation the block's lines share
/// taken off.
pub fn body(text: &str, block: &SourceBlock) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let inner = &lines[block.begin + 1..block.end];
    let indent = common_indent(inner);

    let mut body: Vec<String> = inner
        .iter()
        .map(|line| unescape(line.get(indent..).unwrap_or("").trim_end_matches('\r')))
        .collect();
    if !body.is_empty() {
        body.push(String::new());
    }
    body.join("\n")
}

/// `text` with `block`'s body replaced by `code`.
///
/// The code goes back escaped and indented as the old body was — or, for a
/// block that was empty, as its `#+begin_src` line is. Org would re-indent
/// everything by `org-edit-src-content-indentation`; keeping what the file
/// already had means editing one line of a block does not re-indent it.
pub fn replace_body(text: &str, block: &SourceBlock, code: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let inner = &lines[block.begin + 1..block.end];
    // The shared indentation as written, tabs included, not re-spelled.
    let indent = match inner.iter().find(|line| !line.trim().is_empty()) {
        Some(first) => first[..common_indent(inner)].to_string(),
        None => leading_whitespace(lines[block.begin]).to_string(),
    };

    let mut out: Vec<String> = lines[..=block.begin]
        .iter()
        .map(|l| l.to_string())
        .collect();
    for line in code.lines() {
        if line.trim().is_empty() {
            out.push(String::new());
        } else {
            out.push(format!("{indent}{}", escape(line)));
        }
    }
    out.extend(lines[block.end..].iter().map(|l| l.to_string()));

    let joined = out.join("\n");
    if text.ends_with('\n') {
        format!("{joined}\n")
    } else {
        joined
    }
}

/// Takes one comma off a line Org escaped: `,*`, `,#+`, and `,,*` for a
/// line that really started with a comma.
pub(crate) fn unescape(line: &str) -> String {
    let indent = leading_whitespace(line);
    let rest = &line[indent.len()..];
    let commas = rest.len() - rest.trim_start_matches(',').len();
    let after = &rest[commas..];
    if commas > 0 && (after.starts_with('*') || after.starts_with("#+")) {
        format!("{indent}{}", &rest[1..])
    } else {
        line.to_string()
    }
}

/// Puts a comma before a line that would read as a headline or a keyword.
fn escape(line: &str) -> String {
    let indent = leading_whitespace(line);
    let rest = &line[indent.len()..];
    let after = rest.trim_start_matches(',');
    if after.starts_with('*') || after.starts_with("#+") {
        format!("{indent},{rest}")
    } else {
        line.to_string()
    }
}

fn leading_whitespace(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

/// The indentation every non-blank line shares, in bytes of whitespace.
pub(crate) fn common_indent(lines: &[&str]) -> usize {
    lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| leading_whitespace(line).len())
        .min()
        .unwrap_or(0)
}

/// What follows `keyword` at the start of a line, matched case-insensitively.
fn keyword_rest<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let trimmed = line.trim_start();
    let head = trimmed.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &trimmed[keyword.len()..];
    // `#+begin_srcfoo` is not `#+begin_src`.
    (rest.is_empty() || rest.starts_with(char::is_whitespace) || keyword.ends_with(':'))
        .then_some(rest)
}

/// `rust -n :tangle src/main.rs :mkdirp yes` — the language, then header
/// arguments. Switches like `-n` and `-r` are for export and are skipped.
fn parse_parameters(params: &str) -> (Option<String>, Vec<(String, String)>) {
    let params = params.trim();
    let (language, rest) = match params.split_once(char::is_whitespace) {
        Some((first, rest)) if !first.starts_with(':') => (Some(first), rest),
        None if !params.is_empty() && !params.starts_with(':') => (Some(params), ""),
        _ => (None, params),
    };

    let rest = rest.trim_start();
    // Arguments start at the first `:` token; anything before it is switches.
    let header_from = if rest.starts_with(':') {
        Some(0)
    } else {
        rest.find(" :").map(|at| at + 1)
    };
    let header = header_from.map_or_else(Vec::new, |at| parse_header_args(&rest[at..]));
    (language.map(str::to_string), header)
}

/// `:tangle "my file.rs" :mkdirp yes` as key/value pairs, quotes removed.
fn parse_header_args(args: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut value: Vec<String> = Vec::new();
    let mut key: Option<String> = None;

    let finish = |key: String, value: &[String]| {
        let joined = value.join(" ");
        // A value that is one quoted string is that string; quotes inside a
        // longer value (`:var who="you"`, `:cmdline a "b c"`) are part of
        // what it says, and whoever reads it needs them.
        let unquoted = match joined
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        {
            Some(inner) if !inner.contains('"') => inner.to_string(),
            _ => joined,
        };
        (key, unquoted)
    };

    for token in split_quoted(args) {
        if let Some(name) = token.strip_prefix(':').filter(|name| !name.is_empty()) {
            if let Some(key) = key.take() {
                pairs.push(finish(key, &value));
            }
            key = Some(name.to_ascii_lowercase());
            value.clear();
        } else if key.is_some() {
            value.push(token);
        }
    }
    if let Some(key) = key {
        pairs.push(finish(key, &value));
    }

    pairs
}

/// Splits on whitespace outside double quotes, keeping the quotes.
fn split_quoted(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quoted = false;

    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// ── Tangling ────────────────────────────────────────────────────────────────

/// One file tangling writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tangled {
    pub path: PathBuf,
    pub content: String,
    /// How many blocks went into it.
    pub blocks: usize,
    /// `:shebang` was given, so the file should be executable.
    pub executable: bool,
    /// `:mkdirp yes`: create missing parent directories.
    pub mkdirp: bool,
}

/// Why tangling could not produce its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TangleError {
    /// A noweb reference names no block.
    UnknownReference(String),
    /// A noweb reference ends up including itself.
    Cycle(String),
}

impl std::fmt::Display for TangleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TangleError::UnknownReference(name) => {
                write!(f, "<<{name}>> names no block")
            }
            TangleError::Cycle(name) => write!(f, "<<{name}>> includes itself"),
        }
    }
}

/// The extension Org gives a file tangled with `:tangle yes`.
///
/// Org's `org-babel-tangle-lang-exts` plus what the common `ob-*` libraries
/// add; for any other language, Org uses the language's own name, and so
/// does this.
pub fn extension(language: &str) -> String {
    let known = match language {
        "emacs-lisp" | "elisp" => "el",
        "python" => "py",
        "rust" => "rs",
        "C" | "c" => "c",
        "C++" | "cpp" => "cpp",
        "D" => "d",
        "js" | "javascript" => "js",
        "typescript" | "ts" => "ts",
        "sh" | "shell" | "bash" | "zsh" => "sh",
        "ruby" => "rb",
        "haskell" => "hs",
        "ocaml" => "ml",
        "perl" => "pl",
        "latex" => "tex",
        "scheme" => "scm",
        "clojure" => "clj",
        "julia" => "jl",
        "R" => "R",
        _ => return language.to_string(),
    };
    known.to_string()
}

/// The header arguments that apply to `block`: the file's, then each
/// enclosing subtree's, then the block's own, later overriding earlier.
pub fn effective_header(text: &str, block: &SourceBlock) -> Vec<(String, String)> {
    let settings = FileSettings::scan(text);
    let language = block.language.as_deref().unwrap_or("");
    let specific = format!("header-args:{}", language.to_ascii_lowercase());

    let mut header = Vec::new();
    for (key, value) in &settings.properties {
        let key = key.trim_end_matches('+');
        if key == "header-args" || key == specific {
            header.extend(parse_header_args(value));
        }
    }

    // Ancestors, outermost first.
    let lines: Vec<&str> = text.lines().collect();
    let mut ancestors = Vec::new();
    let mut deepest = usize::MAX;
    for at in (0..block.begin).rev() {
        if let Some(level) = crate::restructure::headline_level(lines[at]) {
            if level < deepest {
                ancestors.push(at);
                deepest = level;
            }
        }
    }
    for headline in ancestors.into_iter().rev() {
        for (key, value) in drawer_properties(&lines, headline) {
            let key = key.trim_end_matches('+').to_ascii_lowercase();
            if key == "header-args" || key == specific {
                header.extend(parse_header_args(&value));
            }
        }
    }

    header.extend(block.header.iter().cloned());
    header
}

/// The properties of the headline at `headline`, with names that may carry
/// colons of their own: `:header-args:rust: …` names `header-args:rust`,
/// because a property name ends at the colon followed by a blank.
fn drawer_properties(lines: &[&str], headline: usize) -> Vec<(String, String)> {
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
        return Vec::new();
    }

    lines[at + 1..]
        .iter()
        .take_while(|line| !line.trim().eq_ignore_ascii_case(":END:"))
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(':')?;
            let split = rest
                .char_indices()
                .find(|&(i, c)| {
                    c == ':' && rest[i + 1..].chars().next().is_none_or(char::is_whitespace)
                })?
                .0;
            Some((
                rest[..split].to_string(),
                rest[split + 1..].trim().to_string(),
            ))
        })
        .collect()
}

fn header_value<'a>(header: &'a [(String, String)], key: &str) -> Option<&'a str> {
    header
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// Every file `text` tangles to, with the content it should have.
///
/// `org_path` is the Org file's own path: `:tangle yes` names a file after
/// it, and a relative target is relative to its directory. Nothing is
/// written here; the caller decides whether to.
pub fn tangle(text: &str, org_path: &Path) -> Result<Vec<Tangled>, TangleError> {
    let all = blocks(text);
    let dir = org_path.parent().unwrap_or(Path::new(""));
    let stem = org_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();

    // Noweb references find blocks by name, and by `:noweb-ref`.
    let headers: Vec<Vec<(String, String)>> = all
        .iter()
        .map(|block| effective_header(text, block))
        .collect();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, block) in all.iter().enumerate() {
        if let Some(name) = &block.name {
            by_name.entry(name.clone()).or_default().push(index);
        }
        if let Some(reference) = header_value(&headers[index], "noweb-ref") {
            by_name
                .entry(reference.to_string())
                .or_default()
                .push(index);
        }
    }
    let bodies: Vec<String> = all.iter().map(|block| body(text, block)).collect();

    let mut files: Vec<Tangled> = Vec::new();
    for (index, block) in all.iter().enumerate() {
        let header = &headers[index];
        let target = match header_value(header, "tangle").map(str::trim) {
            None | Some("") | Some("no") => continue,
            Some("yes") => {
                let language = block.language.as_deref().unwrap_or("txt");
                dir.join(format!("{stem}.{}", extension(language)))
            }
            Some(path) => dir.join(path),
        };

        let expand = matches!(
            header_value(header, "noweb"),
            Some("yes" | "tangle" | "no-export" | "strip-export" | "strip-tangle")
        );
        let code = if expand {
            expand_noweb(&bodies[index], &bodies, &by_name, &mut Vec::new())?
        } else {
            bodies[index].clone()
        };

        let padline = header_value(header, "padline") != Some("no");
        let mkdirp = header_value(header, "mkdirp") == Some("yes");
        let shebang = header_value(header, "shebang").filter(|s| !s.is_empty());

        match files.iter_mut().find(|file| file.path == target) {
            Some(file) => {
                if padline && !file.content.is_empty() {
                    file.content.push('\n');
                }
                file.content.push_str(&code);
                file.blocks += 1;
                file.mkdirp |= mkdirp;
            }
            None => {
                let mut content = String::new();
                if let Some(shebang) = shebang {
                    content.push_str(shebang);
                    content.push('\n');
                }
                content.push_str(&code);
                files.push(Tangled {
                    path: target,
                    content,
                    blocks: 1,
                    executable: shebang.is_some(),
                    mkdirp,
                });
            }
        }
    }

    Ok(files)
}

/// Replaces each `<<name>>` on a line with the named blocks' code, repeating
/// what came before the reference on every inserted line — which is how a
/// reference indented inside a function stays indented.
fn expand_noweb(
    code: &str,
    bodies: &[String],
    by_name: &HashMap<String, Vec<usize>>,
    stack: &mut Vec<String>,
) -> Result<String, TangleError> {
    let mut out = String::new();

    for line in code.lines() {
        let reference = line.find("<<").and_then(|open| {
            let close = line[open + 2..].find(">>")? + open + 2;
            let name = &line[open + 2..close];
            (!name.is_empty() && !name.contains(char::is_whitespace)).then_some((
                open,
                name,
                close + 2,
            ))
        });
        let Some((open, name, after)) = reference else {
            out.push_str(line);
            out.push('\n');
            continue;
        };

        if stack.iter().any(|seen| seen == name) {
            return Err(TangleError::Cycle(name.to_string()));
        }
        let indices = by_name
            .get(name)
            .ok_or_else(|| TangleError::UnknownReference(name.to_string()))?;

        stack.push(name.to_string());
        let mut included = String::new();
        for &index in indices {
            included.push_str(&expand_noweb(&bodies[index], bodies, by_name, stack)?);
        }
        stack.pop();

        let prefix = &line[..open];
        let suffix = &line[after..];
        let included: Vec<&str> = included.lines().collect();
        if included.is_empty() {
            out.push_str(prefix);
            out.push_str(suffix);
            out.push('\n');
        }
        for (i, inner) in included.iter().enumerate() {
            out.push_str(prefix);
            out.push_str(inner);
            if i + 1 == included.len() {
                out.push_str(suffix);
            }
            out.push('\n');
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "\
#+PROPERTY: header-args :mkdirp yes
* Code
:PROPERTIES:
:header-args:rust: :tangle src/lib.rs
:END:
#+NAME: helper
#+begin_src rust
  fn helper() -> u32 {
      42
  }
#+end_src

#+RESULTS:
: 42

#+begin_src rust :tangle no
fn scratch() {}
#+end_src

#+begin_src python :tangle yes
print(1)
#+end_src
";

    #[test]
    fn blocks_are_found_with_their_language_name_and_header() {
        let found = blocks(FILE);
        assert_eq!(found.len(), 3);
        assert_eq!(found[0].begin, 6);
        assert_eq!(found[0].end, 10);
        assert_eq!(found[0].language.as_deref(), Some("rust"));
        assert_eq!(found[0].name.as_deref(), Some("helper"));
        assert_eq!(found[1].header("tangle"), Some("no"));
    }

    #[test]
    fn an_unterminated_block_is_not_a_block() {
        assert!(blocks("#+begin_src rust\nfn main() {}\n").is_empty());
    }

    #[test]
    fn a_name_above_an_earlier_block_stays_with_it() {
        let text = "#+NAME: first\n#+begin_src sh\n#+end_src\n#+begin_src sh\n#+end_src\n";
        let found = blocks(text);
        assert_eq!(found[0].name.as_deref(), Some("first"));
        assert_eq!(found[1].name, None);
    }

    #[test]
    fn keywords_are_matched_in_any_case() {
        let found = blocks("#+BEGIN_SRC sh :tangle a.sh\necho\n#+END_SRC\n");
        assert_eq!(found[0].header("tangle"), Some("a.sh"));
    }

    #[test]
    fn a_header_line_above_the_block_counts() {
        let found = blocks("#+HEADER: :tangle x.rs\n#+begin_src rust :mkdirp yes\n#+end_src\n");
        assert_eq!(found[0].header("tangle"), Some("x.rs"));
        assert_eq!(found[0].header("mkdirp"), Some("yes"));
    }

    #[test]
    fn quoted_header_values_keep_their_spaces() {
        let found =
            blocks("#+begin_src sh :shebang \"#!/bin/sh -e\" :tangle \"my file.sh\"\n#+end_src\n");
        assert_eq!(found[0].header("shebang"), Some("#!/bin/sh -e"));
        assert_eq!(found[0].header("tangle"), Some("my file.sh"));
    }

    #[test]
    fn a_body_comes_out_without_its_shared_indentation() {
        let block = &blocks(FILE)[0];
        assert_eq!(body(FILE, block), "fn helper() -> u32 {\n    42\n}\n");
    }

    #[test]
    fn escaped_lines_are_unescaped_and_escaped_again() {
        let text = "#+begin_src org\n,* A headline\n,#+title: x\n,,* a comma\nplain\n#+end_src\n";
        let block = &blocks(text)[0];
        let code = body(text, block);
        assert_eq!(code, "* A headline\n#+title: x\n,* a comma\nplain\n");
        assert_eq!(replace_body(text, block, &code), text);
    }

    #[test]
    fn replacing_a_body_keeps_the_indentation_it_had() {
        let block = &blocks(FILE)[0];
        let out = replace_body(FILE, block, "fn helper() -> u32 {\n    43\n}\n");
        assert!(
            out.contains("#+begin_src rust\n  fn helper() -> u32 {\n      43\n  }\n#+end_src"),
            "{out}"
        );
        // Round trip: nothing changed means nothing changes.
        assert_eq!(replace_body(FILE, block, &body(FILE, block)), FILE);
    }

    #[test]
    fn an_empty_block_takes_the_indentation_of_its_begin_line() {
        let text = "  #+begin_src sh\n  #+end_src\n";
        let block = &blocks(text)[0];
        assert_eq!(
            replace_body(text, block, "echo hi\n"),
            "  #+begin_src sh\n  echo hi\n  #+end_src\n"
        );
    }

    #[test]
    fn a_block_and_its_result_find_each_other() {
        let found = blocks(FILE);
        assert_eq!(result_of(FILE, &found[0]), Some(12));
        assert_eq!(result_of(FILE, &found[1]), None);
        assert_eq!(block_of_result(FILE, &found, 12), Some(&found[0]));
        // From the output under the keyword, too.
        assert_eq!(block_of_result(FILE, &found, 13), Some(&found[0]));
        assert_eq!(block_of_result(FILE, &found, 14), None);
    }

    #[test]
    fn a_named_result_is_found_wherever_it_is() {
        let text =
            "#+NAME: sum\n#+begin_src sh\necho 3\n#+end_src\n\nProse.\n\n#+RESULTS: sum\n: 3\n";
        let found = blocks(text);
        assert_eq!(result_of(text, &found[0]), Some(7));
        assert_eq!(block_of_result(text, &found, 7), Some(&found[0]));
    }

    #[test]
    fn moving_between_blocks() {
        let found = blocks(FILE);
        assert_eq!(next_block(&found, 0).map(|b| b.begin), Some(6));
        assert_eq!(next_block(&found, 8).map(|b| b.begin), Some(15));
        assert_eq!(previous_block(&found, 16).map(|b| b.begin), Some(15));
        assert_eq!(previous_block(&found, 15).map(|b| b.begin), Some(6));
        assert_eq!(previous_block(&found, 6), None);
    }

    #[test]
    fn header_arguments_are_inherited_file_then_subtree_then_block() {
        let found = blocks(FILE);
        let header = effective_header(FILE, &found[0]);
        assert_eq!(header_value(&header, "tangle"), Some("src/lib.rs"));
        assert_eq!(header_value(&header, "mkdirp"), Some("yes"));

        // The block's own `:tangle no` wins over the subtree's.
        let header = effective_header(FILE, &found[1]);
        assert_eq!(header_value(&header, "tangle"), Some("no"));

        // A language-specific default does not reach another language.
        let header = effective_header(FILE, &found[2]);
        assert_eq!(header_value(&header, "tangle"), Some("yes"));
    }

    #[test]
    fn tangling_writes_each_target_once_with_its_blocks_in_order() {
        let files = tangle(FILE, Path::new("/notes/literate.org")).unwrap();
        assert_eq!(files.len(), 2);

        assert_eq!(files[0].path, Path::new("/notes/src/lib.rs"));
        assert_eq!(files[0].content, "fn helper() -> u32 {\n    42\n}\n");
        assert!(files[0].mkdirp);

        // `:tangle yes` is named after the Org file.
        assert_eq!(files[1].path, Path::new("/notes/literate.py"));
        assert_eq!(files[1].content, "print(1)\n");
    }

    #[test]
    fn blocks_for_the_same_file_are_separated_unless_padline_is_off() {
        let text = "#+begin_src sh :tangle a.sh\none\n#+end_src\n#+begin_src sh :tangle a.sh\ntwo\n#+end_src\n";
        let files = tangle(text, Path::new("x.org")).unwrap();
        assert_eq!(files[0].content, "one\n\ntwo\n");
        assert_eq!(files[0].blocks, 2);

        let text = text.replace(":tangle a.sh\ntwo", ":tangle a.sh :padline no\ntwo");
        assert_eq!(
            tangle(&text, Path::new("x.org")).unwrap()[0].content,
            "one\ntwo\n"
        );
    }

    #[test]
    fn a_shebang_goes_first_and_makes_the_file_executable() {
        let text = "#+begin_src sh :tangle run.sh :shebang #!/bin/sh\necho hi\n#+end_src\n";
        let files = tangle(text, Path::new("x.org")).unwrap();
        assert_eq!(files[0].content, "#!/bin/sh\necho hi\n");
        assert!(files[0].executable);
    }

    #[test]
    fn noweb_references_expand_with_their_prefix_only_when_asked() {
        let text = "\
#+NAME: body
#+begin_src rust
let x = 1;
println!(\"{x}\");
#+end_src
#+begin_src rust :tangle main.rs :noweb yes
fn main() {
    <<body>>
}
#+end_src
#+begin_src rust :tangle plain.rs
<<body>>
#+end_src
";
        let files = tangle(text, Path::new("x.org")).unwrap();
        assert_eq!(
            files[0].content,
            "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n"
        );
        assert_eq!(files[1].content, "<<body>>\n");
    }

    #[test]
    fn noweb_ref_collects_several_blocks_under_one_name() {
        let text = "\
#+begin_src sh :noweb-ref steps
echo one
#+end_src
#+begin_src sh :noweb-ref steps
echo two
#+end_src
#+begin_src sh :tangle all.sh :noweb yes
<<steps>>
#+end_src
";
        let files = tangle(text, Path::new("x.org")).unwrap();
        assert_eq!(files[0].content, "echo one\necho two\n");
    }

    #[test]
    fn a_broken_noweb_reference_is_an_error_not_a_guess() {
        let unknown = "#+begin_src sh :tangle a.sh :noweb yes\n<<nowhere>>\n#+end_src\n";
        assert_eq!(
            tangle(unknown, Path::new("x.org")),
            Err(TangleError::UnknownReference("nowhere".to_string()))
        );

        let cycle = "#+NAME: a\n#+begin_src sh :tangle a.sh :noweb yes\n<<a>>\n#+end_src\n";
        assert_eq!(
            tangle(cycle, Path::new("x.org")),
            Err(TangleError::Cycle("a".to_string()))
        );
    }
}
