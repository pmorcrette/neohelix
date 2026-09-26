//! Reading BibTeX entries, far enough to cite them.
//!
//! Org's `basic` citation processor needs an entry's authors, year and
//! title and nothing more; this reads those, and every other field as
//! text, from `@kind{key, field = {value}, …}`. Braces inside a value are
//! protection for LaTeX and are dropped; `@string` macros are not expanded.

/// One bibliography entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    /// `article`, `book`, … lower-cased.
    pub kind: String,
    /// Field names lower-cased, values with protecting braces removed.
    pub fields: Vec<(String, String)>,
}

impl Entry {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(field, _)| field == name)
            .map(|(_, value)| value.as_str())
    }

    /// The authors' family names: "Doe, Jane and John Smith" gives
    /// `["Doe", "Smith"]`. Editors stand in when there are no authors.
    pub fn surnames(&self) -> Vec<String> {
        let names = self
            .field("author")
            .or_else(|| self.field("editor"))
            .unwrap_or("");
        names
            .split(" and ")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| match name.split_once(',') {
                Some((family, _)) => family.trim().to_string(),
                None => name.split_whitespace().last().unwrap_or(name).to_string(),
            })
            .collect()
    }

    /// The year, from `year` or the start of `date`.
    pub fn year(&self) -> Option<String> {
        self.field("year").map(str::to_string).or_else(|| {
            self.field("date")
                .map(|date| date.chars().take(4).collect())
        })
    }

    /// "Doe", "Doe and Smith", or "Doe et al.", as author-year styles name
    /// a work.
    pub fn short_authors(&self) -> String {
        let names = self.surnames();
        match names.as_slice() {
            [] => self.key.clone(),
            [one] => one.clone(),
            [one, two] => format!("{one} and {two}"),
            [first, ..] => format!("{first} et al."),
        }
    }

    /// A bibliography line: "Doe, Jane (2020). Title. Journal."
    pub fn reference(&self) -> String {
        let mut out = self
            .field("author")
            .or_else(|| self.field("editor"))
            .map(|authors| authors.replace(" and ", "; "))
            .unwrap_or_else(|| self.key.clone());
        if let Some(year) = self.year() {
            out.push_str(&format!(" ({year})"));
        }
        out.push('.');
        if let Some(title) = self.field("title") {
            out.push_str(&format!(" {title}."));
        }
        let container = self
            .field("journal")
            .or_else(|| self.field("journaltitle"))
            .or_else(|| self.field("booktitle"))
            .or_else(|| self.field("publisher"));
        if let Some(container) = container {
            out.push_str(&format!(" {container}."));
        }
        out
    }
}

/// Every entry in a `.bib` file's text.
pub fn parse(text: &str) -> Vec<Entry> {
    let chars: Vec<char> = text.chars().collect();
    let mut entries = Vec::new();
    let mut at = 0;

    while let Some(start) = (at..chars.len()).find(|&i| chars[i] == '@') {
        let open = match (start..chars.len()).find(|&i| chars[i] == '{' || chars[i] == '(') {
            Some(open) => open,
            None => break,
        };
        let kind: String = chars[start + 1..open]
            .iter()
            .collect::<String>()
            .trim()
            .to_ascii_lowercase();
        let close = matching(&chars, open).unwrap_or(chars.len());
        at = close.max(open + 1);
        if matches!(kind.as_str(), "string" | "preamble" | "comment") || kind.is_empty() {
            continue;
        }

        let body: String = chars[open + 1..close.min(chars.len())].iter().collect();
        let Some((key, rest)) = body.split_once(',') else {
            continue;
        };
        entries.push(Entry {
            key: key.trim().to_string(),
            kind,
            fields: fields(rest),
        });
    }
    entries
}

/// Where the brace or parenthesis at `open` closes.
fn matching(chars: &[char], open: usize) -> Option<usize> {
    let close = if chars[open] == '(' { ')' } else { '}' };
    let mut depth = 0;
    for (at, c) in chars.iter().enumerate().skip(open) {
        if *c == chars[open] {
            depth += 1;
        } else if *c == close {
            depth -= 1;
            if depth == 0 {
                return Some(at);
            }
        }
    }
    None
}

/// `author = {Doe, Jane}, year = 2020, title = "A {B} C"`.
fn fields(body: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = body.chars().collect();
    let mut out = Vec::new();
    let mut at = 0;

    while at < chars.len() {
        let Some(eq) = (at..chars.len()).find(|&i| chars[i] == '=') else {
            break;
        };
        let name: String = chars[at..eq]
            .iter()
            .collect::<String>()
            .trim()
            .trim_start_matches(',')
            .trim()
            .to_ascii_lowercase();
        let mut value = String::new();
        let mut i = eq + 1;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        match chars.get(i) {
            Some('{') => {
                let end = matching(&chars, i).unwrap_or(chars.len() - 1);
                value.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            Some('"') => {
                let end = (i + 1..chars.len())
                    .find(|&j| chars[j] == '"')
                    .unwrap_or(chars.len());
                value.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            _ => {
                let end = (i..chars.len())
                    .find(|&j| chars[j] == ',')
                    .unwrap_or(chars.len());
                value.extend(&chars[i..end]);
                i = end;
            }
        }
        let value: String = value.chars().filter(|c| !matches!(c, '{' | '}')).collect();
        let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
        if !name.is_empty() {
            out.push((name, value));
        }
        at = (i..chars.len())
            .find(|&j| chars[j] == ',')
            .map_or(chars.len(), |comma| comma + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIB: &str = r#"
@string{jfp = "Journal of Functional Programming"}
@article{doe2020,
  author = {Doe, Jane and John Smith},
  title  = {A {Study} of Things},
  journal = "Journal of Stuff",
  year   = 2020,
}
@book{solo,
  author = {Ann Other},
  title = {Alone},
  publisher = {Press},
  date = {2019-05-01}
}
@misc{many, author = {A, B and C, D and E, F}, year = {2001}}
"#;

    #[test]
    fn entries_and_their_fields_are_read() {
        let entries = parse(BIB);
        assert_eq!(entries.len(), 3);
        let doe = &entries[0];
        assert_eq!(doe.key, "doe2020");
        assert_eq!(doe.kind, "article");
        assert_eq!(doe.field("title"), Some("A Study of Things"));
        assert_eq!(doe.field("journal"), Some("Journal of Stuff"));
        assert_eq!(doe.year().as_deref(), Some("2020"));
        assert_eq!(entries[1].year().as_deref(), Some("2019"));
    }

    #[test]
    fn authors_are_named_the_way_author_year_styles_name_them() {
        let entries = parse(BIB);
        assert_eq!(entries[0].surnames(), ["Doe", "Smith"]);
        assert_eq!(entries[0].short_authors(), "Doe and Smith");
        assert_eq!(entries[1].short_authors(), "Other");
        assert_eq!(entries[2].short_authors(), "A et al.");
    }

    #[test]
    fn a_reference_reads_like_a_bibliography_line() {
        let entries = parse(BIB);
        assert_eq!(
            entries[0].reference(),
            "Doe, Jane; John Smith (2020). A Study of Things. Journal of Stuff."
        );
        assert_eq!(entries[1].reference(), "Ann Other (2019). Alone. Press.");
    }
}
