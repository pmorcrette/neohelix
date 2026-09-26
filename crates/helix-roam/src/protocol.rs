//! `org-protocol://` URLs: how a browser, or any other program, hands the
//! notes something to keep.
//!
//! Emacs receives these through `emacsclient`, as a file name it intercepts.
//! Helix has no server to hand a URL to, so the fork takes the same URL as
//! an argument — `hx 'org-protocol://capture?url=…'` — and a desktop entry
//! registered for the `org-protocol` scheme starts the editor on it. This
//! module reads the URL; what each request does is the editor's.
//!
//! Both of Org's URL forms are read: the query form Org uses since 9.0
//! (`capture?url=…&title=…`) and the older path form
//! (`capture:/template/url/title/body`), which bookmarklets written for
//! older Orgs still send.

use std::path::{Path, PathBuf};

use uuid::Uuid;

/// What an `org-protocol` URL asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// Keep a link to paste later.
    StoreLink { url: String, title: String },
    /// Capture a page: its link, its title, and the text selected on it.
    Capture {
        template: Option<String>,
        url: String,
        title: String,
        body: String,
    },
    /// Org-Roam's: the node about a page, found by its `:ROAM_REFS:` or made.
    RoamRef {
        reference: String,
        title: String,
        body: String,
    },
    /// Org-Roam's: open a node.
    RoamNode(Uuid),
}

/// Why a URL cannot be acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    NotOrgProtocol,
    /// A sub-protocol nothing here handles, such as `open-source`, which
    /// needs a mapping from web addresses to local files.
    Unsupported(String),
    /// A request missing what it cannot work without.
    Missing(&'static str),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::NotOrgProtocol => write!(f, "not an org-protocol:// URL"),
            ProtocolError::Unsupported(name) => {
                write!(f, "org-protocol://{name} is not handled")
            }
            ProtocolError::Missing(what) => write!(f, "the request has no {what}"),
        }
    }
}

