//! What `#+TAGS:` declares beyond names: the keys of Org's fast tag
//! selection, the rows it lays them out in, and tag groups.
//!
//! ```text
//! #+TAGS: @work(w) @home(h) laptop(l)
//! #+TAGS: { @office(o) @remote(r) }
//! #+TAGS: [ Project : Work Home ]
//! #+TAGS: { Context : @phone @computer }
//! ```
//!
//! Braces make the tags inside exclusive: an entry carries one of them at
//! most. Brackets and braces with a `Name :` first make a group: searching
//! for `Project` finds entries tagged `Work` or `Home` too, and a member that
//! is itself a group brings its own members along, which is how Org builds a
//! hierarchy. A member written `{V@.+}` is a regexp over tag names.

/// A tag offered by the fast selection, with its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagChoice {
    pub name: String,
    pub key: Option<char>,
    /// The row of the popup it is shown on: one per `#+TAGS:` line.
    pub row: usize,
}

/// A group of tags: `[ Name : a b ]`, `{ Name : a b }` or `{ a b }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagGroup {
    /// The group tag, which stands for its members in searches.
    pub name: Option<String>,
    /// The members, a regexp member written `{…}`.
    pub members: Vec<String>,
    /// Braces: one member at most on an entry.
    pub exclusive: bool,
}

/// Everything the `#+TAGS:` lines of a file declare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagSetup {
    pub choices: Vec<TagChoice>,
    pub groups: Vec<TagGroup>,
}

/// `name(k)` into the name and its key.
fn name_and_key(word: &str) -> (String, Option<char>) {
    match word.split_once('(') {
        Some((name, rest)) => {
            let key = rest.trim_end_matches(')').chars().next();
            (name.to_string(), key)
        }
        None => (word.to_string(), None),
    }
}

impl TagSetup {
    /// Reads the values of a file's `#+TAGS:` lines, one row each.
    pub fn parse<'a>(lines: impl IntoIterator<Item = &'a str>) -> TagSetup {
        let mut setup = TagSetup::default();
        for (row, line) in lines.into_iter().enumerate() {
            // The group being read: whether exclusive, its name, its members,
            // and whether the `:` after the name has been seen.
            let mut open: Option<(bool, Vec<String>)> = None;
            let mut group_name: Option<String> = None;
            for word in line.split_whitespace() {
                match word {
                    "{" | "[" => {
                        open = Some((word == "{", Vec::new()));
                        group_name = None;
                    }
                    "}" | "]" => {
                        if let Some((exclusive, members)) = open.take() {
                            setup.groups.push(TagGroup {
                                name: group_name.take(),
                                members,
                                exclusive,
                            });
                        }
                    }
                    ":" => {
                        // What came just before is the group's name.
                        if let Some((_, members)) = &mut open {
                            group_name = members.pop();
                        }
                    }
                    "\\n" => {}
                    word => {
                        let regexp = word.starts_with('{') && word.ends_with('}') && word.len() > 2;
                        let (name, key) = if regexp {
                            (word.to_string(), None)
                        } else {
                            name_and_key(word)
                        };
                        if name.is_empty() {
                            continue;
                        }
                        if let Some((_, members)) = &mut open {
                            members.push(name.clone());
                        }
                        if !regexp && !setup.choices.iter().any(|choice| choice.name == name) {
                            setup.choices.push(TagChoice { name, key, row });
                        }
                    }
                }
            }
            // A group left open at the end of its line still counts.
            if let Some((exclusive, members)) = open {
                setup.groups.push(TagGroup {
                    name: group_name,
                    members,
                    exclusive,
                });
            }
        }
        setup
    }

    /// The `#+TAGS:` of a whole file.
    pub fn of_text(text: &str) -> TagSetup {
        TagSetup::parse(text.lines().filter_map(|line| {
            let rest = line.trim_start().strip_prefix("#+")?;
            let (key, value) = rest.split_once(':')?;
            key.eq_ignore_ascii_case("tags").then_some(value)
        }))
    }

    /// Every choice with a key: its own, or else the first letter of its
    /// name not taken, or else any letter or digit not taken, as Org
    /// assigns them.
    pub fn keyed(&self) -> Vec<(char, &TagChoice)> {
        let mut taken: Vec<char> = self
            .choices
            .iter()
            .filter_map(|choice| choice.key)
            .collect();
        let pool: Vec<char> = ('a'..='z').chain('0'..='9').chain('A'..='Z').collect();
        self.choices
            .iter()
            .filter_map(|choice| {
                let key = choice.key.or_else(|| {
                    let first = choice
                        .name
                        .chars()
                        .find(|c| c.is_ascii_alphanumeric())
                        .map(|c| c.to_ascii_lowercase());
                    let key = first
                        .filter(|c| !taken.contains(c))
                        .or_else(|| pool.iter().copied().find(|c| !taken.contains(c)))?;
                    taken.push(key);
                    Some(key)
                })?;
                Some((key, choice))
            })
            .collect()
    }

    /// The tags that exclude `tag`: the others of its exclusive groups.
    pub fn excluded_by(&self, tag: &str) -> Vec<&str> {
        self.groups
            .iter()
            .filter(|group| group.exclusive && group.members.iter().any(|m| m == tag))
            .flat_map(|group| group.members.iter())
            .map(String::as_str)
            .filter(|member| *member != tag)
            .collect()
    }

    /// What searching for `tag` looks for: the tag, and when it names a
    /// group its members, their own members in turn, and the regexps among
    /// them.
    pub fn expand(&self, tag: &str) -> Expansion {
        let mut expansion = Expansion {
            tags: vec![tag.to_string()],
            patterns: Vec::new(),
        };
        let mut pending = vec![tag.to_string()];
        while let Some(name) = pending.pop() {
            for group in self
                .groups
                .iter()
                .filter(|group| group.name.as_deref() == Some(name.as_str()))
            {
                for member in &group.members {
                    if member.starts_with('{') && member.ends_with('}') {
                        let pattern = member[1..member.len() - 1].to_string();
                        if !expansion.patterns.contains(&pattern) {
                            expansion.patterns.push(pattern);
                        }
                    } else if !expansion.tags.contains(member) {
                        // Seen once: a group that contains itself stops here.
                        expansion.tags.push(member.clone());
                        pending.push(member.clone());
                    }
                }
            }
        }
        expansion
    }

    /// Whether `tag` names a group.
    pub fn is_group(&self, tag: &str) -> bool {
        self.groups
            .iter()
            .any(|group| group.name.as_deref() == Some(tag))
    }

    /// Adds another file's declarations, as the agenda does across files.
    pub fn merge(&mut self, other: &TagSetup) {
        for choice in &other.choices {
            if !self.choices.iter().any(|own| own.name == choice.name) {
                self.choices.push(choice.clone());
            }
        }
        for group in &other.groups {
            if !self.groups.contains(group) {
                self.groups.push(group.clone());
            }
        }
    }
}

