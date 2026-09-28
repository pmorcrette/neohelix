//! Publishing, Org's `org-publish`: a project's Org files exported into a
//! publishing directory that mirrors the base directory, its attachments
//! (pictures, style sheets, …) copied along, a sitemap written if asked.
//!
//! Only what changed is rebuilt: a file is exported again when it is
//! newer than what it was exported to, or when forced. Mirroring the base
//! directory is what keeps the links right: a link between two notes is
//! the same relative path between their exported files.

use std::path::{Path, PathBuf};

use crate::export::{export_with, Backend, Resolve};

/// A publishing project, as configured.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    /// Where the Org files are.
    pub base: PathBuf,
    /// Where they are published to.
    pub publishing: PathBuf,
    pub backend: Backend,
    /// Into subdirectories too.
    pub recursive: bool,
    /// Files whose path, relative to the base, matches are left out.
    pub exclude: Option<regex::Regex>,
    /// The extensions of the files copied as they are.
    pub attachments: Vec<String>,
    /// A sitemap of the project, written into the base as an Org file and
    /// published with the rest.
    pub sitemap: Option<Sitemap>,
    /// The body only, for pages another tool wraps.
    pub body_only: bool,
}

#[derive(Debug, Clone)]
pub struct Sitemap {
    /// Its file name in the base directory.
    pub file: String,
    pub title: String,
}

/// What a publishing run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// The files exported, as published.
    pub published: Vec<PathBuf>,
    /// The attachments copied.
    pub copied: Vec<PathBuf>,
    /// The files already up to date.
    pub unchanged: usize,
    pub warnings: Vec<String>,
}

/// The files under `base` a project covers: its Org files and its
/// attachments, relative to `base`, in order. Hidden files and
/// directories, and the publishing directory, are not looked into.
fn files(project: &Project) -> std::io::Result<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut org = Vec::new();
    let mut attachments = Vec::new();
    let mut pending = vec![project.base.clone()];
    while let Some(dir) = pending.pop() {
        let mut listing: Vec<_> = std::fs::read_dir(&dir)?.collect::<Result<_, _>>()?;
        listing.sort_by_key(|entry| entry.file_name());
        for entry in listing {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.file_type()?.is_dir() {
                if project.recursive && path != project.publishing {
                    pending.push(path);
                }
                continue;
            }
            let relative = path
                .strip_prefix(&project.base)
                .unwrap_or(&path)
                .to_path_buf();
            let shown = relative.to_string_lossy().replace('\\', "/");
            if project
                .exclude
                .as_ref()
                .is_some_and(|exclude| exclude.is_match(&shown))
            {
                continue;
            }
            let extension = path
                .extension()
                .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if crate::parser::is_org_file(&path) {
                org.push(relative);
            } else if project
                .attachments
                .iter()
                .any(|wanted| wanted.eq_ignore_ascii_case(&extension))
            {
                attachments.push(relative);
            }
        }
    }
    org.sort();
    attachments.sort();
    Ok((org, attachments))
}

/// Whether `target` is missing or older than `source`.
fn stale(source: &Path, target: &Path) -> bool {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|meta| meta.modified());
    match (modified(source), modified(target)) {
        (Ok(source), Ok(target)) => source > target,
        _ => true,
    }
}

/// A file's title: its `#+TITLE:`, or its name.
fn title_of(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let rest = line.trim_start().strip_prefix("#+")?;
                let (key, value) = rest.split_once(':')?;
                key.eq_ignore_ascii_case("title")
                    .then(|| value.trim().to_string())
            })
        })
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

/// The sitemap's text: a link to every other Org file, under an item for
/// each directory, as Org's tree style has it.
fn sitemap_text(project: &Project, sitemap: &Sitemap, org: &[PathBuf]) -> String {
    let mut out = format!("#+TITLE: {}\n\n", sitemap.title);
    let mut listed: Vec<PathBuf> = Vec::new();
    for relative in org {
        if relative == Path::new(&sitemap.file) {
            continue;
        }
        // The directories on the way, each listed once.
        let mut dir = PathBuf::new();
        let parents: Vec<_> = relative
            .parent()
            .into_iter()
            .flat_map(Path::components)
            .collect();
        for (depth, part) in parents.iter().enumerate() {
            dir.push(part);
            if !listed.contains(&dir) {
                listed.push(dir.clone());
                out.push_str(&format!(
                    "{}- {}\n",
                    "  ".repeat(depth),
                    part.as_os_str().to_string_lossy()
                ));
            }
        }
        let shown = relative.to_string_lossy().replace('\\', "/");
        out.push_str(&format!(
            "{}- [[file:{shown}][{}]]\n",
            "  ".repeat(parents.len()),
            title_of(&project.base.join(relative))
        ));
    }
    out
}

