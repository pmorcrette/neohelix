//! A structural model of a unified diff.
//!
//! Magit navigates a diff as a tree — file, then hunk, then line — and stages
//! any node of it. That needs more than diff text: every line has to know
//! which side it came from and where it sits in both the old and the new file,
//! so a selection can be turned back into a patch.

use std::fmt::{self, Write as _};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Which side of the diff a line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiffLineKind {
    /// Unchanged, present in both files.
    Context,
    /// Added by the change; exists only in the new file.
    Addition,
    /// Removed by the change; exists only in the old file.
    Deletion,
}

impl DiffLineKind {
    /// The character a unified diff prefixes this kind of line with.
    pub fn prefix(self) -> char {
        match self {
            DiffLineKind::Context => ' ',
            DiffLineKind::Addition => '+',
            DiffLineKind::Deletion => '-',
        }
    }

    fn from_prefix(prefix: u8) -> Option<Self> {
        match prefix {
            b' ' => Some(DiffLineKind::Context),
            b'+' => Some(DiffLineKind::Addition),
            b'-' => Some(DiffLineKind::Deletion),
            _ => None,
        }
    }
}

/// One line of a hunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    /// The line's text, without the `+`/`-`/space prefix and without its
    /// line ending.
    pub content: String,
    /// 1-based line number in the old file; `None` for an addition.
    pub old_line: Option<u32>,
    /// 1-based line number in the new file; `None` for a deletion.
    pub new_line: Option<u32>,
    /// Whether the file it came from lacks a trailing newline on this line.
    pub no_newline: bool,
}

impl DiffLine {
    /// Renders the line as it appears in a patch, prefix included.
    pub fn to_patch_line(&self) -> String {
        format!("{}{}", self.kind.prefix(), self.content)
    }
}

/// The `@@ -old_start,old_count +new_start,new_count @@ section` line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HunkHeader {
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The optional trailing context git prints after the ranges, such as the
    /// enclosing function.
    pub section: String,
}

impl fmt::Display for HunkHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "@@ -{} +{} @@",
            format_range(self.old_start, self.old_count),
            format_range(self.new_start, self.new_count),
        )?;
        if !self.section.is_empty() {
            write!(f, " {}", self.section)?;
        }
        Ok(())
    }
}

/// git omits the count when it is 1.
fn format_range(start: u32, count: u32) -> String {
    if count == 1 {
        start.to_string()
    } else {
        format!("{start},{count}")
    }
}

/// A contiguous run of changes within a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: HunkHeader,
    pub lines: Vec<DiffLine>,
    /// Whether the UI currently hides this hunk's lines.
    pub folded: bool,
}

impl DiffHunk {
    /// Lines that differ between the two sides.
    pub fn changed_lines(&self) -> impl Iterator<Item = &DiffLine> {
        self.lines
            .iter()
            .filter(|line| line.kind != DiffLineKind::Context)
    }

    /// How many lines this hunk adds and removes.
    pub fn stats(&self) -> (usize, usize) {
        self.lines
            .iter()
            .fold((0, 0), |(added, removed), line| match line.kind {
                DiffLineKind::Addition => (added + 1, removed),
                DiffLineKind::Deletion => (added, removed + 1),
                DiffLineKind::Context => (added, removed),
            })
    }
}

/// What happened to a file between the two trees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed {
        from: PathBuf,
    },
    Copied {
        from: PathBuf,
    },
    /// The file's mode changed, e.g. a symlink became a regular file.
    TypeChanged,
}

impl FileStatus {
    /// A single letter, as `git status --short` would print it.
    pub fn code(&self) -> char {
        match self {
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Modified => 'M',
            FileStatus::Renamed { .. } => 'R',
            FileStatus::Copied { .. } => 'C',
            FileStatus::TypeChanged => 'T',
        }
    }
}

/// Every change to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    /// The file's path on the new side; for a deletion, the path it had.
    pub path: PathBuf,
    pub status: FileStatus,
    pub hunks: Vec<DiffHunk>,
    /// Whether the UI currently hides this file's hunks.
    pub folded: bool,
    /// Binary files have no hunks to show.
    pub binary: bool,
}

