//! What Org draws differently from how it is written: entities such as
//! `\alpha` as `α` (Org's `org-pretty-entities`), and a link as its
//! description (`org-link-descriptive`).
//!
//! The result is a list of ranges, each drawn as one grapheme; the editor
//! turns them into conceals. A link keeps its description visible by being
//! two ranges: the opening brackets and target folded into the first
//! grapheme of the description, and the closing brackets into its last.
//! Nothing is concealed in source, example and export blocks, in
//! fixed-width lines, or in `=verbatim=` and `~code~`, where Org does not
//! read entities or links either.

use unicode_segmentation::UnicodeSegmentation;

/// A range of characters, drawn as `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pretty {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

/// Org's entities that have a character of their own, by name. A subset of
/// `org-entities`: the Greek alphabet, arrows, relations and operators, and
/// the typographic ones people write.
const ENTITIES: &[(&str, &str)] = &[
    ("alpha", "α"),
    ("beta", "β"),
    ("gamma", "γ"),
    ("delta", "δ"),
    ("epsilon", "ε"),
    ("varepsilon", "ε"),
    ("zeta", "ζ"),
    ("eta", "η"),
    ("theta", "θ"),
    ("vartheta", "ϑ"),
    ("iota", "ι"),
    ("kappa", "κ"),
    ("lambda", "λ"),
    ("mu", "μ"),
    ("nu", "ν"),
    ("xi", "ξ"),
    ("omicron", "ο"),
    ("pi", "π"),
    ("varpi", "ϖ"),
    ("rho", "ρ"),
    ("varrho", "ϱ"),
    ("sigma", "σ"),
    ("varsigma", "ς"),
    ("sigmaf", "ς"),
    ("tau", "τ"),
    ("upsilon", "υ"),
    ("phi", "φ"),
    ("varphi", "ϕ"),
    ("chi", "χ"),
    ("psi", "ψ"),
    ("omega", "ω"),
    ("Gamma", "Γ"),
    ("Delta", "Δ"),
    ("Theta", "Θ"),
    ("Lambda", "Λ"),
    ("Xi", "Ξ"),
    ("Pi", "Π"),
    ("Sigma", "Σ"),
    ("Upsilon", "Υ"),
    ("Phi", "Φ"),
    ("Psi", "Ψ"),
    ("Omega", "Ω"),
    ("to", "→"),
    ("rarr", "→"),
    ("rightarrow", "→"),
    ("larr", "←"),
    ("leftarrow", "←"),
    ("gets", "←"),
    ("uarr", "↑"),
    ("uparrow", "↑"),
    ("darr", "↓"),
    ("downarrow", "↓"),
    ("harr", "↔"),
    ("leftrightarrow", "↔"),
    ("rArr", "⇒"),
    ("Rightarrow", "⇒"),
    ("lArr", "⇐"),
    ("Leftarrow", "⇐"),
    ("hArr", "⇔"),
    ("Leftrightarrow", "⇔"),
    ("mapsto", "↦"),
    ("leq", "≤"),
    ("le", "≤"),
    ("geq", "≥"),
    ("ge", "≥"),
    ("neq", "≠"),
    ("ne", "≠"),
    ("approx", "≈"),
    ("equiv", "≡"),
    ("sim", "∼"),
    ("simeq", "≃"),
    ("propto", "∝"),
    ("ll", "≪"),
    ("gg", "≫"),
    ("in", "∈"),
    ("isin", "∈"),
    ("notin", "∉"),
    ("ni", "∋"),
    ("subset", "⊂"),
    ("supset", "⊃"),
    ("subseteq", "⊆"),
    ("supseteq", "⊇"),
    ("cup", "∪"),
    ("cap", "∩"),
    ("emptyset", "∅"),
    ("empty", "∅"),
    ("forall", "∀"),
    ("exists", "∃"),
    ("nexist", "∄"),
    ("neg", "¬"),
    ("lnot", "¬"),
    ("land", "∧"),
    ("wedge", "∧"),
    ("lor", "∨"),
    ("vee", "∨"),
    ("oplus", "⊕"),
    ("otimes", "⊗"),
    ("perp", "⊥"),
    ("angle", "∠"),
    ("infin", "∞"),
    ("infty", "∞"),
    ("partial", "∂"),
    ("nabla", "∇"),
    ("sum", "∑"),
    ("prod", "∏"),
    ("int", "∫"),
    ("sqrt", "√"),
    ("radic", "√"),
    ("pm", "±"),
    ("plusmn", "±"),
    ("mp", "∓"),
    ("times", "×"),
    ("div", "÷"),
    ("cdot", "⋅"),
    ("sdot", "⋅"),
    ("circ", "∘"),
    ("ast", "∗"),
    ("star", "⋆"),
    ("prime", "′"),
    ("Prime", "″"),
    ("deg", "°"),
    ("hbar", "ℏ"),
    ("ell", "ℓ"),
    ("aleph", "ℵ"),
    ("Re", "ℜ"),
    ("Im", "ℑ"),
    ("wp", "℘"),
    ("dagger", "†"),
    ("ddagger", "‡"),
    ("bull", "•"),
    ("bullet", "•"),
    ("dots", "…"),
    ("ldots", "…"),
    ("hellip", "…"),
    ("cdots", "⋯"),
    ("mdash", "—"),
    ("ndash", "–"),
    ("laquo", "«"),
    ("raquo", "»"),
    ("lsaquo", "‹"),
    ("rsaquo", "›"),
    ("lsquo", "‘"),
    ("rsquo", "’"),
    ("ldquo", "“"),
    ("rdquo", "”"),
    ("sect", "§"),
    ("S", "§"),
    ("para", "¶"),
    ("P", "¶"),
    ("copy", "©"),
    ("copyright", "©"),
    ("reg", "®"),
    ("trade", "™"),
    ("euro", "€"),
    ("EUR", "€"),
    ("pound", "£"),
    ("yen", "¥"),
    ("cent", "¢"),
    ("checkmark", "✓"),
    ("smiley", "☺"),
    ("frown", "☹"),
    ("clubs", "♣"),
    ("spades", "♠"),
    ("hearts", "♥"),
    ("diamonds", "♦"),
];

