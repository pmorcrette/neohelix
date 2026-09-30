//! magit-todos: the `TODO`, `FIXME` and like comments of the repository, for
//! a section of the status buffer.
//!
//! As in magit-todos, a keyword counts only when a colon follows it, with an
//! optional `(who)` between (`TODO:`, `FIXME(ana):`), so the word in prose
//! is not a to-do. The search is `git grep`, over the tracked files and the
//! untracked ones git does not ignore.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The keywords looked for when none are configured.
pub const DEFAULT_KEYWORDS: [&str; 5] = ["TODO", "FIXME", "HACK", "XXX", "BUG"];

/// One keyword comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Todo {
    /// Relative to the working tree.
    pub path: PathBuf,
    /// One-based.
    pub line: usize,
    pub keyword: String,
    /// What follows the keyword, trimmed of comment closers.
    pub text: String,
}

/// Keywords safe to put in a pattern: letters, digits, `_` and `-` only.
fn usable(keywords: &[String]) -> Vec<&str> {
    keywords
        .iter()
        .map(|keyword| keyword.trim())
        .filter(|keyword| {
            !keyword.is_empty()
                && keyword
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        })
        .collect()
}

/// The extended regular expression `git grep -E` is given.
pub fn pattern(keywords: &[String]) -> Option<String> {
    let keywords = usable(keywords);
    (!keywords.is_empty())
        .then(|| format!("(^|[^[:alnum:]_+])({})(\\([^)]*\\))?:", keywords.join("|")))
}

/// Reads `git grep -n --null` output: `path\0line\0content` per line. Only
/// the listed keywords are kept, and they come out in the order the list
/// gives them, then by file and line, as magit-todos groups them.
pub fn parse(output: &str, keywords: &[String]) -> Vec<Todo> {
    let keywords = usable(keywords);
    let mut todos: Vec<(usize, Todo)> = Vec::new();
    for line in output.lines() {
        let mut parts = line.splitn(3, '\0');
        let (Some(path), Some(number), Some(content)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(number) = number.parse::<usize>() else {
            continue;
        };
        let Some((rank, keyword, text)) = find_keyword(content, &keywords) else {
            continue;
        };
        todos.push((
            rank,
            Todo {
                path: PathBuf::from(path),
                line: number,
                keyword: keyword.to_string(),
                text,
            },
        ));
    }
    // Stable, so a keyword's comments stay in git's file and line order.
    todos.sort_by_key(|(rank, _)| *rank);
    todos.into_iter().map(|(_, todo)| todo).collect()
}

/// The first keyword of `content` that a colon follows, its rank in the
/// list, and the text after it.
fn find_keyword<'a>(content: &str, keywords: &[&'a str]) -> Option<(usize, &'a str, String)> {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '+');
    let mut best: Option<(usize, usize, &str, String)> = None;
    for (rank, keyword) in keywords.iter().enumerate() {
        let mut from = 0;
        while let Some(at) = content[from..].find(keyword).map(|at| from + at) {
            from = at + keyword.len();
            if word(content[..at].chars().next_back()) {
                continue;
            }
            let mut rest = &content[at + keyword.len()..];
            if let Some(after) = rest.strip_prefix('(') {
                match after.find(')') {
                    Some(close) => rest = &after[close + 1..],
                    None => continue,
                }
            }
            let Some(text) = rest.strip_prefix(':') else {
                continue;
            };
            if best.as_ref().is_none_or(|(start, ..)| at < *start) {
                best = Some((at, rank, keyword, clean(text)));
            }
            break;
        }
    }
    best.map(|(_, rank, keyword, text)| (rank, keyword, text))
}

/// The comment's text without the closers of block comments.
fn clean(text: &str) -> String {
    let mut text = text.trim();
    for closer in ["*/", "-->", "#}", "%}", "*)"] {
        text = text.strip_suffix(closer).unwrap_or(text).trim_end();
    }
    text.to_string()
}

/// The keyword comments of the repository at `workdir`, at most `limit` of
/// them, and whether there were more.
pub fn scan(workdir: &Path, keywords: &[String], limit: usize) -> (Vec<Todo>, bool) {
    let Some(pattern) = pattern(keywords) else {
        return (Vec::new(), false);
    };
    let output = Command::new("git")
        .args([
            "grep",
            "-n",
            "-I",
            "--null",
            "--no-color",
            "--untracked",
            "--exclude-standard",
            "-E",
            "-e",
            &pattern,
        ])
        .current_dir(workdir)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    // git grep exits with 1 when nothing matches.
    let Ok(output) = output else {
        return (Vec::new(), false);
    };
    let mut todos = parse(&String::from_utf8_lossy(&output.stdout), keywords);
    let more = todos.len() > limit;
    todos.truncate(limit);
    (todos, more)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords() -> Vec<String> {
        DEFAULT_KEYWORDS.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn keywords_need_a_colon_and_are_grouped_in_order() {
        let output = "src/a.rs\x0010\x00    // TODO: tidy this\n\
                      src/a.rs\x0012\x00    /* FIXME(ana): off by one */\n\
                      README.md\x003\x00A TODO list, not a to-do\n\
                      notes.org\x001\x00#+TODO: TODO DONE\n\
                      src/b.rs\x004\x00// NOTTODO: no\n\
                      src/b.rs\x007\x00x(); // XXX:\n\
                      web/i.html\x002\x00<!-- TODO: alt text -->\n";
        let todos = parse(output, &keywords());
        let seen: Vec<(String, usize, &str, &str)> = todos
            .iter()
            .map(|t| {
                (
                    t.path.display().to_string(),
                    t.line,
                    t.keyword.as_str(),
                    t.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                ("src/a.rs".to_string(), 10, "TODO", "tidy this"),
                ("web/i.html".to_string(), 2, "TODO", "alt text"),
                ("src/a.rs".to_string(), 12, "FIXME", "off by one"),
                ("src/b.rs".to_string(), 7, "XXX", ""),
            ]
        );
    }

    #[test]
    fn the_pattern_takes_only_plain_keywords() {
        let odd = vec!["TODO".to_string(), "a|b".to_string(), " ".to_string()];
        assert_eq!(
            pattern(&odd).as_deref(),
            Some("(^|[^[:alnum:]_+])(TODO)(\\([^)]*\\))?:")
        );
        assert_eq!(pattern(&[]), None);
    }

    #[test]
    fn a_repository_is_searched_with_git_grep() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap()
        };
        run(&["init", "-q"]);
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n// TODO: first\n").unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored.rs\n").unwrap();
        std::fs::write(dir.path().join("ignored.rs"), "// TODO: hidden\n").unwrap();
        std::fs::write(dir.path().join("new.rs"), "// FIXME: untracked\n").unwrap();
        run(&["add", "a.rs", ".gitignore"]);

        let (todos, more) = scan(dir.path(), &keywords(), 10);
        let found: Vec<&str> = todos.iter().map(|t| t.text.as_str()).collect();
        assert_eq!(found, ["first", "untracked"]);
        assert!(!more);

        let (todos, more) = scan(dir.path(), &keywords(), 1);
        assert_eq!(todos.len(), 1);
        assert!(more);
    }
}
