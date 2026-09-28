//! Babel: running a source block and writing what it printed back under it.
//!
//! This module decides *what* to run and *how the results read*; it runs
//! nothing. Running is the editor's, behind two gates this crate cannot
//! check: the workspace must be trusted for code execution, and each run is
//! confirmed. Nothing here is reached by opening, exporting or tangling a
//! file — Org's export evaluates blocks by default, and this fork's does not.

use std::path::{Path, PathBuf};

use crate::source::{self, SourceBlock};

/// How a block's results are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collect {
    /// What the program printed.
    Output,
    /// The value of the block, which for Python is what `return` gives and
    /// for a shell is what it printed.
    Value,
}

/// How a block's results are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `: line`, or an example block when long.
    Verbatim,
    /// As printed, to be read as Org.
    Raw,
    /// Not written at all.
    Silent,
}

/// Everything the editor needs to run a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The program and its arguments, before the script's path.
    pub program: Vec<String>,
    /// What the script file is named with, so the interpreter recognises it.
    pub extension: String,
    /// The script to write and run.
    pub script: String,
    /// Where to run it: `:dir`, or the Org file's directory.
    pub dir: PathBuf,
    /// Arguments after the script, from `:cmdline`.
    pub args: Vec<String>,
    pub collect: Collect,
    pub format: Format,
    /// The value file a Python value block writes to is passed as the first
    /// argument; this says to read it rather than stdout.
    pub value_file: bool,
    /// The block's name, which a named block's results carry.
    pub name: Option<String>,
    /// The language as the block gives it, for messages.
    pub language: String,
}

/// Why a block cannot be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BabelError {
    NoLanguage,
    /// Nothing here knows how to run it.
    Unsupported(String),
    /// A `:var` this cannot pass to the language.
    Variable(String),
}

impl std::fmt::Display for BabelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BabelError::NoLanguage => write!(f, "the block names no language"),
            BabelError::Unsupported(language) => {
                write!(f, "running {language} blocks is not supported")
            }
            BabelError::Variable(why) => write!(f, "{why}"),
        }
    }
}

/// The interpreter for a language and the extension its scripts carry.
///
/// Only interpreted languages: a compiled one needs a build step, and
/// `emacs-lisp` needs Emacs.
fn interpreter(language: &str) -> Option<(Vec<&'static str>, &'static str)> {
    Some(match language {
        "sh" | "shell" => (vec!["sh"], "sh"),
        "bash" => (vec!["bash"], "sh"),
        "zsh" => (vec!["zsh"], "sh"),
        "fish" => (vec!["fish"], "fish"),
        "python" | "python3" => (vec!["python3"], "py"),
        "ruby" => (vec!["ruby"], "rb"),
        "perl" => (vec!["perl"], "pl"),
        "lua" => (vec!["lua"], "lua"),
        "js" | "javascript" | "node" => (vec!["node"], "js"),
        "awk" => (vec!["awk", "-f"], "awk"),
        "R" => (vec!["Rscript"], "R"),
        "julia" => (vec!["julia"], "jl"),
        _ => return None,
    })
}

fn is_shell(language: &str) -> bool {
    matches!(language, "sh" | "shell" | "bash" | "zsh" | "fish")
}

/// `:var name=value`, for the literal values a script can take directly: a
/// number or a quoted string. A reference to another block or a table is
/// Babel's own evaluation, which this does not do.
fn variables(header: &[(String, String)]) -> Result<Vec<(String, String)>, BabelError> {
    let mut vars = Vec::new();
    for (key, value) in header {
        if key != "var" {
            continue;
        }
        for assignment in split_assignments(value) {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| BabelError::Variable(format!(":var {assignment} has no value")))?;
            let name = name.trim();
            let value = value.trim();
            let literal = value.parse::<f64>().is_ok()
                || (value.len() >= 2 && value.starts_with('"') && value.ends_with('"'));
            if !literal {
                return Err(BabelError::Variable(format!(
                    ":var {name}={value} refers to something to evaluate; only numbers and quoted strings can be passed"
                )));
            }
            vars.push((name.to_string(), value.to_string()));
        }
    }
    Ok(vars)
}