/// The character an entity is drawn as.
pub fn entity(name: &str) -> Option<&'static str> {
    ENTITIES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, drawn)| *drawn)
}

/// The ranges of `text` Org draws differently, in character offsets.
pub fn conceals(text: &str) -> Vec<Pretty> {
    let mut found = Vec::new();
    let mut offset = 0;
    let mut in_block = false;
    for line in text.split_inclusive('\n') {
        let chars = line.chars().count();
        let trimmed = line.trim_start();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("#+begin_") {
            in_block = true;
        } else if lower.starts_with("#+end_") {
            in_block = false;
        } else if !in_block && !trimmed.starts_with(": ") && trimmed.trim_end() != ":" {
            line_conceals(line.trim_end_matches(['\n', '\r']), offset, &mut found);
        }
        offset += chars;
    }
    found
}

/// Conceals within one line, which starts at character `offset`.
fn line_conceals(line: &str, offset: usize, found: &mut Vec<Pretty>) {
    let chars: Vec<char> = line.chars().collect();
    let verbatim = verbatim_spans(&chars);
    let quoted = |at: usize| verbatim.iter().any(|span| span.contains(&at));

    let mut i = 0;
    while i < chars.len() {
        if quoted(i) {
            i += 1;
            continue;
        }
        if chars[i] == '[' && chars.get(i + 1) == Some(&'[') {
            if let Some((pieces, end)) = link_at(&chars, i) {
                found.extend(pieces.into_iter().map(|mut piece| {
                    piece.start += offset;
                    piece.end += offset;
                    piece
                }));
                i = end;
                continue;
            }
        }
        if chars[i] == '\\' && (i == 0 || chars[i - 1] != '\\') {
            let name_end = (i + 1..chars.len())
                .find(|&j| !chars[j].is_ascii_alphabetic())
                .unwrap_or(chars.len());
            let name: String = chars[i + 1..name_end].iter().collect();
            if let Some(drawn) = entity(&name) {
                // `\alpha{}` is the form that may touch a letter; the braces
                // go with it.
                let end =
                    if chars.get(name_end) == Some(&'{') && chars.get(name_end + 1) == Some(&'}') {
                        name_end + 2
                    } else {
                        name_end
                    };
                found.push(Pretty {
                    start: offset + i,
                    end: offset + end,
                    replacement: drawn.to_string(),
                });
                i = end;
                continue;
            }
        }
        i += 1;
    }
}

