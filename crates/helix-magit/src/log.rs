//! The log, and a single commit.
//!
//! Both are read from the `git` binary: `git log --graph` draws the graph,
//! which is not worth reimplementing, and `git show` gives a commit's diff in
//! the same unified format the status buffer already parses.

use std::path::{Path, PathBuf};

use crate::command::GitCommand;
use crate::diff::{parse_unified_diff, FileDiff};

/// How many commits a log shows before asking for more.
pub const DEFAULT_LIMIT: usize = 256;

/// What a log shows: which commits, and which of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFilter {
    /// A revision or range (`main`, `v1.0..HEAD`); `None` is HEAD.
    pub range: Option<String>,
    /// Every branch, tag and remote branch, as `git log --all`.
    pub all: bool,
    /// Commits whose author matches, case-insensitively.
    pub author: Option<String>,
    /// Commits whose message matches, case-insensitively.
    pub grep: Option<String>,
    /// Commits that touch this path.
    pub path: Option<PathBuf>,
    /// Follow the path through renames; git allows it for one file only.
    pub follow: bool,
    pub limit: usize,
}

impl Default for LogFilter {
    fn default() -> Self {
        Self {
            range: None,
            all: false,
            author: None,
            grep: None,
            path: None,
            follow: false,
            limit: DEFAULT_LIMIT,
        }
    }
}

/// The pseudo-flag the log menu uses for a path. git takes paths after
/// `--`, not as a flag, so [`LogFilter::from_args`] moves it there.
pub const PATH_FLAG: &str = "--path=";

impl LogFilter {
    /// A filter from the log menu's arguments.
    pub fn from_args(args: &[String]) -> Self {
        let mut filter = Self::default();
        for arg in args {
            if let Some(author) = arg.strip_prefix("--author=") {
                filter.author = Some(author.to_string());
            } else if let Some(grep) = arg.strip_prefix("--grep=") {
                filter.grep = Some(grep.to_string());
            } else if let Some(path) = arg.strip_prefix(PATH_FLAG) {
                filter.path = Some(PathBuf::from(path));
            } else if arg == "--all" {
                filter.all = true;
            }
        }
        filter
    }

    /// Checks what a user typed as a range: it goes on git's command line,
    /// so it must not be taken for an option.
    pub fn valid_range(range: &str) -> Result<(), String> {
        if range.starts_with('-') {
            Err(format!("`{range}` is not a revision"))
        } else if range.trim().is_empty() {
            Err("empty revision".to_string())
        } else {
            Ok(())
        }
    }

    /// The `git log` arguments.
    pub fn args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "log",
            "--graph",
            "--color=never",
            "--decorate=short",
            "--date=short",
            LOG_FORMAT,
        ]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
        args.push(format!("--max-count={}", self.limit));
        if self.author.is_some() || self.grep.is_some() {
            args.push("--regexp-ignore-case".into());
        }
        if let Some(author) = &self.author {
            args.push(format!("--author={author}"));
        }
        if let Some(grep) = &self.grep {
            args.push(format!("--grep={grep}"));
        }
        if self.all {
            args.push("--all".into());
        }
        if self.follow && self.path.is_some() {
            args.push("--follow".into());
        }
        if let Some(range) = &self.range {
            args.push(range.clone());
        }
        args.push("--".into());
        if let Some(path) = &self.path {
            args.push(path.display().to_string());
        }
        args
    }

    /// A one-line summary for the log's title.
    pub fn describe(&self) -> String {
        let mut parts = vec![if self.all {
            "all references".to_string()
        } else {
            self.range.clone().unwrap_or_else(|| "HEAD".to_string())
        }];
        if let Some(author) = &self.author {
            parts.push(format!("author ~ {author}"));
        }
        if let Some(grep) = &self.grep {
            parts.push(format!("message ~ {grep}"));
        }
        if let Some(path) = &self.path {
            parts.push(format!("touching {}", path.display()));
        }
        parts.join(", ")
    }
}

/// Hash, refs, date, author and subject, NUL-separated after the graph.
const LOG_FORMAT: &str = "--format=%x00%h%x00%D%x00%ad%x00%an%x00%s";

/// One line of the log: a commit, or a line of graph joining commits.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogEntry {
    /// The graph's columns on this line (`* | `, `|\`).
    pub graph: String,
    /// `None` on a graph-only line.
    pub hash: Option<String>,
    /// `HEAD -> main`, `origin/main`, `tag: v1.0`.
    pub refs: Vec<String>,
    pub date: String,
    pub author: String,
    pub subject: String,
}

/// Reads `git log --graph` with [`LOG_FORMAT`].
pub fn parse_log(text: &str) -> Vec<LogEntry> {
    text.lines()
        .map(|line| {
            let Some((graph, rest)) = line.split_once('\0') else {
                return LogEntry {
                    graph: line.trim_end().to_string(),
                    ..LogEntry::default()
                };
            };
            let mut fields = rest.splitn(5, '\0');
            let mut next = || fields.next().unwrap_or_default().to_string();
            let hash = next();
            let refs = next();
            LogEntry {
                graph: graph.trim_end().to_string(),
                hash: Some(hash),
                refs: refs
                    .split(", ")
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .collect(),
                date: next(),
                author: next(),
                subject: next(),
            }
        })
        .collect()
}