/// `a=1 b="two words"` into its assignments, keeping quoted spaces.
fn split_assignments(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The script with variables set before the code.
fn preamble(language: &str, vars: &[(String, String)]) -> Result<String, BabelError> {
    let mut out = String::new();
    for (name, value) in vars {
        let line = if is_shell(language) {
            // A shell variable is a string: the quotes the Org value carries
            // are shell quotes too.
            format!("{name}={value}")
        } else {
            match language {
                "python" | "python3" | "ruby" | "julia" => format!("{name} = {value}"),
                "js" | "javascript" | "node" => format!("const {name} = {value};"),
                "perl" => format!("my ${name} = {value};"),
                "lua" => format!("local {name} = {value}"),
                "R" => format!("{name} <- {value}"),
                other => {
                    return Err(BabelError::Variable(format!(
                        ":var is not passed to {other} blocks"
                    )))
                }
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// Wraps Python code so that its `return` is the block's value, the way
/// Org's non-session Python does: the body becomes a function, and what it
/// returns is written to the file named by the first argument.
fn python_value(code: &str) -> String {
    let mut out = String::from("import sys\ndef __babel_main():\n");
    let mut empty = true;
    for line in code.lines() {
        if !line.trim().is_empty() {
            empty = false;
        }
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    if empty {
        out.push_str("    pass\n");
    }
    out.push_str(
        "__babel_value = __babel_main()\nwith open(sys.argv[1], \"w\") as __babel_file:\n    __babel_file.write(str(__babel_value))\n",
    );
    out
}

/// What running `block` means, from its header arguments.
pub fn plan(text: &str, block: &SourceBlock, org_path: &Path) -> Result<Plan, BabelError> {
    let language = block.language.clone().ok_or(BabelError::NoLanguage)?;
    let (program, extension) =
        interpreter(&language).ok_or_else(|| BabelError::Unsupported(language.clone()))?;
    let header = source::effective_header(text, block);
    let value = |key: &str| {
        header
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };

    // `:results` takes several words in any order.
    let results: Vec<&str> =
        value("results").map_or_else(Vec::new, |r| r.split_whitespace().collect());
    let collect = if results.contains(&"output") {
        Collect::Output
    } else {
        Collect::Value
    };
    let format = if results.contains(&"silent") || results.contains(&"none") {
        Format::Silent
    } else if results.contains(&"raw") || results.contains(&"org") {
        Format::Raw
    } else {
        Format::Verbatim
    };

    let vars = variables(&header)?;
    let code = source::body(text, block);
    let python = matches!(language.as_str(), "python" | "python3");
    let value_file = collect == Collect::Value && python;
    let script = format!(
        "{}{}",
        preamble(&language, &vars)?,
        if value_file {
            python_value(&code)
        } else {
            code
        }
    );

    let org_dir = org_path.parent().unwrap_or(Path::new(""));
    let dir = match value("dir") {
        Some(dir) if !dir.is_empty() => org_dir.join(dir),
        _ => org_dir.to_path_buf(),
    };

    Ok(Plan {
        program: program.into_iter().map(str::to_string).collect(),
        extension: extension.to_string(),
        script,
        dir,
        args: value("cmdline").map_or_else(Vec::new, |line| {
            split_assignments(line)
                .into_iter()
                .map(|arg| arg.trim_matches('"').to_string())
                .collect()
        }),
        collect,
        format,
        value_file,
        name: block.name.clone(),
        language,
    })
}

/// Whether a value was asked for from a language that can only give its
/// output, which the caller should say rather than hide.
pub fn value_is_output(plan: &Plan) -> bool {
    plan.collect == Collect::Value && !plan.value_file && !is_shell(&plan.language)
}

/// Org puts results of this many lines or more in an example block rather
/// than `: ` lines (`org-babel-min-lines-for-block-output`).
const BLOCK_OUTPUT_LINES: usize = 10;

/// The lines a result is written as, `#+RESULTS:` included.
pub fn results_lines(output: &str, plan: &Plan) -> Vec<String> {
    let keyword = match &plan.name {
        Some(name) => format!("#+RESULTS: {name}"),
        None => "#+RESULTS:".to_string(),
    };
    let body: Vec<&str> = output.trim_end_matches('\n').lines().collect();
    let mut out = vec![keyword];

    match plan.format {
        Format::Silent => return Vec::new(),
        Format::Raw => out.extend(body.iter().map(|line| line.to_string())),
        Format::Verbatim if body.len() >= BLOCK_OUTPUT_LINES => {
            out.push("#+begin_example".to_string());
            out.extend(body.iter().map(|line| {
                // A line that would end the example, or read as a headline,
                // is escaped as it is in any block.
                let trimmed = line.trim_start();
                if trimmed.starts_with('*') || trimmed.starts_with("#+") {
                    format!(",{line}")
                } else {
                    line.to_string()
                }
            }));
            out.push("#+end_example".to_string());
        }
        Format::Verbatim => out.extend(body.iter().map(|line| {
            if line.is_empty() {
                ":".to_string()
            } else {
                format!(": {line}")
            }
        })),
    }
    out
}

/// Where the results under `block` are, as a line range, if it has any.
fn results_range(lines: &[&str], text: &str, block: &SourceBlock) -> Option<(usize, usize)> {
    let start = source::result_of(text, block)?;
    let mut end = start + 1;
    if lines.get(end).is_some_and(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("#+begin_")
    }) {
        let closing = lines[end]
            .split_whitespace()
            .next()
            .map(|begin| begin.to_ascii_lowercase().replacen("#+begin_", "#+end_", 1))?;
        while end < lines.len() && !lines[end].trim().eq_ignore_ascii_case(&closing) {
            end += 1;
        }
        return Some((start, (end + 1).min(lines.len())));
    }
    while end < lines.len()
        && !lines[end].trim().is_empty()
        && crate::restructure::headline_level(lines[end]).is_none()
    {
        end += 1;
    }
    Some((start, end))
}

/// `text` with `results` replacing whatever results `block` had, or placed
/// after it, one blank line down, when it had none.
pub fn write_results(text: &str, block: &SourceBlock, results: &[String]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();

    match results_range(&lines, text, block) {
        Some((start, end)) => {
            out.splice(start..end, results.iter().cloned());
        }
        None if results.is_empty() => {}
        None => {
            let mut inserted = vec![String::new()];
            inserted.extend(results.iter().cloned());
            // Keep a blank line between the results and what follows.
            if lines
                .get(block.end + 1)
                .is_some_and(|line| !line.trim().is_empty())
            {
                inserted.push(String::new());
            }
            out.splice(block.end + 1..block.end + 1, inserted);
        }
    }

    crate::restructure::rejoin(&out, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_of(text: &str) -> Result<Plan, BabelError> {
        let block = source::blocks(text).remove(0);
        plan(text, &block, Path::new("/notes/n.org"))
    }

    #[test]
    fn a_shell_block_runs_in_the_file_s_directory() {
        let plan = plan_of("#+begin_src sh\necho hi\n#+end_src\n").unwrap();
        assert_eq!(plan.program, ["sh"]);
        assert_eq!(plan.script, "echo hi\n");
        assert_eq!(plan.dir, Path::new("/notes"));
        assert_eq!(plan.format, Format::Verbatim);
    }

    #[test]
    fn dir_and_cmdline_are_read() {
        let plan =
            plan_of("#+begin_src sh :dir sub :cmdline a \"b c\"\necho\n#+end_src\n").unwrap();
        assert_eq!(plan.dir, Path::new("/notes/sub"));
        assert_eq!(plan.args, ["a", "b c"]);
    }

    #[test]
    fn python_values_come_from_return() {
        let plan = plan_of("#+begin_src python\nx = 2\nreturn x * 21\n#+end_src\n").unwrap();
        assert!(plan.value_file);
        assert_eq!(
            plan.script,
            "import sys\ndef __babel_main():\n    x = 2\n    return x * 21\n\
             __babel_value = __babel_main()\nwith open(sys.argv[1], \"w\") as __babel_file:\n    __babel_file.write(str(__babel_value))\n"
        );

        let output = plan_of("#+begin_src python :results output\nprint(1)\n#+end_src\n").unwrap();
        assert!(!output.value_file);
        assert_eq!(output.script, "print(1)\n");
    }

    #[test]
    fn variables_are_set_before_the_code() {
        let plan = plan_of(
            "#+begin_src python :results output :var n=3 who=\"you\"\nprint(n, who)\n#+end_src\n",
        )
        .unwrap();
        assert_eq!(plan.script, "n = 3\nwho = \"you\"\nprint(n, who)\n");

        let shell = plan_of("#+begin_src sh :var n=3\necho $n\n#+end_src\n").unwrap();
        assert_eq!(shell.script, "n=3\necho $n\n");

        assert!(matches!(
            plan_of("#+begin_src sh :var t=my-table\necho\n#+end_src\n"),
            Err(BabelError::Variable(_))
        ));
    }

    #[test]
    fn languages_nothing_can_run_say_so() {
        assert_eq!(
            plan_of("#+begin_src rust\nfn main() {}\n#+end_src\n"),
            Err(BabelError::Unsupported("rust".to_string()))
        );
        assert_eq!(
            plan_of("#+begin_src\nx\n#+end_src\n"),
            Err(BabelError::NoLanguage)
        );
    }

    #[test]
    fn results_are_verbatim_lines_or_an_example_when_long() {
        let plan = plan_of("#+begin_src sh\necho\n#+end_src\n").unwrap();
        assert_eq!(
            results_lines("a\n\nb\n", &plan),
            ["#+RESULTS:", ": a", ":", ": b"]
        );

        let long: String = (1..=10).map(|n| format!("{n}\n")).collect();
        let lines = results_lines(&long, &plan);
        assert_eq!(lines[1], "#+begin_example");
        assert_eq!(lines.last().unwrap(), "#+end_example");
    }

    #[test]
    fn a_named_block_s_results_carry_its_name() {
        let plan = plan_of("#+NAME: sum\n#+begin_src sh\necho 3\n#+end_src\n").unwrap();
        assert_eq!(results_lines("3", &plan), ["#+RESULTS: sum", ": 3"]);
    }

    #[test]
    fn results_go_under_the_block_and_replace_old_ones() {
        let text = "#+begin_src sh\necho hi\n#+end_src\nAfter.\n";
        let block = source::blocks(text).remove(0);
        let first = write_results(text, &block, &["#+RESULTS:".into(), ": hi".into()]);
        assert_eq!(
            first,
            "#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: hi\n\nAfter.\n"
        );

        let block = source::blocks(&first).remove(0);
        let second = write_results(&first, &block, &["#+RESULTS:".into(), ": bye".into()]);
        assert_eq!(
            second,
            "#+begin_src sh\necho hi\n#+end_src\n\n#+RESULTS:\n: bye\n\nAfter.\n"
        );
    }

    #[test]
    fn example_results_are_replaced_whole() {
        let text = "#+begin_src sh\nx\n#+end_src\n\n#+RESULTS:\n#+begin_example\n1\n\n2\n#+end_example\nAfter.\n";
        let block = source::blocks(text).remove(0);
        let out = write_results(text, &block, &["#+RESULTS:".into(), ": new".into()]);
        assert_eq!(
            out,
            "#+begin_src sh\nx\n#+end_src\n\n#+RESULTS:\n: new\nAfter.\n"
        );
    }
}
