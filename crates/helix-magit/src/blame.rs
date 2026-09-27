//! Blame: which commit last changed each line of a file.

use std::path::Path;

use crate::command::GitCommand;

/// One line of a file, and the commit it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BlameLine {
    /// The full hash; all zeroes for a line not committed yet.
    pub hash: String,
    pub author: String,
    /// Seconds since the epoch, and the author's zone as git writes it.
    pub time: i64,
    pub tz: String,
    pub summary: String,
    /// The file's name in that commit, which a rename makes differ.
    pub filename: String,
    /// The line's number in that commit's version, from 1.
    pub orig_line: usize,
    /// The commit and file name the line had before that commit changed
    /// it, when there was a before.
    pub previous: Option<(String, String)>,
    pub content: String,
}

impl BlameLine {
    pub fn short(&self) -> &str {
        &self.hash[..self.hash.len().min(7)]
    }

    pub fn is_uncommitted(&self) -> bool {
        self.hash.chars().all(|c| c == '0')
    }

    /// The author's date, in the author's zone.
    pub fn date(&self) -> String {
        date(self.time, &self.tz)
    }
}

/// Reads `git blame --line-porcelain`, which describes every line in full.
pub fn parse(text: &str) -> Vec<BlameLine> {
    let mut lines = Vec::new();
    let mut current: Option<BlameLine> = None;
    for line in text.lines() {
        if let Some(content) = line.strip_prefix('\t') {
            if let Some(mut done) = current.take() {
                done.content = content.to_string();
                lines.push(done);
            }
            continue;
        }
        match current.as_mut() {
            None => {
                let mut words = line.split(' ');
                let hash = words.next().unwrap_or_default();
                if hash.len() >= 40 && hash.chars().all(|c| c.is_ascii_hexdigit()) {
                    current = Some(BlameLine {
                        hash: hash.to_string(),
                        orig_line: words.next().and_then(|n| n.parse().ok()).unwrap_or(1),
                        ..BlameLine::default()
                    });
                }
            }
            Some(entry) => {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                match key {
                    "author" => entry.author = value.to_string(),
                    "author-time" => entry.time = value.parse().unwrap_or(0),
                    "author-tz" => entry.tz = value.to_string(),
                    "summary" => entry.summary = value.to_string(),
                    "filename" => entry.filename = value.to_string(),
                    "previous" => {
                        if let Some((hash, name)) = value.split_once(' ') {
                            entry.previous = Some((hash.to_string(), name.to_string()));
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    lines
}

/// How a blame looks for where lines come from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlameOptions {
    /// Lines moved within the file keep their older commit (`-M`).
    pub moves: bool,
    /// Lines copied or moved from other files keep theirs: from the files
    /// the same commit changed (1, `-C`), also from where the file was
    /// created (2, `-C -C`), or from any commit (3, `-C -C -C`).
    pub copies: u8,
    /// Reverse blame since this revision: for each line as it was there,
    /// the last commit that still had it (`--reverse`).
    pub reverse_from: Option<String>,
}

impl BlameOptions {
    /// What differs from a plain blame, for the view's title.
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if self.moves {
            parts.push("-M".to_string());
        }
        if self.copies > 0 {
            parts.push(vec!["-C"; self.copies as usize].join(" "));
        }
        parts.join(" ")
    }
}

/// Blames `path` as of `rev`, or as it is in the working tree.
pub fn blame(workdir: &Path, path: &Path, rev: Option<&str>) -> Result<Vec<BlameLine>, String> {
    blame_with(workdir, path, rev, &BlameOptions::default())
}

/// [`blame`] with `options`. Reversed, `rev` is where the history ends
/// (HEAD when `None`, as the working tree has no history).
pub fn blame_with(
    workdir: &Path,
    path: &Path,
    rev: Option<&str>,
    options: &BlameOptions,
) -> Result<Vec<BlameLine>, String> {
    let mut args = vec!["blame".to_string(), "--line-porcelain".to_string()];
    if options.moves {
        args.push("-M".into());
    }
    args.extend((0..options.copies.min(3)).map(|_| "-C".to_string()));
    if let Some(rev) = rev {
        crate::log::LogFilter::valid_range(rev)?;
    }
    match &options.reverse_from {
        Some(from) => {
            crate::log::LogFilter::valid_range(from)?;
            // git's own words for an empty range are "More than one commit
            // to dig up from".
            let end = rev.unwrap_or("HEAD");
            let commit = |rev: &str| {
                GitCommand::new(
                    workdir,
                    vec![
                        "rev-parse".into(),
                        "--verify".into(),
                        format!("{rev}^{{commit}}"),
                    ],
                )
                .run()
                .ok()
                .filter(|output| output.success)
                .map(|output| output.stdout.trim().to_string())
            };
            if commit(from).is_some() && commit(from) == commit(end) {
                return Err(format!(
                    "{from} is {end}: a reverse blame starts from an older commit"
                ));
            }
            args.push("--reverse".into());
            args.push(format!("{from}..{}", rev.unwrap_or("HEAD")));
        }
        None => args.extend(rev.map(str::to_string)),
    }
    args.push("--".into());
    args.push(path.display().to_string());
    let output = GitCommand::new(workdir, args)
        .run()
        .map_err(|err| err.to_string())?;
    if output.success {
        Ok(parse(&output.stdout))
    } else {
        Err(output.summary())
    }
}

/// `YYYY-MM-DD` for a moment, in a zone written as git writes it (`+0200`).
pub fn date(time: i64, tz: &str) -> String {
    let offset = tz
        .get(1..5)
        .and_then(|digits| {
            let hours: i64 = digits.get(0..2)?.parse().ok()?;
            let minutes: i64 = digits.get(2..4)?.parse().ok()?;
            Some((hours * 60 + minutes) * 60)
        })
        .map(|seconds| {
            if tz.starts_with('-') {
                -seconds
            } else {
                seconds
            }
        })
        .unwrap_or(0);
    let days = (time + offset).div_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORCELAIN: &str = "\
1a2b3c4d5e6f7a8b9c0d1a2b3c4d5e6f7a8b9c0d 1 1 1
author Ann
author-mail <a@e>
author-time 1790000000
author-tz +0200
committer Ann
summary First commit
boundary
filename src/lib.rs
\tfn main() {
9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f9f 3 2 1
author Bob
author-time 1790086400
author-tz -0500
summary Fix it
previous 1a2b3c4d5e6f7a8b9c0d1a2b3c4d5e6f7a8b9c0d src/old.rs
filename src/lib.rs
\t    fixed();
";

    #[test]
    fn every_line_is_read_with_its_commit() {
        let lines = parse(PORCELAIN);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].short(), "1a2b3c4");
        assert_eq!(lines[0].author, "Ann");
        assert_eq!(lines[0].content, "fn main() {");
        assert_eq!(lines[0].previous, None);
        assert_eq!(lines[1].orig_line, 3);
        assert_eq!(lines[1].content, "    fixed();");
        assert_eq!(
            lines[1].previous,
            Some((
                "1a2b3c4d5e6f7a8b9c0d1a2b3c4d5e6f7a8b9c0d".to_string(),
                "src/old.rs".to_string()
            ))
        );
    }

    #[test]
    fn dates_are_the_authors_own() {
        // 1790000000 is 2026-09-21 14:13:20 UTC.
        assert_eq!(date(1_790_000_000, "+0000"), "2026-09-21");
        assert_eq!(date(0, "+0000"), "1970-01-01");
        // Near midnight, the zone decides the day.
        assert_eq!(date(1_789_948_800 - 1, "+0000"), "2026-09-20");
        assert_eq!(date(1_789_948_800 - 1, "+0100"), "2026-09-21");
        assert_eq!(date(1_789_948_800, "-0100"), "2026-09-20");
        // A leap day.
        assert_eq!(date(951_782_400, "+0000"), "2000-02-29");
    }

    #[test]
    fn uncommitted_lines_are_recognised() {
        let line = BlameLine {
            hash: "0".repeat(40),
            ..BlameLine::default()
        };
        assert!(line.is_uncommitted());
    }
}