/// Publishes `project`: every Org file that changed, and every attachment,
/// unless `force` says every one.
pub fn publish(project: &Project, resolve: &dyn Resolve, force: bool) -> std::io::Result<Report> {
    let mut report = Report::default();
    let (mut org, attachments) = files(project)?;

    if let Some(sitemap) = &project.sitemap {
        let path = project.base.join(&sitemap.file);
        let text = sitemap_text(project, sitemap, &org);
        // Written only when it says something new, so that it is not
        // republished every time.
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, &text)?;
        }
        let relative = PathBuf::from(&sitemap.file);
        if !org.contains(&relative) {
            org.push(relative);
        }
    }

    for relative in &org {
        let source = project.base.join(relative);
        let target = project
            .publishing
            .join(relative)
            .with_extension(project.backend.extension());
        if !force && !stale(&source, &target) {
            report.unchanged += 1;
            continue;
        }
        let text = std::fs::read_to_string(&source)?;
        let exported = export_with(&text, &source, project.backend, resolve, project.body_only);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, exported.bytes())?;
        report.warnings.extend(
            exported
                .warnings
                .into_iter()
                .map(|warning| format!("{}: {warning}", relative.display())),
        );
        report.published.push(target);
    }

    for relative in &attachments {
        let source = project.base.join(relative);
        let target = project.publishing.join(relative);
        if !force && !stale(&source, &target) {
            report.unchanged += 1;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&source, &target)?;
        report.copied.push(target);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::NoResolve;

    fn project(base: &Path, out: &Path) -> Project {
        Project {
            name: "site".into(),
            base: base.to_path_buf(),
            publishing: out.to_path_buf(),
            backend: Backend::Html,
            recursive: true,
            exclude: Some(regex::Regex::new("^drafts/").unwrap()),
            attachments: vec!["png".into(), "css".into()],
            sitemap: Some(Sitemap {
                file: "sitemap.org".into(),
                title: "Plan".into(),
            }),
            body_only: false,
        }
    }

    #[test]
    fn publishes_what_changed_mirrors_and_copies() {
        let base = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let b = base.path();
        std::fs::create_dir_all(b.join("posts")).unwrap();
        std::fs::create_dir_all(b.join("drafts")).unwrap();
        std::fs::write(
            b.join("index.org"),
            "#+TITLE: Home\nSee [[file:posts/one.org][one]].\n",
        )
        .unwrap();
        std::fs::write(b.join("posts/one.org"), "#+TITLE: First post\nText.\n").unwrap();
        std::fs::write(b.join("drafts/wip.org"), "Not yet.\n").unwrap();
        std::fs::write(b.join("posts/pic.png"), [0u8, 1, 2]).unwrap();
        std::fs::write(b.join("notes.txt"), "ignored").unwrap();

        let project = project(b, out.path());
        let report = publish(&project, &NoResolve, false).unwrap();
        let o = out.path();
        assert!(o.join("index.html").exists());
        assert!(o.join("posts/one.html").exists());
        assert!(o.join("posts/pic.png").exists());
        assert!(o.join("sitemap.html").exists());
        assert!(!o.join("drafts/wip.html").exists());
        assert!(!o.join("notes.txt").exists());
        assert_eq!((report.published.len(), report.copied.len()), (3, 1));
        let index = std::fs::read_to_string(o.join("index.html")).unwrap();
        assert!(index.contains("href=\"posts/one.html\""), "{index}");
        let sitemap = std::fs::read_to_string(b.join("sitemap.org")).unwrap();
        assert_eq!(
            sitemap,
            "#+TITLE: Plan\n\n- [[file:index.org][Home]]\n- posts\n  - [[file:posts/one.org][First post]]\n"
        );

        // Nothing changed: nothing is rebuilt, the sitemap not rewritten.
        let again = publish(&project, &NoResolve, false).unwrap();
        assert!(
            again.published.is_empty() && again.copied.is_empty(),
            "{again:?}"
        );
        assert_eq!(again.unchanged, 4);

        // One file changed: it alone is published again.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(b.join("posts/one.org"), "#+TITLE: First post\nMore.\n").unwrap();
        let third = publish(&project, &NoResolve, false).unwrap();
        assert_eq!(third.published, vec![o.join("posts/one.html")]);

        let forced = publish(&project, &NoResolve, true).unwrap();
        assert_eq!(forced.published.len(), 3);
    }
}