impl FileDiff {
    /// The path the change started from, which differs only for a rename or
    /// a copy.
    pub fn source_path(&self) -> &Path {
        match &self.status {
            FileStatus::Renamed { from } | FileStatus::Copied { from } => from,
            _ => &self.path,
        }
    }

    /// Total lines added and removed across every hunk.
    pub fn stats(&self) -> (usize, usize) {
        self.hunks.iter().fold((0, 0), |(added, removed), hunk| {
            let (a, r) = hunk.stats();
            (added + a, removed + r)
        })
    }
}

/// How much whitespace matters when lines are compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Whitespace {
    #[default]
    Exact,
    /// `--ignore-space-change`: runs of whitespace are one space, and
    /// whitespace at the end of a line does not count.
    IgnoreChange,
    /// `--ignore-all-space`.
    IgnoreAll,
}

/// Which diff algorithm lines up the two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiffAlgorithm {
    #[default]
    Histogram,
    Myers,
    Minimal,
    /// Only git has it: diffs computed in-process use histogram instead,
    /// which is patience's refinement.
    Patience,
}

impl DiffAlgorithm {
    pub fn name(self) -> &'static str {
        match self {
            DiffAlgorithm::Histogram => "histogram",
            DiffAlgorithm::Myers => "myers",
            DiffAlgorithm::Minimal => "minimal",
            DiffAlgorithm::Patience => "patience",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.trim() {
            "histogram" => Some(DiffAlgorithm::Histogram),
            "myers" | "default" => Some(DiffAlgorithm::Myers),
            "minimal" => Some(DiffAlgorithm::Minimal),
            "patience" => Some(DiffAlgorithm::Patience),
            _ => None,
        }
    }
}

/// How a diff is computed and shown: Magit's diff arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffOptions {
    /// Lines of context around each change.
    pub context: u32,
    pub whitespace: Whitespace,
    pub algorithm: DiffAlgorithm,
    /// Mark the words that changed inside changed lines.
    pub word_diff: bool,
    /// Show each file's size of change instead of its hunks.
    pub stat: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            context: 3,
            whitespace: Whitespace::Exact,
            algorithm: DiffAlgorithm::Histogram,
            word_diff: false,
            stat: false,
        }
    }
}

impl DiffOptions {
    /// The arguments that make `git diff` / `git show` compute the same
    /// diff; word marking and the summary are the view's own.
    pub fn git_args(&self) -> Vec<String> {
        let mut args = vec![
            format!("--unified={}", self.context),
            format!("--diff-algorithm={}", self.algorithm.name()),
        ];
        match self.whitespace {
            Whitespace::Exact => {}
            Whitespace::IgnoreChange => args.push("--ignore-space-change".into()),
            Whitespace::IgnoreAll => args.push("--ignore-all-space".into()),
        }
        args
    }

    /// Options from the diff menu's arguments; anything not given keeps its
    /// default.
    pub fn from_args(args: &[String]) -> Self {
        let mut options = Self::default();
        for arg in args {
            if let Some(context) = arg.strip_prefix("--unified=") {
                if let Ok(context) = context.trim().parse() {
                    options.context = context;
                }
            } else if let Some(name) = arg.strip_prefix("--diff-algorithm=") {
                if let Some(algorithm) = DiffAlgorithm::parse(name) {
                    options.algorithm = algorithm;
                }
            } else if arg == "--ignore-all-space" {
                options.whitespace = Whitespace::IgnoreAll;
            } else if arg == "--ignore-space-change" && options.whitespace == Whitespace::Exact {
                options.whitespace = Whitespace::IgnoreChange;
            } else if arg == "--word-diff" {
                options.word_diff = true;
            } else if arg == "--stat" {
                options.stat = true;
            }
        }
        options
    }

    /// What differs from the defaults, for a view's title.
    pub fn describe(&self) -> String {
        let default = Self::default();
        let mut parts = Vec::new();
        if self.context != default.context {
            parts.push(format!("-U{}", self.context));
        }
        match self.whitespace {
            Whitespace::Exact => {}
            Whitespace::IgnoreChange => parts.push("-b".to_string()),
            Whitespace::IgnoreAll => parts.push("-w".to_string()),
        }
        if self.algorithm != default.algorithm {
            parts.push(self.algorithm.name().to_string());
        }
        if self.word_diff {
            parts.push("words".to_string());
        }
        if self.stat {
            parts.push("stat".to_string());
        }
        parts.join(" ")
    }