/// `%XX` decoding. `+` stays a plus: Org's decoder leaves it, and a
/// bookmarklet encodes spaces with `encodeURIComponent`, which writes `%20`.
pub fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        // Read as bytes: slicing the text could split a character when a
        // `%` is followed by something that is not a hex digit.
        let hex = |byte: u8| (byte as char).to_digit(16);
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[at + 1]), hex(bytes[at + 2])) {
                out.push((high * 16 + low) as u8);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads an `org-protocol://` URL.
pub fn parse(url: &str) -> Result<Request, ProtocolError> {
    let rest = url
        .strip_prefix("org-protocol://")
        .or_else(|| url.strip_prefix("org-protocol:/"))
        .ok_or(ProtocolError::NotOrgProtocol)?;

    // `capture?url=…` or `capture:/t/url/title/body`.
    let (name, params) = match rest.find(['?', ':', '/']) {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let pairs: Vec<(String, String)> = if let Some(query) = params.strip_prefix('?') {
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .map(|(key, value)| (key.to_string(), decode(value)))
            .collect()
    } else {
        // The old form: positional, slash-separated, each part encoded.
        let keys: &[&str] = match name {
            "capture" => &["template", "url", "title", "body"],
            "store-link" => &["url", "title"],
            _ => &[],
        };
        let parts = params
            .trim_start_matches(':')
            .trim_start_matches('/')
            .split('/')
            .map(decode);
        keys.iter().map(|key| key.to_string()).zip(parts).collect()
    };
    let get = |key: &str| {
        pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
    };
    let text = |key: &str| get(key).unwrap_or_default();

    match name {
        "store-link" => Ok(Request::StoreLink {
            url: get("url").ok_or(ProtocolError::Missing("url"))?,
            title: text("title"),
        }),
        "capture" => Ok(Request::Capture {
            template: get("template").filter(|t| !t.is_empty()),
            url: text("url"),
            title: text("title"),
            body: text("body"),
        }),
        "roam-ref" => Ok(Request::RoamRef {
            reference: get("ref").ok_or(ProtocolError::Missing("ref"))?,
            title: text("title"),
            body: text("body"),
        }),
        "roam-node" => get("node")
            .and_then(|id| id.parse().ok())
            .map(Request::RoamNode)
            .ok_or(ProtocolError::Missing("node id")),
        other => Err(ProtocolError::Unsupported(other.to_string())),
    }
}

/// `[[url][title]]`, or the bare link when there is no title.
fn link(url: &str, title: &str) -> String {
    let title = title.replace(['[', ']'], "");
    match (url.is_empty(), title.trim().is_empty()) {
        (true, _) => title,
        (false, true) => format!("[[{url}]]"),
        (false, false) => format!("[[{url}][{}]]", title.trim()),
    }
}

/// The selected text as a quote block, or nothing when none was sent.
fn quote(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = body
        .lines()
        .map(|line| {
            // Escaped like any block's lines: a quoted `* ` is not a headline.
            let trimmed = line.trim_start();
            if trimmed.starts_with('*') || trimmed.starts_with("#+") {
                format!(",{line}")
            } else {
                line.to_string()
            }
        })
        .collect();
    format!("#+begin_quote\n{}\n#+end_quote\n", lines.join("\n"))
}

/// The entry a capture appends to the inbox.
pub fn inbox_entry(url: &str, title: &str, body: &str, stamp: &str) -> String {
    let heading = link(url, title);
    let heading = if heading.is_empty() {
        "Captured".to_string()
    } else {
        heading
    };
    format!(
        "* {heading}\n:PROPERTIES:\n:CAPTURED: {stamp}\n:END:\n{}",
        quote(body)
    )
}

/// What a captured page adds under a node made from a template.
pub fn capture_addition(url: &str, title: &str, body: &str) -> String {
    let mut out = String::new();
    let link = link(url, title);
    if !link.is_empty() {
        out.push_str(&link);
        out.push('\n');
    }
    out.push_str(&quote(body));
    out
}

/// A new node about a page, as Org-Roam's default `roam-ref` template makes
/// it: the page's address in `:ROAM_REFS:`, its title as the node's.
pub fn ref_node(id: Uuid, reference: &str, title: &str, body: &str) -> String {
    let title = if title.trim().is_empty() {
        reference
    } else {
        title.trim()
    };
    // A reference with a space has to be quoted in the property.
    let reference = if reference.contains(char::is_whitespace) {
        format!("\"{reference}\"")
    } else {
        reference.to_string()
    };
    format!(
        ":PROPERTIES:\n:ID:       {id}\n:ROAM_REFS: {reference}\n:END:\n#+title: {title}\n\n{}",
        quote(body)
    )
}

/// Every `.org` file under `dir`.
fn org_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "org") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Where in `dir` a property line with `key` holds `value` — the file and
/// the line — searching the files rather than the index, which a URL
/// handled as the editor starts arrives before.
pub fn find_property(dir: &Path, key: &str, value: &str) -> Option<(PathBuf, usize)> {
    let prefix = format!(":{}:", key.to_ascii_uppercase());
    for path in org_files(dir) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, raw) in text.lines().enumerate() {
            let trimmed = raw.trim();
            let Some(rest) = trimmed
                .get(..prefix.len())
                .filter(|head| head.eq_ignore_ascii_case(&prefix))
                .map(|_| &trimmed[prefix.len()..])
            else {
                continue;
            };
            if rest
                .split_whitespace()
                .any(|word| word.trim_matches('"') == value)
            {
                return Some((path, line));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_escapes_are_decoded_and_plus_is_kept() {
        assert_eq!(decode("a%20b%2Fc+d"), "a b/c+d");
        assert_eq!(decode("%C3%A9t%C3%A9"), "été");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("%é%zz"), "%é%zz");
    }

    #[test]
    fn a_capture_reads_its_query() {
        let request = parse(
            "org-protocol://capture?template=w&url=https%3A%2F%2Fexample.org%2Fa&title=A%20page&body=Some%20text",
        )
        .unwrap();
        assert_eq!(
            request,
            Request::Capture {
                template: Some("w".to_string()),
                url: "https://example.org/a".to_string(),
                title: "A page".to_string(),
                body: "Some text".to_string(),
            }
        );
    }

    #[test]
    fn the_old_path_form_is_read_too() {
        let request =
            parse("org-protocol://capture:/w/https%3A%2F%2Fexample.org/A%20page/Text").unwrap();
        assert_eq!(
            request,
            Request::Capture {
                template: Some("w".to_string()),
                url: "https://example.org".to_string(),
                title: "A page".to_string(),
                body: "Text".to_string(),
            }
        );
    }

    #[test]
    fn roam_requests_are_read() {
        assert_eq!(
            parse("org-protocol://roam-ref?template=r&ref=https%3A%2F%2Fx.org&title=X").unwrap(),
            Request::RoamRef {
                reference: "https://x.org".to_string(),
                title: "X".to_string(),
                body: String::new(),
            }
        );
        let id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        assert_eq!(
            parse(&format!("org-protocol://roam-node?node={id}")).unwrap(),
            Request::RoamNode(id.parse().unwrap())
        );
    }

    #[test]
    fn what_cannot_be_acted_on_says_why() {
        assert_eq!(parse("https://x.org"), Err(ProtocolError::NotOrgProtocol));
        assert_eq!(
            parse("org-protocol://open-source?url=x"),
            Err(ProtocolError::Unsupported("open-source".to_string()))
        );
        assert_eq!(
            parse("org-protocol://roam-ref?title=X"),
            Err(ProtocolError::Missing("ref"))
        );
    }

    #[test]
    fn an_inbox_entry_links_the_page_and_quotes_the_selection() {
        assert_eq!(
            inbox_entry("https://x.org", "X [draft]", "* not a headline\nplain", "[2026-09-24 Thu 10:00]"),
            "* [[https://x.org][X draft]]\n:PROPERTIES:\n:CAPTURED: [2026-09-24 Thu 10:00]\n:END:\n\
             #+begin_quote\n,* not a headline\nplain\n#+end_quote\n"
        );
    }

    #[test]
    fn a_ref_node_carries_the_reference() {
        let id: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8".parse().unwrap();
        assert_eq!(
            ref_node(id, "https://x.org", "X", ""),
            ":PROPERTIES:\n:ID:       6ba7b810-9dad-11d1-80b4-00c04fd430c8\n:ROAM_REFS: https://x.org\n:END:\n#+title: X\n\n"
        );
    }
}
