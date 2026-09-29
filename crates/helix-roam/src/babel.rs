//! Babel: running a source block and writing what it printed back under it.
//!
//! This module decides *what* to run and *how the results read*; it runs
//! nothing. Running is the editor's, behind two gates this crate cannot
//! check: the workspace must be trusted for code execution, and each run is
//! confirmed. Nothing here is reached by opening, exporting or tangling a
//! file — Org's export evaluates blocks by default, and this fork's does not.
//!
//! A plan's commands carry placeholders the editor fills in: `{src}` the
//! script it writes, `{bin}` what a compiled language builds, `{file}` the
//! file a block's results go to.

use std::path::{Path, PathBuf};

use crate::source::{self, SourceBlock};

pub mod detangle;
pub mod values;

use values::Value;

/// How a block's results are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collect {
    /// What the program printed.
    Output,
    /// The value of the block, which for Python is what `return` gives (or
    /// in a session the last expression) and for the others what they
    /// printed.
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
    /// The program and its arguments, `{src}` for the script.
    pub program: Vec<String>,
    /// For a compiled language, the command that builds `{bin}` from
    /// `{src}` first.
    pub build: Option<Vec<String>>,
    /// What the script file is named with, so the interpreter recognises it.
    pub extension: String,
    /// The script to write and run, or to send to the session.
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
    /// `:session`: the name of the interpreter it runs in, which lives on
    /// between runs.
    pub session: Option<String>,
    /// `:cache yes`: what the results are marked with, and compared to, so
    /// an unchanged block is not run again.
    pub cache: Option<String>,
    /// `:file`: the file the results are, as the link to it reads, and
    /// where it is.
    pub file: Option<(String, PathBuf)>,
    /// For a `#+CALL:`, the line it is on, which the results go under.
    pub call: Option<String>,
}

/// Why a block cannot be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BabelError {
    NoLanguage,
    /// Nothing here knows how to run it.
    Unsupported(String),
    /// A `:var` this cannot pass to the language.
    Variable(String),
    /// Something else in the header that cannot be done.
    Header(String),
}

impl std::fmt::Display for BabelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BabelError::NoLanguage => write!(f, "the block names no language"),
            BabelError::Unsupported(language) => {
                write!(f, "running {language} blocks is not supported")
            }
            BabelError::Variable(why) | BabelError::Header(why) => write!(f, "{why}"),
        }
    }
}

/// How a language is run: the program (`{src}` for the script), the
/// extension its scripts carry, and for a compiled one the build.
struct Runner {
    program: Vec<&'static str>,
    extension: &'static str,
    build: Option<Vec<&'static str>>,
}

fn runner(language: &str) -> Option<Runner> {
    let interpreted = |program: Vec<&'static str>, extension| Runner {
        program,
        extension,
        build: None,
    };
    Some(match language {
        "sh" | "shell" => interpreted(vec!["sh", "{src}"], "sh"),
        "bash" => interpreted(vec!["bash", "{src}"], "sh"),
        "zsh" => interpreted(vec!["zsh", "{src}"], "sh"),
        "fish" => interpreted(vec!["fish", "{src}"], "fish"),
        "python" | "python3" => interpreted(vec!["python3", "{src}"], "py"),
        "ruby" => interpreted(vec!["ruby", "{src}"], "rb"),
        "perl" => interpreted(vec!["perl", "{src}"], "pl"),
        "lua" => interpreted(vec!["lua", "{src}"], "lua"),
        "js" | "javascript" | "node" => interpreted(vec!["node", "{src}"], "js"),
        "awk" => interpreted(vec!["awk", "-f", "{src}"], "awk"),
        "R" => interpreted(vec!["Rscript", "{src}"], "R"),
        "julia" => interpreted(vec!["julia", "{src}"], "jl"),
        // Graphviz draws into the block's `:file`, in that file's format.
        "dot" => interpreted(vec!["dot", "-T{ext}", "-o", "{file}", "{src}"], "dot"),
        "C" | "c" => Runner {
            program: vec!["{bin}"],
            extension: "c",
            build: Some(vec!["cc", "{flags}", "-o", "{bin}", "{src}", "{libs}"]),
        },
        "C++" | "cpp" => Runner {
            program: vec!["{bin}"],
            extension: "cpp",
            build: Some(vec!["c++", "{flags}", "-o", "{bin}", "{src}", "{libs}"]),
        },
        "rust" => Runner {
            program: vec!["{bin}"],
            extension: "rs",
            build: Some(vec![
                "rustc",
                "--edition",
                "2021",
                "{flags}",
                "-o",
                "{bin}",
                "{src}",
            ]),
        },
        // `go run` builds and runs in one.
        "go" => interpreted(vec!["go", "run", "{src}"], "go"),
        _ => return None,
    })
}