    /// The key a line is compared by, as the whitespace setting sees it.
    pub fn comparable(&self, line: &str) -> String {
        match self.whitespace {
            Whitespace::Exact => line.to_string(),
            Whitespace::IgnoreChange => line.split_whitespace().collect::<Vec<_>>().join(" "),
            Whitespace::IgnoreAll => line.chars().filter(|c| !c.is_whitespace()).collect(),
        }
    }
}

/// The byte ranges of the words that differ between a deleted line and the
/// added line that replaced it: `(in old, in new)`.
pub fn word_changes(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    use imara_diff::{Algorithm, Diff, InternedInput};

    let old_words = words(old);
    let new_words = words(new);
    let mut input: InternedInput<&str> = InternedInput::default();
    input.update_before(old_words.iter().map(|range| &old[range.clone()]));
    input.update_after(new_words.iter().map(|range| &new[range.clone()]));
    let diff = Diff::compute(Algorithm::Myers, &input);

    let span = |ranges: &[Range<usize>], tokens: Range<u32>| -> Option<Range<usize>> {
        if tokens.is_empty() {
            return None;
        }
        Some(ranges[tokens.start as usize].start..ranges[tokens.end as usize - 1].end)
    };
    let mut in_old = Vec::new();
    let mut in_new = Vec::new();
    for hunk in diff.hunks() {
        in_old.extend(span(&old_words, hunk.before));
        in_new.extend(span(&new_words, hunk.after));
    }
    (
        merge_across_spaces(old, in_old),
        merge_across_spaces(new, in_new),
    )
}

/// Joins changed ranges that only whitespace separates, so `+ 1` reads as
/// one change rather than two.
fn merge_across_spaces(text: &str, ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        match out.last_mut() {
            Some(last) if text[last.end..range.start].trim().is_empty() => last.end = range.end,
            _ => out.push(range),
        }
    }
    out
}

/// A line cut into words, runs of spaces and single other characters.
fn words(line: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let same = |next: char| {
            (c.is_alphanumeric() || c == '_') && (next.is_alphanumeric() || next == '_')
                || c.is_whitespace() && next.is_whitespace()
        };
        let mut end = start + c.len_utf8();
        while let Some(&(at, next)) = chars.peek() {
            if !same(next) {
                break;
            }
            end = at + next.len_utf8();
            chars.next();
        }
        out.push(start..end);
    }
    out
}

/// For each changed line of a hunk that has a counterpart, the ranges of
/// the words that changed. A run of deletions followed by a run of
/// additions is paired line by line, as far as both go; the rest have no
/// counterpart and are changed whole.
pub fn refine(hunk: &DiffHunk) -> std::collections::HashMap<usize, Vec<Range<usize>>> {
    let mut out = std::collections::HashMap::new();
    let lines = &hunk.lines;
    let mut at = 0;
    while at < lines.len() {
        if lines[at].kind != DiffLineKind::Deletion {
            at += 1;
            continue;
        }
        let deletions = at;
        while at < lines.len() && lines[at].kind == DiffLineKind::Deletion {
            at += 1;
        }
        let additions = at;
        while at < lines.len() && lines[at].kind == DiffLineKind::Addition {
            at += 1;
        }
        let pairs = (additions - deletions).min(at - additions);
        for offset in 0..pairs {
            let (old, new) = (deletions + offset, additions + offset);
            let (in_old, in_new) = word_changes(&lines[old].content, &lines[new].content);
            out.insert(old, in_old);
            out.insert(new, in_new);
        }
    }
    out
}