/// A link starting at `start`: the ranges that leave its description (or,
/// without one, its target) showing, and where the link ends.
fn link_at(chars: &[char], start: usize) -> Option<(Vec<Pretty>, usize)> {
    let close = (start + 2..chars.len().saturating_sub(1))
        .find(|&j| chars[j] == ']' && chars[j + 1] == ']')?;
    let inner = &chars[start + 2..close];
    // `[[target][description]]`, or `[[target]]` showing the target.
    let shown_from = match inner.iter().position(|&c| c == ']') {
        Some(split) if inner.get(split + 1) == Some(&'[') => start + 2 + split + 2,
        Some(_) => return None,
        None => start + 2,
    };
    let shown_to = close;
    if shown_to <= shown_from {
        return None;
    }
    let end = close + 2;

    let shown: String = chars[shown_from..shown_to].iter().collect();
    let graphemes: Vec<&str> = shown.graphemes(true).collect();
    let first = *graphemes.first()?;
    let last = *graphemes.last()?;
    let pieces = if graphemes.len() == 1 {
        vec![Pretty {
            start,
            end,
            replacement: first.to_string(),
        }]
    } else {
        vec![
            Pretty {
                start,
                end: shown_from + first.chars().count(),
                replacement: first.to_string(),
            },
            Pretty {
                start: shown_to - last.chars().count(),
                end,
                replacement: last.to_string(),
            },
        ]
    };
    Some((pieces, end))
}

/// Where `=verbatim=` and `~code~` are on a line, as Org finds them: a
/// marker after a space, an opening bracket or quote, or the line's start,
/// then text not beginning or ending with a space, then the same marker
/// before a space, punctuation or the line's end.
fn verbatim_spans(chars: &[char]) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let marker = chars[i];
        let opens = (marker == '=' || marker == '~')
            && (i == 0 || chars[i - 1].is_whitespace() || "({['\"-".contains(chars[i - 1]))
            && chars.get(i + 1).is_some_and(|c| !c.is_whitespace());
        if opens {
            let closing = (i + 2..chars.len()).find(|&j| {
                chars[j] == marker
                    && !chars[j - 1].is_whitespace()
                    && chars
                        .get(j + 1)
                        .is_none_or(|c| c.is_whitespace() || ".,;:!?)}]'\"-".contains(*c))
            });
            if let Some(close) = closing {
                spans.push(i..close + 1);
                i = close + 1;
                continue;
            }
        }
        i += 1;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies the conceals to `text`, as the display would.
    fn drawn(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        let found = conceals(text);
        while i < chars.len() {
            match found.iter().find(|pretty| pretty.start == i) {
                Some(pretty) => {
                    out.push_str(&pretty.replacement);
                    i = pretty.end;
                }
                None => {
                    out.push(chars[i]);
                    i += 1;
                }
            }
        }
        out
    }

    #[test]
    fn entities_are_drawn_as_their_characters() {
        assert_eq!(drawn("\\alpha + \\beta{}x \\to y\n"), "α + βx → y\n");
        // Not an entity, a longer name, or an escaped backslash: left alone.
        assert_eq!(
            drawn("\\alphabet \\foo \\\\alpha\n"),
            "\\alphabet \\foo \\\\alpha\n"
        );
    }

    #[test]
    fn links_are_drawn_as_their_descriptions() {
        assert_eq!(
            drawn("See [[https://orgmode.org][the manual]] and [[id:x][é]].\n"),
            "See the manual and é.\n"
        );
        assert_eq!(drawn("[[file:notes.org]]\n"), "file:notes.org\n");
        // Unfinished or empty: left alone.
        assert_eq!(drawn("[[half\n[[x][]]\n"), "[[half\n[[x][]]\n");
    }

    #[test]
    fn nothing_is_concealed_where_org_reads_text_literally() {
        let text =
            "=\\alpha= ~[[x][y]]~ \\alpha\n#+begin_src latex\n\\alpha\n#+end_src\n: \\beta\n";
        assert_eq!(
            drawn(text),
            "=\\alpha= ~[[x][y]]~ α\n#+begin_src latex\n\\alpha\n#+end_src\n: \\beta\n"
        );
    }

    #[test]
    fn offsets_count_characters_not_bytes() {
        let found = conceals("é \\alpha\n");
        assert_eq!((found[0].start, found[0].end), (2, 8));
    }
}