/// Runs the log. An error is git's own message.
pub fn read_log(workdir: &Path, filter: &LogFilter) -> Result<Vec<LogEntry>, String> {
    let output = GitCommand::new(workdir, filter.args())
        .run()
        .map_err(|err| err.to_string())?;
    if output.success {
        Ok(parse_log(&output.stdout))
    } else {
        Err(output.summary())
    }
}

/// A commit as the commit view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitDetails {
    pub hash: String,
    pub short: String,
    /// `Name <email>`.
    pub author: String,
    pub date: String,
    pub refs: Vec<String>,
    /// The whole message, subject first.
    pub message: String,
    /// Against the first parent, so a merge shows what it brought in.
    pub files: Vec<FileDiff>,
}

const SHOW_FORMAT: &str = "--format=%H%x00%h%x00%an <%ae>%x00%ad%x00%D%x00%B%x00";

/// Reads `git show` with [`SHOW_FORMAT`].
pub fn parse_show(text: &str) -> Option<CommitDetails> {
    let mut fields = text.splitn(7, '\0');
    let mut next = || fields.next().map(str::to_string);
    let hash = next()?;
    let short = next()?;
    let author = next()?;
    let date = next()?;
    let refs = next()?;
    let message = next()?;
    let diff = next().unwrap_or_default();
    Some(CommitDetails {
        hash: hash.trim().to_string(),
        short,
        author,
        date,
        refs: refs
            .split(", ")
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect(),
        message: message.trim_end().to_string(),
        files: parse_unified_diff(&diff),
    })
}

/// Reads one commit — or a stash, which is a commit too.
pub fn show(workdir: &Path, rev: &str) -> Result<CommitDetails, String> {
    LogFilter::valid_range(rev)?;
    let args = [
        "show",
        "--color=never",
        "--no-ext-diff",
        "--diff-merges=first-parent",
        "--date=format:%Y-%m-%d %H:%M",
        "--patch",
        SHOW_FORMAT,
        rev,
        "--",
    ];
    let output = GitCommand::new(workdir, args.iter().map(|arg| arg.to_string()).collect())
        .run()
        .map_err(|err| err.to_string())?;
    if !output.success {
        return Err(output.summary());
    }
    parse_show(&output.stdout).ok_or_else(|| format!("could not read {rev}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_lines_and_commit_lines_are_told_apart() {
        let text = "* \0a1b2c3d\0HEAD -> main, tag: v1\x002026-09-24\0Ann\0Merge topic\n\
                    |\\  \n\
                    | * \0e4f5a6b\0\x002026-09-23\0Bob\0Topic work\n";
        let entries = parse_log(text);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].graph, "*");
        assert_eq!(entries[0].hash.as_deref(), Some("a1b2c3d"));
        assert_eq!(entries[0].refs, ["HEAD -> main", "tag: v1"]);
        assert_eq!(entries[0].subject, "Merge topic");
        assert_eq!(entries[1].hash, None);
        assert_eq!(entries[1].graph, "|\\");
        assert_eq!(entries[2].graph, "| *");
        assert!(entries[2].refs.is_empty());
        assert_eq!(entries[2].author, "Bob");
    }

    #[test]
    fn a_filter_builds_its_command_line() {
        let filter = LogFilter {
            range: Some("v1..HEAD".into()),
            author: Some("ann".into()),
            grep: Some("fix".into()),
            path: Some(PathBuf::from("src/lib.rs")),
            limit: 10,
            ..LogFilter::default()
        };
        let args = filter.args();
        let tail: Vec<&str> = args[6..].iter().map(String::as_str).collect();
        assert_eq!(
            tail,
            [
                "--max-count=10",
                "--regexp-ignore-case",
                "--author=ann",
                "--grep=fix",
                "v1..HEAD",
                "--",
                "src/lib.rs"
            ]
        );
        assert_eq!(
            filter.describe(),
            "v1..HEAD, author ~ ann, message ~ fix, touching src/lib.rs"
        );
    }

    #[test]
    fn menu_arguments_become_a_filter() {
        let filter = LogFilter::from_args(&[
            "--author=ann".into(),
            "--path=a b.txt".into(),
            "--all".into(),
        ]);
        assert_eq!(filter.author.as_deref(), Some("ann"));
        assert_eq!(filter.path, Some(PathBuf::from("a b.txt")));
        assert!(filter.all);
        assert_eq!(filter.grep, None);
    }

    #[test]
    fn a_range_is_never_an_option() {
        assert!(LogFilter::valid_range("--output=/tmp/x").is_err());
        assert!(LogFilter::valid_range("main..topic").is_ok());
    }
}