/// Parses `git diff` output into one [`FileDiff`] per file.
///
/// Unrecognised lines between file headers are skipped rather than rejected:
/// git prints mode changes, index lines and similarity scores that the model
/// does not need, and new ones appearing must not break the view.
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if !line.starts_with("diff --git ") {
            continue;
        }

        let mut file = FileDiff {
            path: parse_diff_git_paths(line)
                .map(|(_, b)| b)
                .unwrap_or_default(),
            status: FileStatus::Modified,
            hunks: Vec::new(),
            folded: false,
            binary: false,
        };
        let mut rename_from: Option<PathBuf> = None;
        let mut copy_from: Option<PathBuf> = None;

        // Header lines, up to the first hunk or the next file.
        while let Some(&next) = lines.peek() {
            if next.starts_with("diff --git ") || next.starts_with("@@") {
                break;
            }
            let next = lines.next().expect("peeked");

            if next.starts_with("new file mode") {
                file.status = FileStatus::Added;
            } else if next.starts_with("deleted file mode") {
                file.status = FileStatus::Deleted;
            } else if let Some(from) = next.strip_prefix("rename from ") {
                rename_from = Some(unquote_path(from));
            } else if let Some(to) = next.strip_prefix("rename to ") {
                file.path = unquote_path(to);
            } else if let Some(from) = next.strip_prefix("copy from ") {
                copy_from = Some(unquote_path(from));
            } else if let Some(to) = next.strip_prefix("copy to ") {
                file.path = unquote_path(to);
            } else if next.starts_with("Binary files ") || next.starts_with("GIT binary patch") {
                file.binary = true;
            } else if let Some(path) = next.strip_prefix("+++ ") {
                if path != "/dev/null" {
                    file.path = strip_diff_prefix(path);
                }
            } else if let Some(path) = next.strip_prefix("--- ") {
                // A `+++ /dev/null` marks a deletion, so keep the old path.
                if path != "/dev/null" && file.path.as_os_str().is_empty() {
                    file.path = strip_diff_prefix(path);
                }
            } else if next.starts_with("old mode ") {
                file.status = FileStatus::TypeChanged;
            }
        }

        if let Some(from) = rename_from {
            file.status = FileStatus::Renamed { from };
        } else if let Some(from) = copy_from {
            file.status = FileStatus::Copied { from };
        }

        // Hunks.
        while let Some(&next) = lines.peek() {
            if next.starts_with("diff --git ") {
                break;
            }
            let next = lines.next().expect("peeked");

            let Some(header) = parse_hunk_header(next) else {
                continue;
            };

            let mut hunk = DiffHunk {
                lines: Vec::new(),
                folded: false,
                header,
            };
            let mut old_line = hunk.header.old_start;
            let mut new_line = hunk.header.new_start;

            while let Some(&body) = lines.peek() {
                if body.starts_with("diff --git ") || body.starts_with("@@") {
                    break;
                }
                let body = lines.next().expect("peeked");

                if body.starts_with('\\') {
                    // "\ No newline at end of file" describes the line above.
                    if let Some(last) = hunk.lines.last_mut() {
                        last.no_newline = true;
                    }
                    continue;
                }

                // An empty line in the body is a context line whose content is
                // empty and whose trailing space git trimmed.
                let (kind, content) = match body.as_bytes().first().copied() {
                    None => (DiffLineKind::Context, ""),
                    Some(prefix) => match DiffLineKind::from_prefix(prefix) {
                        Some(kind) => (kind, &body[1..]),
                        // Not part of the hunk body; ignore it.
                        None => continue,
                    },
                };

                let (old, new) = match kind {
                    DiffLineKind::Context => {
                        let pair = (Some(old_line), Some(new_line));
                        old_line += 1;
                        new_line += 1;
                        pair
                    }
                    DiffLineKind::Addition => {
                        let pair = (None, Some(new_line));
                        new_line += 1;
                        pair
                    }
                    DiffLineKind::Deletion => {
                        let pair = (Some(old_line), None);
                        old_line += 1;
                        pair
                    }
                };

                hunk.lines.push(DiffLine {
                    kind,
                    content: content.to_string(),
                    old_line: old,
                    new_line: new,
                    no_newline: false,
                });
            }

            file.hunks.push(hunk);
        }

        files.push(file);
    }

    files
}