/// A tag with what it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub tags: Vec<String>,
    /// Regexps over tag names.
    pub patterns: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "\
#+TAGS: @work(w) @home(h) laptop
#+TAGS: { @office(o) @remote }
#+TAGS: [ Project : Work Home ]
#+TAGS: [ Work : Client Internal {W@.+} ]
";

    #[test]
    fn choices_keys_rows_and_groups() {
        let setup = TagSetup::of_text(TEXT);
        let names: Vec<&str> = setup.choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "@work", "@home", "laptop", "@office", "@remote", "Project", "Work", "Home",
                "Client", "Internal"
            ]
        );
        assert_eq!(setup.choices[3].row, 1);
        let keyed = setup.keyed();
        assert_eq!(keyed[0].0, 'w');
        assert_eq!(keyed[2], ('l', &setup.choices[2]));
        // `@remote` would take `r`… which is free.
        assert_eq!(keyed[4].0, 'r');
        // `Work` finds `w` taken and gets the first free letter.
        let work = keyed.iter().find(|(_, c)| c.name == "Work").unwrap().0;
        assert_eq!(work, 'a');

        assert_eq!(setup.excluded_by("@office"), ["@remote"]);
        assert!(setup.excluded_by("@work").is_empty());

        let project = setup.expand("Project");
        assert_eq!(
            project.tags,
            ["Project", "Work", "Home", "Client", "Internal"]
        );
        assert_eq!(project.patterns, ["W@.+"]);
        assert!(setup.is_group("Work") && !setup.is_group("Home"));
    }

    #[test]
    fn a_group_inside_itself_does_not_loop() {
        let setup = TagSetup::parse(["[ A : B ]", "[ B : A C ]"]);
        assert_eq!(setup.expand("A").tags, ["A", "B", "C"]);
    }
}
