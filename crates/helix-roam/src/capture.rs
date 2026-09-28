//! Creating a node from a template.
//!
//! Org-capture's template language is an Emacs one, built on `%`-escapes and
//! plists. Reproducing it would import syntax nobody here asked for, so this
//! is the small subset that earns its place, written the way Helix's own
//! configuration reads: `${placeholder}` substitution, plus `%?` for where the
//! cursor should land.
//!
//! Expansion is pure text, so what a template produces is settled by tests
//! rather than by opening the editor.

use std::fmt;

/// A named shape for a new node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// The key that selects it.
    pub key: String,
    /// What the picker shows.
    pub description: String,
    /// The file to create, itself expanded — usually `${slug}.org`.
    pub file: String,
    /// The file's contents.
    pub content: String,
}

/// What a template is filled in with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fields {
    pub title: String,
    pub slug: String,
    pub id: String,
    /// Today, as `2026-09-18`.
    pub date: String,
}

/// A filled-in template, ready to be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capture {
    /// Path of the file to create, relative to the notes directory.
    pub path: String,
    pub content: String,
    /// Byte offset where `%?` was, if the template marked one.
    pub cursor: Option<usize>,
}

/// A placeholder a template used and nothing supplies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownPlaceholder(pub String);

impl fmt::Display for UnknownPlaceholder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown placeholder ${{{}}}; known ones are title, slug, id and date",
            self.0
        )
    }
}

impl std::error::Error for UnknownPlaceholder {}

impl Fields {
    /// The value of a placeholder, if it names one.
    fn get(&self, name: &str) -> Option<&str> {
        match name {
            "title" => Some(&self.title),
            "slug" => Some(&self.slug),
            "id" => Some(&self.id),
            "date" => Some(&self.date),
            _ => None,
        }
    }
}

/// Fills in a template.
///
/// An unknown placeholder is an error rather than being left as written: a
/// typo that silently ends up in the note is worse than one that is reported.
pub fn expand(template: &Template, fields: &Fields) -> Result<Capture, UnknownPlaceholder> {
    let path = substitute(&template.file, fields)?;
    let content = substitute(&template.content, fields)?;

    // `%?` marks the cursor and is removed; only the first is honoured.
    let (content, cursor) = match content.find("%?") {
        Some(at) => (
            format!("{}{}", &content[..at], &content[at + 2..]),
            Some(at),
        ),
        None => (content, None),
    };

    Ok(Capture {
        path,
        content,
        cursor,
    })
}

/// Replaces every `${name}` with its value.
fn substitute(text: &str, fields: &Fields) -> Result<String, UnknownPlaceholder> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];

        let Some(close) = after.find('}') else {
            // An unclosed `${` is literal text rather than a broken template.
            out.push_str(&rest[at..]);
            return Ok(out);
        };

        let name = &after[..close];
        let value = fields
            .get(name)
            .ok_or_else(|| UnknownPlaceholder(name.to_string()))?;
        out.push_str(value);
        rest = &after[close + 1..];
    }

    out.push_str(rest);
    Ok(out)
}

/// A file name that is safe on every platform and still readable.
pub fn slugify(title: &str) -> String {
    let mut out = String::with_capacity(title.len());

    for c in title.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }

    let slug = out.trim_matches('-');
    if slug.is_empty() {
        "node".to_string()
    } else {
        slug.to_string()
    }
}

/// The template used when the configuration names none.
pub fn default_template() -> Template {
    Template {
        key: "d".to_string(),
        description: "Default".to_string(),
        file: "${slug}.org".to_string(),
        content: ":PROPERTIES:\n:ID:       ${id}\n:END:\n#+title: ${title}\n\n%?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Fields {
        Fields {
            title: "A New Note".to_string(),
            slug: "a-new-note".to_string(),
            id: "6ba7b810-9dad-11d1-80b4-00c04fd430c8".to_string(),
            date: "2026-09-18".to_string(),
        }
    }

    #[test]
    fn the_default_template_produces_a_file_node() {
        let out = expand(&default_template(), &fields()).unwrap();

        assert_eq!(out.path, "a-new-note.org");
        assert_eq!(
            out.content,
            ":PROPERTIES:\n:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:END:\n\
             #+title: A New Note\n\n"
        );
        // The cursor lands where `%?` was, which is the end here.
        assert_eq!(out.cursor, Some(out.content.len()));
    }

    #[test]
    fn every_placeholder_is_substituted_including_in_the_file_name() {
        let template = Template {
            key: "j".into(),
            description: "Journal".into(),
            file: "journal/${date}-${slug}.org".into(),
            content: "#+title: ${title} (${date})\nid ${id}\n".into(),
        };
        let out = expand(&template, &fields()).unwrap();

        assert_eq!(out.path, "journal/2026-09-18-a-new-note.org");
        assert_eq!(
            out.content,
            "#+title: A New Note (2026-09-18)\nid 6ba7b810-9dad-11d1-80b4-00c04fd430c8\n"
        );
        assert_eq!(out.cursor, None);
    }

    #[test]
    fn an_unknown_placeholder_is_reported_rather_than_written_into_the_note() {
        let template = Template {
            key: "x".into(),
            description: "Broken".into(),
            file: "x.org".into(),
            content: "#+title: ${titel}\n".into(),
        };

        assert_eq!(
            expand(&template, &fields()),
            Err(UnknownPlaceholder("titel".to_string()))
        );
    }

    #[test]
    fn an_unclosed_placeholder_is_literal_text() {
        let template = Template {
            key: "x".into(),
            description: "Literal".into(),
            file: "x.org".into(),
            content: "costs ${100 and more".into(),
        };

        assert_eq!(
            expand(&template, &fields()).unwrap().content,
            "costs ${100 and more"
        );
    }

    #[test]
    fn only_the_first_cursor_mark_is_honoured() {
        let template = Template {
            key: "x".into(),
            description: "Two marks".into(),
            file: "x.org".into(),
            content: "one %? two %? three".into(),
        };
        let out = expand(&template, &fields()).unwrap();

        assert_eq!(out.cursor, Some(4));
        // The second stays as written rather than vanishing silently.
        assert_eq!(out.content, "one  two %? three");
    }

    #[test]
    fn slugs_are_readable_and_safe() {
        assert_eq!(slugify("A New Note"), "a-new-note");
        assert_eq!(slugify("C++ / Rust: a comparison"), "c-rust-a-comparison");
        assert_eq!(slugify("  spaces  "), "spaces");
        // Something with no usable characters still needs a name.
        assert_eq!(slugify("///"), "node");
        // Non-ASCII letters are letters.
        assert_eq!(slugify("Café Noir"), "café-noir");
    }
}