fn is_shell(language: &str) -> bool {
    matches!(language, "sh" | "shell" | "bash" | "zsh" | "fish")
}

fn is_compiled(language: &str) -> bool {
    matches!(language, "C" | "c" | "C++" | "cpp" | "rust" | "go")
}

/// `:var name=value` pairs, as written: `a=1 b="two words" t=table`.
fn var_pairs(header: &[(String, String)]) -> Result<Vec<(String, String)>, BabelError> {
    let mut pairs = Vec::new();
    for (key, value) in header {
        if key != "var" {
            continue;
        }
        for assignment in split_assignments(value) {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| BabelError::Variable(format!(":var {assignment} has no value")))?;
            pairs.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok(pairs)
}

/// The value a `:var` names: a literal, or the named table, list, example
/// or block results of the file or else of the library. `colnames` keeps
/// a table's column names.
fn resolve(
    name: &str,
    value: &str,
    text: &str,
    library: &[String],
    colnames: bool,
) -> Result<Value, BabelError> {
    if let Some(literal) = Value::literal(value) {
        return Ok(literal);
    }
    // `name()` is the block's results, as `name` is.
    let reference = value.strip_suffix("()").unwrap_or(value);
    if reference.contains('(') {
        return Err(BabelError::Variable(format!(
            ":var {name}={value}: a block called with arguments is a #+CALL:, not a :var"
        )));
    }
    for source in std::iter::once(text).chain(library.iter().map(String::as_str)) {
        match values::named(source, reference, colnames) {
            Ok(Some(found)) => return Ok(found),
            Ok(None) => {}
            Err(why) => return Err(BabelError::Variable(format!(":var {name}: {why}"))),
        }
    }
    Err(BabelError::Variable(format!(
        ":var {name}={value} names nothing in the file or the library"
    )))
}

/// `a=1 b="two words"` into its assignments, keeping quoted spaces; also
/// splits on commas outside quotes, as a `#+CALL:`'s arguments are.
fn split_assignments(value: &str) -> Vec<String> {
    split_words(value, true)
}

/// Words split on whitespace outside quotes, and on commas too if `commas`.
/// Command-line words keep theirs: `-Wl,-rpath,/opt/lib` is one.
fn split_words(value: &str, commas: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in value.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if (c.is_whitespace() || (commas && c == ',')) && !quoted => {
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

/// The lines that set the variables, in the language.
fn preamble(language: &str, vars: &[(String, Value)]) -> Result<String, BabelError> {
    let mut out = String::new();
    for (name, value) in vars {
        out.push_str(&values::assignment(language, name, value)?);
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

fn indent(code: &str) -> String {
    code.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("    {line}\n")
            }
        })
        .collect::<Vec<_>>()
        .concat()
}

/// A compiled language's source: the includes or imports, the variables,
/// and the code inside a `main` when it has none of its own, as Org's
/// `ob-C`, `ob-rust` and `ob-go` write it.
fn compiled_source(
    language: &str,
    code: &str,
    vars: &str,
    header: &dyn Fn(&str) -> Option<String>,
) -> Result<String, BabelError> {
    let wants_main = header("main").as_deref() != Some("no");
    Ok(match language {
        "C" | "c" | "C++" | "cpp" => {
            let cpp = matches!(language, "C++" | "cpp");
            let mut out = String::new();
            match header("includes") {
                Some(includes) => {
                    for include in split_words(&includes, false) {
                        let include = include.trim_matches('"');
                        let include = if include.starts_with('<') {
                            include.to_string()
                        } else {
                            format!("\"{include}\"")
                        };
                        out.push_str(&format!("#include {include}\n"));
                    }
                }
                None if !code.contains("#include") => {
                    let defaults: &[&str] = if cpp {
                        &["<iostream>", "<string>"]
                    } else {
                        &["<stdio.h>", "<stdlib.h>", "<string.h>"]
                    };
                    for include in defaults {
                        out.push_str(&format!("#include {include}\n"));
                    }
                }
                None => {}
            }
            out.push_str(vars);
            if wants_main && !code.contains("main(") {
                out.push_str(&format!(
                    "int main() {{\n{}    return 0;\n}}\n",
                    indent(code)
                ));
            } else {
                out.push_str(code);
            }
            out
        }
        "rust" => {
            if code.contains("fn main") {
                if !vars.is_empty() {
                    return Err(BabelError::Variable(
                        "a Rust block with its own main takes no :var; leave main out and Babel writes it"
                            .to_string(),
                    ));
                }
                code.to_string()
            } else {
                format!("fn main() {{\n{}{}}}\n", indent(vars), indent(code))
            }
        }
        "go" => {
            if code.contains("package main") {
                if !vars.is_empty() {
                    return Err(BabelError::Variable(
                        "a Go block with its own package takes no :var; leave main out and Babel writes it"
                            .to_string(),
                    ));
                }
                code.to_string()
            } else {
                let imports: Vec<String> = match header("imports") {
                    Some(imports) => split_words(&imports, false)
                        .into_iter()
                        .map(|import| format!("\"{}\"", import.trim_matches('"')))
                        .collect(),
                    // What the code uses, as `goimports` would add.
                    None => ["fmt", "os", "strings", "math", "sort", "strconv", "time"]
                        .iter()
                        .filter(|package| code.contains(&format!("{package}.")))
                        .map(|package| format!("\"{package}\""))
                        .collect(),
                };
                let imports = match imports.len() {
                    0 => String::new(),
                    _ => format!("import (\n{}\n)\n\n", imports.join("\n")),
                };
                format!(
                    "package main\n\n{imports}func main() {{\n{}{}}}\n",
                    indent(vars),
                    indent(code)
                )
            }
        }
        _ => code.to_string(),
    })
}

/// FNV-1a, in hex: what `:cache` marks results with. Stable across runs
/// and machines, which the standard library's hasher is not.
fn fingerprint(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

/// What running `block` means, from its header arguments.
pub fn plan(text: &str, block: &SourceBlock, org_path: &Path) -> Result<Plan, BabelError> {
    plan_with(text, block, org_path, &[])
}

/// What running `block` means, `:var`s also found in `library`, the texts
/// of the Library of Babel's files.
pub fn plan_with(
    text: &str,
    block: &SourceBlock,
    org_path: &Path,
    library: &[String],
) -> Result<Plan, BabelError> {
    plan_inner(text, block, org_path, library, &[], &[])
}

/// The plan, with a `#+CALL:`'s arguments and header over the block's own.
fn plan_inner(
    text: &str,
    block: &SourceBlock,
    org_path: &Path,
    library: &[String],
    call_vars: &[(String, String)],
    call_header: &[(String, String)],
) -> Result<Plan, BabelError> {
    let language = block.language.clone().ok_or(BabelError::NoLanguage)?;
    let runner = runner(&language).ok_or_else(|| BabelError::Unsupported(language.clone()))?;
    let mut header = source::effective_header(text, block);
    header.extend(call_header.iter().cloned());
    let value = |key: &str| {
        header
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };

    // `:results` takes several words in any order.
    let results_words = value("results").unwrap_or_default();
    let results: Vec<&str> = results_words.split_whitespace().collect();
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

    // Each variable once, a call's arguments over the block's own.
    let mut pairs = var_pairs(&header)?;
    for (name, value) in call_vars {
        pairs.retain(|(own, _)| own != name);
        pairs.push((name.clone(), value.clone()));
    }
    let colnames = value("colnames").is_some_and(|keep| keep.trim() == "no");
    let mut vars = Vec::new();
    for (name, raw) in &pairs {
        vars.push((name.clone(), resolve(name, raw, text, library, colnames)?));
    }

    let session = value("session")
        .map(|name| name.trim().to_string())
        .filter(|name| name != "none")
        .map(|name| {
            if name.is_empty() || name == "yes" {
                "default".to_string()
            } else {
                name
            }
        });
    let python = matches!(language.as_str(), "python" | "python3");
    if session.is_some() && !python && !matches!(language.as_str(), "sh" | "shell" | "bash" | "zsh")
    {
        return Err(BabelError::Header(format!(
            "{language} blocks have no sessions here; only Python and the shells do"
        )));
    }

    let code = source::body(text, block);
    let preamble = preamble(&language, &vars)?;
    let value_file = collect == Collect::Value && python && session.is_none();
    let script = if is_compiled(&language) {
        compiled_source(&language, &code, &preamble, &|key| value(key))?
    } else if value_file {
        format!("{preamble}{}", python_value(&code))
    } else {
        format!("{preamble}{code}")
    };

    let org_dir = org_path.parent().unwrap_or(Path::new(""));
    let dir = match value("dir").as_deref().map(str::trim) {
        // The entry's attachment directory, as Org's `:dir 'attach`.
        Some("attach" | "'attach") => crate::attach::attach_dir(text, block.begin, org_dir)
            .ok_or_else(|| {
                BabelError::Header(
                    ":dir attach needs the entry to have an :ID: or a :DIR:".to_string(),
                )
            })?,
        Some(dir) if !dir.is_empty() => org_dir.join(dir),
        _ => org_dir.to_path_buf(),
    };

    // `:file`, or the block's name with `:file-ext`, under `:output-dir`.
    let file_name = value("file").filter(|file| !file.is_empty()).or_else(|| {
        let ext = value("file-ext")?;
        Some(format!("{}.{ext}", block.name.as_deref()?))
    });
    let file = file_name.map(|name| {
        let relative = match value("output-dir") {
            Some(out) if !out.is_empty() => Path::new(&out).join(&name),
            _ => PathBuf::from(&name),
        };
        let absolute = dir.join(&relative);
        // The link reads from the Org file, which is where it is followed.
        let link = absolute
            .strip_prefix(org_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| absolute.clone());
        (link.to_string_lossy().replace('\\', "/"), absolute)
    });
    if language == "dot" && file.is_none() {
        return Err(BabelError::Header(
            "a dot block draws into its :file; give it one".to_string(),
        ));
    }

    let args = value("cmdline").map_or_else(Vec::new, |line| {
        split_words(&line, false)
            .into_iter()
            .map(|arg| arg.trim_matches('"').to_string())
            .collect()
    });
    let words = |key: &str| -> Vec<String> {
        value(key)
            .map(|flags| {
                split_words(&flags, false)
                    .into_iter()
                    .map(|flag| flag.trim_matches('"').to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let (flags, libs) = (words("flags"), words("libs"));
    let fill = |command: &[&str]| -> Vec<String> {
        let ext = file
            .as_ref()
            .and_then(|(link, _)| Path::new(link).extension())
            .map(|ext| ext.to_string_lossy().into_owned())
            .unwrap_or_default();
        command
            .iter()
            .flat_map(|token| match *token {
                "{flags}" => flags.clone(),
                "{libs}" => libs.clone(),
                token => vec![token.replace("{ext}", &ext)],
            })
            .collect()
    };

    let cache = (value("cache").as_deref() == Some("yes")).then(|| {
        fingerprint(&[
            &language,
            &script,
            &results_words,
            &args.join("\u{1}"),
            file.as_ref().map_or("", |(link, _)| link.as_str()),
        ])
    });

    Ok(Plan {
        program: fill(&runner.program),
        build: runner.build.as_deref().map(fill),
        extension: runner.extension.to_string(),
        script,
        dir,
        args,
        collect,
        format,
        value_file,
        name: block.name.clone(),
        language,
        session,
        cache,
        file,
        call: None,
    })
}

/// A `#+CALL: name[header](args) header` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub line: usize,
    pub name: String,
    pub args: Vec<(String, String)>,
    pub header: Vec<(String, String)>,
}

/// The `#+CALL:` at `line` of `text`.
pub fn call_at(text: &str, line: usize) -> Option<Call> {
    let raw = text.lines().nth(line)?;
    let trimmed = raw.trim_start();
    let rest = trimmed
        .get(..7)
        .filter(|head| head.eq_ignore_ascii_case("#+call:"))
        .map(|_| trimmed[7..].trim())?;
    let name_end = rest.find(['[', '(']).unwrap_or(rest.len());
    let name = rest[..name_end].trim().to_string();
    if name.is_empty() {
        return None;
    }
    let mut after = &rest[name_end..];
    let mut header = Vec::new();
    // `name[:results output](…)`: header arguments for the block itself.
    if let Some(inner) = after.strip_prefix('[') {
        let close = inner.find(']')?;
        header.extend(source::parse_header_args(&inner[..close]));
        after = &inner[close + 1..];
    }
    let mut args = Vec::new();
    if let Some(inner) = after.strip_prefix('(') {
        let close = inner.rfind(')')?;
        for assignment in split_assignments(&inner[..close]) {
            let (key, value) = assignment.split_once('=')?;
            args.push((key.trim().to_string(), value.trim().to_string()));
        }
        after = &inner[close + 1..];
    }
    header.extend(source::parse_header_args(after));
    Some(Call {
        line,
        name,
        args,
        header,
    })
}

/// What running a `#+CALL:` means: the named block, found in `text` or in
/// the library, run with the call's arguments, its results under the call.
pub fn plan_call(
    text: &str,
    call: &Call,
    org_path: &Path,
    library: &[String],
) -> Result<Plan, BabelError> {
    let sources = std::iter::once(text).chain(library.iter().map(String::as_str));
    // What the variables name is found in the calling file too, not only
    // in the file the block is in.
    let lookup: Vec<String> = std::iter::once(text.to_string())
        .chain(library.iter().cloned())
        .collect();
    for source_text in sources {
        let blocks = source::blocks(source_text);
        if let Some(block) = blocks
            .iter()
            .find(|block| block.name.as_deref() == Some(call.name.as_str()))
        {
            let mut plan = plan_inner(
                source_text,
                block,
                org_path,
                &lookup,
                &call.args,
                &call.header,
            )?;
            // Where it runs and what it reads are the calling file's.
            let org_dir = org_path.parent().unwrap_or(Path::new(""));
            if !block.header.iter().any(|(key, _)| key == "dir") {
                plan.dir = org_dir.to_path_buf();
            }
            plan.name = None;
            plan.call = text.lines().nth(call.line).map(str::to_string);
            return Ok(plan);
        }
    }
    Err(BabelError::Header(format!(
        "no block is named {} in the file or the library",
        call.name
    )))
}

/// Whether a value was asked for from a language that can only give its
/// output, which the caller should say rather than hide.
pub fn value_is_output(plan: &Plan) -> bool {
    plan.collect == Collect::Value
        && !plan.value_file
        && plan.session.is_none()
        && !is_shell(&plan.language)
        && !is_compiled(&plan.language)
        && plan.file.is_none()
}

// ── Sessions ───────────────────────────────────────────────────────────────

/// What a session prints when a chunk is done.
pub const SESSION_DONE: &str = "__babel_done__";

/// The Python a session runs: it reads chunks, runs each in one namespace
/// that lives on, and prints the value of the last expression when a value
/// is asked for. Errors are printed, not fatal: the session stays.
const PYTHON_DRIVER: &str = r##"import ast, contextlib, io, sys, traceback
sys.stderr = sys.stdout
env = {"__name__": "__main__"}
while True:
    head = sys.stdin.readline()
    if not head:
        break
    want_value = head.strip().endswith("value")
    lines = []
    for line in sys.stdin:
        if line.rstrip("\n") == "#__babel_end__":
            break
        lines.append(line)
    try:
        tree = ast.parse("".join(lines))
        last = None
        if want_value and tree.body and isinstance(tree.body[-1], ast.Expr):
            last = ast.Expression(tree.body.pop().value)
        if want_value:
            with contextlib.redirect_stdout(io.StringIO()):
                exec(compile(tree, "<block>", "exec"), env)
                value = eval(compile(last, "<block>", "eval"), env) if last else None
            if value is not None:
                print(value)
        else:
            exec(compile(tree, "<block>", "exec"), env)
    except SystemExit:
        pass
    except BaseException:
        traceback.print_exc()
    print("__babel_done__", flush=True)
"##;

/// The program a session of `language` is, reading its chunks on stdin.
pub fn session_program(language: &str) -> Option<Vec<String>> {
    Some(match language {
        "python" | "python3" => vec![
            "python3".to_string(),
            "-u".to_string(),
            "-c".to_string(),
            PYTHON_DRIVER.to_string(),
        ],
        "sh" | "shell" => vec!["sh".to_string()],
        "bash" => vec!["bash".to_string()],
        "zsh" => vec!["zsh".to_string()],
        _ => return None,
    })
}

/// What a session starts with: a shell's errors into its output.
pub fn session_start(language: &str) -> &'static str {
    if is_shell(language) {
        "exec 2>&1\n"
    } else {
        ""
    }
}

/// What to send a session to run the plan's script: the output that comes
/// back ends with a line [`SESSION_DONE`].
pub fn session_chunk(plan: &Plan) -> String {
    let mut script = plan.script.clone();
    if !script.ends_with('\n') {
        script.push('\n');
    }
    if is_shell(&plan.language) {
        format!("{script}printf '\\n%s\\n' {SESSION_DONE}\n")
    } else {
        let mode = match plan.collect {
            Collect::Value => "value",
            Collect::Output => "output",
        };
        format!("#__babel__ {mode}\n{script}#__babel_end__\n")
    }
}

// ── Results ────────────────────────────────────────────────────────────────

/// Whether the results `block` has were made by this very plan: `:cache
/// yes` and a mark that matches, so there is no need to run it again.
pub fn cached(text: &str, block: &SourceBlock, plan: &Plan) -> bool {
    let Some(hash) = &plan.cache else {
        return false;
    };
    let Some(line) = source::result_of(text, block) else {
        return false;
    };
    let keyword = text.lines().nth(line).unwrap_or("").trim_start();
    keyword
        .get(10..)
        .and_then(|rest| rest.split_once(']'))
        .is_some_and(|(mark, _)| mark == hash)
}

/// Org puts results of this many lines or more in an example block rather
/// than `: ` lines (`org-babel-min-lines-for-block-output`).
const BLOCK_OUTPUT_LINES: usize = 10;

/// The lines a result is written as, `#+RESULTS:` included.
pub fn results_lines(output: &str, plan: &Plan) -> Vec<String> {
    let mark = plan
        .cache
        .as_ref()
        .map(|hash| format!("[{hash}]"))
        .unwrap_or_default();
    let keyword = match &plan.name {
        Some(name) => format!("#+RESULTS{mark}: {name}"),
        None => format!("#+RESULTS{mark}:"),
    };
    if plan.format == Format::Silent {
        return Vec::new();
    }
    // A file's results are a link to it, whatever was printed.
    if let Some((link, _)) = &plan.file {
        return vec![keyword, format!("[[file:{link}]]")];
    }
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

/// The results starting at `start`, as a line range: an example block
/// whole, or the lines down to a blank one or a headline.
fn results_extent(lines: &[&str], start: usize) -> (usize, usize) {
    let mut end = start + 1;
    if lines.get(end).is_some_and(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("#+begin_")
    }) {
        let closing = lines[end]
            .split_whitespace()
            .next()
            .map(|begin| begin.to_ascii_lowercase().replacen("#+begin_", "#+end_", 1))
            .unwrap_or_default();
        while end < lines.len() && !lines[end].trim().eq_ignore_ascii_case(&closing) {
            end += 1;
        }
        return (start, (end + 1).min(lines.len()));
    }
    while end < lines.len()
        && !lines[end].trim().is_empty()
        && crate::restructure::headline_level(lines[end]).is_none()
    {
        end += 1;
    }
    (start, end)
}

/// Replaces the results in `range`, or puts them after line `after`, one
/// blank line down.
fn place_results(
    text: &str,
    after: usize,
    range: Option<(usize, usize)>,
    results: &[String],
) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = lines.iter().map(|line| line.to_string()).collect();

    match range {
        Some((start, end)) => {
            out.splice(start..end, results.iter().cloned());
        }
        None if results.is_empty() => {}
        None => {
            let mut inserted = vec![String::new()];
            inserted.extend(results.iter().cloned());
            // Keep a blank line between the results and what follows.
            if lines
                .get(after + 1)
                .is_some_and(|line| !line.trim().is_empty())
            {
                inserted.push(String::new());
            }
            out.splice(after + 1..after + 1, inserted);
        }
    }

    crate::restructure::rejoin(&out, text)
}

/// `text` with `results` replacing whatever results `block` had, or placed
/// after it, one blank line down, when it had none.
pub fn write_results(text: &str, block: &SourceBlock, results: &[String]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let range = source::result_of(text, block).map(|start| results_extent(&lines, start));
    place_results(text, block.end, range, results)
}

/// `text` with `results` under the `#+CALL:` at `line`, replacing the ones
/// already there.
pub fn write_call_results(text: &str, line: usize, results: &[String]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let range = (line + 1..lines.len())
        .find(|&at| !lines[at].trim().is_empty())
        .filter(|&at| {
            let trimmed = lines[at].trim_start().to_ascii_lowercase();
            trimmed.starts_with("#+results:") || trimmed.starts_with("#+results[")
        })
        .map(|start| results_extent(&lines, start));
    place_results(text, line, range, results)
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
        assert_eq!(plan.program, ["sh", "{src}"]);
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
            plan_of("#+begin_src haskell\nmain = pure ()\n#+end_src\n"),
            Err(BabelError::Unsupported("haskell".to_string()))
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

    #[test]
    fn a_var_takes_a_named_table_or_another_block_s_results() {
        let text = "\
#+NAME: data
| a | 1 |
| b | 2 |

#+NAME: total
#+begin_src sh
echo 3
#+end_src

#+RESULTS: total
: 3

#+begin_src python :var rows=data n=total()
return len(rows) + n
#+end_src
";
        let blocks = source::blocks(text);
        let plan = plan(text, &blocks[1], Path::new("/n/a.org")).unwrap();
        assert!(
            plan.script
                .starts_with("rows = [[\"a\", 1], [\"b\", 2]]\nn = 3\n"),
            "{}",
            plan.script
        );
    }

    #[test]
    fn a_var_is_found_in_the_library_too() {
        let library = vec!["#+NAME: greeting\n: bonjour\n".to_string()];
        let text = "#+begin_src sh :var g=greeting\necho $g\n#+end_src\n";
        let block = source::blocks(text).remove(0);
        let plan = plan_with(text, &block, Path::new("/n/a.org"), &library).unwrap();
        assert_eq!(plan.script, "g='bonjour'\necho $g\n");
        assert!(plan_with(text, &block, Path::new("/n/a.org"), &[]).is_err());
    }

    #[test]
    fn compiled_languages_build_first_and_get_a_main() {
        let c = plan_of(
            "#+begin_src C :var n=3 :flags -O2 :libs -lm\nprintf(\"%d\\n\", n);\n#+end_src\n",
        )
        .unwrap();
        assert_eq!(c.program, ["{bin}"]);
        assert_eq!(
            c.build.as_deref().unwrap(),
            ["cc", "-O2", "-o", "{bin}", "{src}", "-lm"]
        );
        assert_eq!(
            c.script,
            "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\nint n = 3;\nint main() {\n    printf(\"%d\\n\", n);\n    return 0;\n}\n"
        );
        let rust =
            plan_of("#+begin_src rust :var n=2\nprintln!(\"{}\", n * 21);\n#+end_src\n").unwrap();
        assert_eq!(
            rust.script,
            "fn main() {\n    let n: i64 = 2;\n    println!(\"{}\", n * 21);\n}\n"
        );
        let go = plan_of("#+begin_src go\nfmt.Println(42)\n#+end_src\n").unwrap();
        assert_eq!(go.program, ["go", "run", "{src}"]);
        assert_eq!(
            go.script,
            "package main\n\nimport (\n\"fmt\"\n)\n\nfunc main() {\n    fmt.Println(42)\n}\n"
        );
        // A value from a compiled block is what it prints; nothing to say.
        assert!(!value_is_output(&c));
    }

    #[test]
    fn sessions_are_python_and_the_shells() {
        let python = plan_of("#+begin_src python :session\nx = 2\nx * 3\n#+end_src\n").unwrap();
        assert_eq!(python.session.as_deref(), Some("default"));
        assert!(!python.value_file);
        assert_eq!(
            session_chunk(&python),
            "#__babel__ value\nx = 2\nx * 3\n#__babel_end__\n"
        );
        let shell = plan_of("#+begin_src bash :session work\necho hi\n#+end_src\n").unwrap();
        assert_eq!(
            session_chunk(&shell),
            "echo hi\nprintf '\\n%s\\n' __babel_done__\n"
        );
        assert!(plan_of("#+begin_src ruby :session\np 1\n#+end_src\n").is_err());
        assert!(
            plan_of("#+begin_src python :session none\nreturn 1\n#+end_src\n")
                .unwrap()
                .session
                .is_none()
        );
    }

    #[test]
    fn cached_results_carry_a_mark_and_are_recognised() {
        let text = "#+begin_src sh :cache yes\necho hi\n#+end_src\n";
        let block = source::blocks(text).remove(0);
        let plan = plan(text, &block, Path::new("/n/a.org")).unwrap();
        let hash = plan.cache.clone().unwrap();
        let results = results_lines("hi", &plan);
        assert_eq!(results[0], format!("#+RESULTS[{hash}]:"));
        let written = write_results(text, &block, &results);
        let block = source::blocks(&written).remove(0);
        assert!(cached(&written, &block, &plan));
        // A change to the code is a change to the mark.
        let changed = written.replace("echo hi", "echo bye");
        let block = source::blocks(&changed).remove(0);
        let replanned = super::plan(&changed, &block, Path::new("/n/a.org")).unwrap();
        assert!(!cached(&changed, &block, &replanned));
    }

    #[test]
    fn file_results_are_a_link() {
        let plan = plan_of(
            "#+NAME: graph\n#+begin_src dot :file-ext svg :output-dir img\ndigraph { a -> b }\n#+end_src\n",
        )
        .unwrap();
        assert_eq!(plan.program, ["dot", "-Tsvg", "-o", "{file}", "{src}"]);
        let (link, path) = plan.file.clone().unwrap();
        assert_eq!(link, "img/graph.svg");
        assert_eq!(path, Path::new("/notes/img/graph.svg"));
        assert_eq!(
            results_lines("", &plan),
            ["#+RESULTS: graph", "[[file:img/graph.svg]]"]
        );
        assert!(plan_of("#+begin_src dot\ndigraph {}\n#+end_src\n").is_err());

        let attached = "* Entry\n:PROPERTIES:\n:ID: abcdef\n:END:\n#+begin_src python :dir attach :file plot.png\npass\n#+end_src\n";
        let block = source::blocks(attached).remove(0);
        let plan = super::plan(attached, &block, Path::new("/n/a.org")).unwrap();
        assert_eq!(plan.dir, Path::new("/n/data/ab/cdef"));
        assert_eq!(plan.file.unwrap().0, "data/ab/cdef/plot.png");
    }

    #[test]
    fn a_call_runs_a_named_block_with_its_arguments() {
        let text = "\
#+NAME: double
#+begin_src sh :var x=1
echo $((x * 2))
#+end_src

#+CALL: double(x=21) :results output
";
        let call = call_at(text, 5).unwrap();
        assert_eq!(call.name, "double");
        assert_eq!(call.args, [("x".to_string(), "21".to_string())]);
        let plan = plan_call(text, &call, Path::new("/n/a.org"), &[]).unwrap();
        assert_eq!(plan.script, "x=21\necho $((x * 2))\n");
        assert_eq!(plan.collect, Collect::Output);
        let written = write_call_results(text, 5, &results_lines("42", &plan));
        assert!(
            written.ends_with("#+CALL: double(x=21) :results output\n\n#+RESULTS:\n: 42\n"),
            "{written}"
        );
        let again = write_call_results(&written, 5, &results_lines("0", &plan));
        assert!(again.ends_with("\n#+RESULTS:\n: 0\n"), "{again}");
        assert!(plan_call(
            text,
            &call_at("#+CALL: nothing()\n", 0).unwrap(),
            Path::new("/n/a.org"),
            &[]
        )
        .is_err());
    }

    #[test]
    fn a_library_block_called_on_a_table_of_the_calling_file() {
        let library = "\
#+NAME: count
#+begin_src python :var rows=0
return len(rows)
#+end_src
"
        .to_string();
        let text = "\
#+NAME: mine
| a |
| b |

#+CALL: count(rows=mine)
";
        let call = call_at(text, 4).unwrap();
        let plan = plan_call(text, &call, Path::new("/n/a.org"), &[library]).unwrap();
        assert!(
            plan.script.starts_with("rows = [[\"a\"], [\"b\"]]\n"),
            "{}",
            plan.script
        );
    }

    #[test]
    fn commas_split_arguments_but_not_command_line_words() {
        assert_eq!(split_assignments("a=1,b=\"x, y\""), ["a=1", "b=\"x, y\""]);
        assert_eq!(
            split_words("-Wl,-rpath,/opt/lib -O2", false),
            ["-Wl,-rpath,/opt/lib", "-O2"]
        );
    }
}