/// Splits `diff --git a/old b/new` into its two paths.
///
/// Paths containing spaces are ambiguous in this line, which is why git also
/// prints `---`/`+++`; those override what is parsed here.
fn parse_diff_git_paths(line: &str) -> Option<(PathBuf, PathBuf)> {
    let rest = line.strip_prefix("diff --git ")?;
    let middle = rest.find(" b/")?;
    Some((
        strip_diff_prefix(&rest[..middle]),
        strip_diff_prefix(&rest[middle + 1..]),
    ))
}

/// Removes the `a/` or `b/` prefix git puts on diff paths.
fn strip_diff_prefix(path: &str) -> PathBuf {
    let path = path.trim();
    // `+++ b/file\t` — git appends a tab before any timestamp.
    let path = path.split('\t').next().unwrap_or(path);
    let path = path
        .strip_prefix("a/")
        .or_else(|| path.strip_prefix("b/"))
        .unwrap_or(path);
    unquote_path(path)
}

/// git quotes paths with unusual characters; take the quoted form literally
/// rather than attempting to decode its escapes.
fn unquote_path(path: &str) -> PathBuf {
    let path = path.trim();
    PathBuf::from(
        path.strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(path),
    )
}

/// Parses `@@ -1,3 +1,4 @@ optional section`.
fn parse_hunk_header(line: &str) -> Option<HunkHeader> {
    let rest = line.strip_prefix("@@ ")?;
    let end = rest.find(" @@")?;
    let (ranges, section) = rest.split_at(end);
    let section = section.trim_start_matches(" @@").trim();

    let (old, new) = ranges.split_once(' ')?;
    let (old_start, old_count) = parse_range(old.strip_prefix('-')?)?;
    let (new_start, new_count) = parse_range(new.strip_prefix('+')?)?;

    Some(HunkHeader {
        old_start,
        old_count,
        new_start,
        new_count,
        section: section.to_string(),
    })
}

/// Parses `start,count`, where an omitted count means 1.
fn parse_range(range: &str) -> Option<(u32, u32)> {
    match range.split_once(',') {
        Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
        None => Some((range.parse().ok()?, 1)),
    }
}

/// Renders a file's diff back to patch text, as `git apply` accepts it.
///
/// Round-tripping matters for Task 2.3: staging a selection means emitting a
/// patch built from part of this model.
pub fn render_patch(file: &FileDiff) -> String {
    let mut out = String::new();
    let source = file.source_path().display();
    let target = file.path.display();

    let _ = writeln!(out, "diff --git a/{source} b/{target}");
    match &file.status {
        FileStatus::Added => {
            let _ = writeln!(out, "new file mode 100644");
        }
        FileStatus::Deleted => {
            let _ = writeln!(out, "deleted file mode 100644");
        }
        FileStatus::Renamed { from } => {
            let _ = writeln!(out, "rename from {}", from.display());
            let _ = writeln!(out, "rename to {target}");
        }
        FileStatus::Copied { from } => {
            let _ = writeln!(out, "copy from {}", from.display());
            let _ = writeln!(out, "copy to {target}");
        }
        FileStatus::Modified | FileStatus::TypeChanged => {}
    }

    let _ = writeln!(out, "--- {}", old_side(file));
    let _ = writeln!(out, "+++ {}", new_side(file));

    for hunk in &file.hunks {
        let _ = writeln!(out, "{}", hunk.header);
        for line in &hunk.lines {
            let _ = writeln!(out, "{}", line.to_patch_line());
            if line.no_newline {
                let _ = writeln!(out, "\\ No newline at end of file");
            }
        }
    }

    out
}

fn old_side(file: &FileDiff) -> String {
    match file.status {
        FileStatus::Added => "/dev/null".to_string(),
        _ => format!("a/{}", file.source_path().display()),
    }
}

