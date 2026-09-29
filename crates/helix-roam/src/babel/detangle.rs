//! Detangling: a file tangled with `:comments link` keeps, around each
//! block's code, the link back to where it came from —
//!
//! ```text
//! # [[file:notes.org::*Setup][Setup:1]]
//! code
//! # Setup:1 ends here
//! ```
//!
//! — so edits made in the tangled file can be written back into the
//! blocks, as Org's `org-babel-detangle` does.

use std::path::{Path, PathBuf};

use crate::source::{self, SourceBlock};

/// One block's code in a tangled file, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The Org file, resolved against the tangled file's directory.
    pub org: PathBuf,
    /// `*Heading` or a block's name.
    pub search: String,
    /// `Heading:2` or the name.
    pub label: String,
    pub code: String,
}

/// `[[file:PATH::SEARCH][LABEL]]` at the end of a comment line, with the
/// comment's start before it.
fn link(line: &str) -> Option<(&str, &str, &str, &str)> {
    let start = line.find("[[file:")?;
    let rest = line[start + 7..].trim_end().strip_suffix("]]")?;
    let (target, label) = rest.split_once("][")?;
    let (path, search) = target.split_once("::").unwrap_or((target, ""));
    Some((line[..start].trim(), path, search, label))
}

/// Why a chunk was not written back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unapplied {
    /// No block of the Org file is the one the link names.
    NoBlock,
    /// The block's references were expanded when it was tangled, so its
    /// tangled code is not its body: writing it back would lose them.
    Noweb,
}

/// The chunks of `tangled`, the text of the file at `tangled_path`.
pub fn chunks(tangled: &str, tangled_path: &Path) -> Vec<Chunk> {
    let dir = tangled_path.parent().unwrap_or(Path::new(""));
    let lines: Vec<&str> = tangled.lines().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let Some((prefix, path, search, label)) = link(lines[at]) else {
            at += 1;
            continue;
        };
        // The whole end line as tangling writes it, not code that happens to
        // end the same way.
        let ending = format!("{prefix} {label} ends here");
        let ending = ending.trim();
        let Some(end) = (at + 1..lines.len()).find(|&i| lines[i].trim() == ending) else {
            at += 1;
            continue;
        };
        let mut code = lines[at + 1..end].join("\n");
        code.push('\n');
        found.push(Chunk {
            org: dir.join(path),
            search: search.to_string(),
            label: label.to_string(),
            code,
        });
        at = end + 1;
    }
    found
}

/// The block of `org` a chunk names.
pub fn block_for(org: &str, chunk: &Chunk) -> Option<SourceBlock> {
    let all = source::blocks(org);
    (0..all.len())
        .find(|&index| {
            let (search, label) = source::link_target(org, &all, index);
            search == chunk.search && label == chunk.label
        })
        .map(|index| all[index].clone())
}

/// `org` with the chunk's code back in its block. A block already holding
/// that code comes back unchanged.
pub fn apply(org: &str, chunk: &Chunk) -> Result<String, Unapplied> {
    let block = block_for(org, chunk).ok_or(Unapplied::NoBlock)?;
    if source::body(org, &block) == chunk.code {
        return Ok(org.to_string());
    }
    if source::tangles_expanded(org, &block) {
        return Err(Unapplied::Noweb);
    }
    Ok(source::replace_body(org, &block, &chunk.code))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: &str = "\
* Setup
#+begin_src sh :tangle setup.sh :comments link
echo one
#+end_src

#+begin_src sh :tangle setup.sh :comments link
echo two
#+end_src
* Tools
#+NAME: helper
#+begin_src python :tangle tools.py :comments link
def helper():
    return 1
#+end_src
";

    #[test]
    fn tangling_with_links_and_back() {
        let tangled = source::tangle(ORG, Path::new("/n/notes.org")).unwrap();
        let setup = &tangled[0];
        assert_eq!(
            setup.content,
            "# [[file:notes.org::*Setup][Setup:1]]\necho one\n# Setup:1 ends here\n\n\
             # [[file:notes.org::*Setup][Setup:2]]\necho two\n# Setup:2 ends here\n"
        );
        let tools = &tangled[1];
        assert!(tools
            .content
            .starts_with("# [[file:notes.org::helper][helper]]\n"));

        // The second block edited in the tangled file goes back to it alone.
        let edited = setup.content.replace("echo two", "echo deux\necho zwei");
        let found = chunks(&edited, &setup.path);
        assert_eq!(found.len(), 2);
        assert_eq!(found[1].org, Path::new("/n/notes.org"));
        let mut org = ORG.to_string();
        for chunk in &found {
            org = apply(&org, chunk).unwrap();
        }
        assert!(org.contains("echo one\n#+end_src"), "{org}");
        assert!(org.contains("echo deux\necho zwei\n#+end_src"), "{org}");

        let named = chunks(&tools.content.replace("return 1", "return 2"), &tools.path);
        let org = apply(&org, &named[0]).unwrap();
        assert!(org.contains("    return 2\n"), "{org}");
    }

    #[test]
    fn code_ending_like_the_marker_does_not_end_the_chunk() {
        let tangled = "# [[file:n.org::*A][A:1]]\necho A:1 ends here\n# A:1 ends here\n";
        let found = chunks(tangled, Path::new("/n/a.sh"));
        assert_eq!(found[0].code, "echo A:1 ends here\n");
    }

    #[test]
    fn blocks_sharing_a_name_go_back_each_to_its_own() {
        let org = "\
#+NAME: part
#+begin_src sh :tangle a.sh :comments link
echo one
#+end_src
#+NAME: part
#+begin_src sh :tangle a.sh :comments link
echo two
#+end_src
";
        let tangled = source::tangle(org, Path::new("/n/n.org")).unwrap();
        assert!(tangled[0].content.contains("[[file:n.org::part][part:2]]"));
        let edited = tangled[0].content.replace("echo two", "echo deux");
        let mut out = org.to_string();
        for chunk in chunks(&edited, &tangled[0].path) {
            out = apply(&out, &chunk).unwrap();
        }
        assert!(
            out.contains("echo one\n") && out.contains("echo deux\n"),
            "{out}"
        );
    }

    #[test]
    fn expanded_references_are_not_written_back() {
        let org = "\
#+NAME: greet
#+begin_src sh
echo hi
#+end_src
#+begin_src sh :tangle a.sh :comments link :noweb yes
<<greet>>
echo done
#+end_src
";
        let tangled = source::tangle(org, Path::new("/n/n.org")).unwrap();
        let found = chunks(&tangled[0].content, &tangled[0].path);
        assert_eq!(found[0].code, "echo hi\necho done\n");
        assert_eq!(apply(org, &found[0]), Err(Unapplied::Noweb));
    }
}