fn new_side(file: &FileDiff) -> String {
    match file.status {
        FileStatus::Deleted => "/dev/null".to_string(),
        _ => format!("b/{}", file.path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/main.rs b/src/main.rs
index 83db48f..bf269f4 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,5 +1,6 @@ fn main()
 use std::io;
-let old = 1;
+let new = 1;
+let extra = 2;
 
 fn main() {
diff --git a/README.md b/README.md
new file mode 100644
--- /dev/null
+++ b/README.md
@@ -0,0 +1 @@
+# Title
";

    #[test]
    fn parses_every_file_in_a_diff() {
        let files = parse_unified_diff(SAMPLE);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, Path::new("src/main.rs"));
        assert_eq!(files[0].status, FileStatus::Modified);
        assert_eq!(files[1].path, Path::new("README.md"));
        assert_eq!(files[1].status, FileStatus::Added);
    }

    #[test]
    fn parses_the_hunk_header_including_its_section() {
        let files = parse_unified_diff(SAMPLE);
        let header = &files[0].hunks[0].header;

        assert_eq!(header.old_start, 1);
        assert_eq!(header.old_count, 5);
        assert_eq!(header.new_start, 1);
        assert_eq!(header.new_count, 6);
        assert_eq!(header.section, "fn main()");
    }

    #[test]
    fn line_numbers_advance_per_side() {
        let files = parse_unified_diff(SAMPLE);
        let lines = &files[0].hunks[0].lines;

        // " use std::io;" — context, present on both sides.
        assert_eq!(lines[0].kind, DiffLineKind::Context);
        assert_eq!(lines[0].content, "use std::io;");
        assert_eq!((lines[0].old_line, lines[0].new_line), (Some(1), Some(1)));

        // "-let old = 1;" — only in the old file.
        assert_eq!(lines[1].kind, DiffLineKind::Deletion);
        assert_eq!(lines[1].content, "let old = 1;");
        assert_eq!((lines[1].old_line, lines[1].new_line), (Some(2), None));

        // Two additions, which do not consume old line numbers.
        assert_eq!(lines[2].kind, DiffLineKind::Addition);
        assert_eq!((lines[2].old_line, lines[2].new_line), (None, Some(2)));
        assert_eq!(lines[3].kind, DiffLineKind::Addition);
        assert_eq!((lines[3].old_line, lines[3].new_line), (None, Some(3)));

        // The context line after them resumes from old line 3.
        assert_eq!(lines[4].kind, DiffLineKind::Context);
        assert_eq!((lines[4].old_line, lines[4].new_line), (Some(3), Some(4)));
    }

    #[test]
    fn an_empty_context_line_is_not_dropped() {
        // git writes a bare newline for a blank context line, trimming the
        // leading space, so the body line is empty rather than " ".
        let files = parse_unified_diff(SAMPLE);
        let blank = &files[0].hunks[0].lines[4];
        assert_eq!(blank.kind, DiffLineKind::Context);
        assert_eq!(blank.content, "");
    }

    #[test]
    fn counts_additions_and_deletions() {
        let files = parse_unified_diff(SAMPLE);
        assert_eq!(files[0].stats(), (2, 1));
        assert_eq!(files[0].hunks[0].changed_lines().count(), 3);
        assert_eq!(files[1].stats(), (1, 0));
    }

    #[test]
    fn parses_a_deletion() {
        let text = "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-first
-second
";
        let files = parse_unified_diff(text);
        assert_eq!(files[0].status, FileStatus::Deleted);
        assert_eq!(files[0].path, Path::new("gone.txt"));
        assert_eq!(files[0].stats(), (0, 2));
    }

    #[test]
    fn parses_a_rename() {
        let text = "\
diff --git a/old/name.rs b/new/name.rs
similarity index 92%
rename from old/name.rs
rename to new/name.rs
--- a/old/name.rs
+++ b/new/name.rs
@@ -1 +1 @@
-old
+new
";
        let files = parse_unified_diff(text);
        assert_eq!(
            files[0].status,
            FileStatus::Renamed {
                from: PathBuf::from("old/name.rs")
            }
        );
        assert_eq!(files[0].path, Path::new("new/name.rs"));
        assert_eq!(files[0].source_path(), Path::new("old/name.rs"));
    }

    #[test]
    fn a_binary_file_has_no_hunks() {
        let text = "\
diff --git a/logo.png b/logo.png
index 1234567..89abcde 100644
Binary files a/logo.png and b/logo.png differ
";
        let files = parse_unified_diff(text);
        assert!(files[0].binary);
        assert!(files[0].hunks.is_empty());
    }

    #[test]
    fn an_omitted_count_means_one_line() {
        let header = parse_hunk_header("@@ -7 +7 @@").unwrap();
        assert_eq!((header.old_start, header.old_count), (7, 1));
        assert_eq!((header.new_start, header.new_count), (7, 1));
        // And it is written back the way git writes it.
        assert_eq!(header.to_string(), "@@ -7 +7 @@");
    }

    #[test]
    fn a_missing_trailing_newline_marks_the_line_above() {
        let text = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1 +1 @@
-old
\\ No newline at end of file
+new
";
        let files = parse_unified_diff(text);
        let lines = &files[0].hunks[0].lines;
        assert!(lines[0].no_newline);
        assert!(!lines[1].no_newline);
    }

    #[test]
    fn multiple_hunks_in_one_file() {
        let text = "\
diff --git a/a.txt b/a.txt
--- a/a.txt
+++ b/a.txt
@@ -1,2 +1,2 @@
 one
-two
+TWO
@@ -10,2 +10,2 @@
 ten
-eleven
+ELEVEN
";
        let files = parse_unified_diff(text);
        assert_eq!(files[0].hunks.len(), 2);
        assert_eq!(files[0].hunks[1].header.old_start, 10);
        assert_eq!(files[0].hunks[1].lines[1].old_line, Some(11));
        assert_eq!(files[0].stats(), (2, 2));
    }

    #[test]
    fn a_patch_round_trips_through_the_model() {
        // Re-parsing a rendered patch must give back the same model, which is
        // what line-level staging will depend on.
        let files = parse_unified_diff(SAMPLE);
        for file in &files {
            let rendered = render_patch(file);
            let reparsed = parse_unified_diff(&rendered);
            assert_eq!(reparsed.len(), 1, "one file per patch");
            assert_eq!(&reparsed[0], file, "round trip changed the model");
        }
    }

    #[test]
    fn unparseable_input_yields_no_files() {
        assert!(parse_unified_diff("").is_empty());
        assert!(parse_unified_diff("not a diff at all\njust text\n").is_empty());
    }

    #[test]
    fn options_come_from_the_menu_and_go_to_git() {
        let options = DiffOptions::from_args(&[
            "--unified=7".into(),
            "--ignore-space-change".into(),
            "--diff-algorithm=patience".into(),
            "--word-diff".into(),
        ]);
        assert_eq!(options.context, 7);
        assert_eq!(options.whitespace, Whitespace::IgnoreChange);
        assert_eq!(options.algorithm, DiffAlgorithm::Patience);
        assert!(options.word_diff);
        assert_eq!(
            options.git_args(),
            [
                "--unified=7",
                "--diff-algorithm=patience",
                "--ignore-space-change"
            ]
        );
        assert_eq!(options.describe(), "-U7 -b patience words");
        assert_eq!(DiffOptions::default().describe(), "");
    }

    #[test]
    fn changed_words_are_found_within_a_pair_of_lines() {
        let (old, new) = word_changes("let total = count + 1;", "let total = count * 2;");
        let old_text = "let total = count + 1;";
        let new_text = "let total = count * 2;";
        let pick = |text: &str, ranges: &[Range<usize>]| -> Vec<String> {
            ranges
                .iter()
                .map(|range| text[range.clone()].to_string())
                .collect()
        };
        assert_eq!(pick(old_text, &old), ["+ 1"]);
        assert_eq!(pick(new_text, &new), ["* 2"]);
    }

    #[test]
    fn deletions_pair_with_the_additions_after_them() {
        let file = &parse_unified_diff(
            "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n ctx\n-old one\n-gone\n+new one\n",
        )[0];
        let refined = refine(&file.hunks[0]);
        // "old one" pairs with "new one"; "gone" has no partner.
        assert_eq!(
            refined.get(&1).map(Vec::as_slice),
            Some(std::slice::from_ref(&(0..3)))
        );
        assert_eq!(
            refined.get(&3).map(Vec::as_slice),
            Some(std::slice::from_ref(&(0..3)))
        );
        assert_eq!(refined.get(&2), None);
    }
}
