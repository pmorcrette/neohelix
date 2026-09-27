//! Running Magit's git commands from the editor.
//!
//! Everything here happens off the editor's thread: the command runs on a
//! blocking task and the result comes back as a compositor callback, so a
//! slow push never freezes Helix.

use std::path::PathBuf;

use helix_magit::command::Seed;
use helix_magit::command::{self, GitCommand, GitOutput};
use helix_magit::{Ask, AskKind, Plan, Requirement};
use helix_view::editor::PendingCommit;
use helix_view::Editor;

use crate::compositor::{Compositor, Context};
use crate::job::Callback;
use crate::ui::confirm::Confirm;
use crate::ui::diff_view::DiffView;

/// Starts a plan, asking for whatever it still needs first.
pub fn execute(compositor: &mut Compositor, cx: &mut Context, plan: Plan, workdir: PathBuf) {
    match plan.requirement.clone() {
        Requirement::None => {
            if plan.destructive {
                confirm_then_run(compositor, plan, workdir);
            } else {
                run(cx, plan, workdir);
            }
        }
        Requirement::Ask(asks) => ask_next(compositor, cx.editor, plan, workdir, asks, Vec::new()),
        Requirement::CommitMessage { seed } => {
            // The message must be seen.
            step_aside(compositor, cx.editor);
            compose(cx.editor, plan, workdir, seed)
        }
        Requirement::TodoList => start_rebase(compositor, cx, plan, workdir),
    }
}

// ── Interactive rebase ──────────────────────────────────────────────────
//
// See `helix_magit::rebase` for why git runs twice rather than calling
// back into Helix for the todo-list.

/// Whether the rebase's command line already names where to start from.
fn has_base(args: &[String]) -> bool {
    // `rebase --interactive [--flags…] [base]`: the first two are fixed.
    // `--root` rebases from the very first commit, and is a base too.
    args.iter()
        .skip(2)
        .any(|arg| !arg.starts_with('-') || arg == "--root")
}

/// Asks where to rebase from, unless the command line says already.
fn start_rebase(compositor: &mut Compositor, cx: &mut Context, plan: Plan, workdir: PathBuf) {
    if has_base(&plan.args) {
        capture_todo(cx, plan, workdir);
        return;
    }
    let prompt = crate::ui::Prompt::new(
        "Rebase from (commit, empty for the upstream): ".into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let mut plan = plan.clone();
            if !input.trim().is_empty() {
                plan.args.push(input.trim().to_string());
            }
            capture_todo(cx, plan, workdir.clone());
        },
    );
    compositor.push(Box::new(prompt));
}

/// First run: git writes its todo-list, which is kept and opened for
/// editing while the rebase itself is cancelled.
fn capture_todo(cx: &mut Context, plan: Plan, workdir: PathBuf) {
    use helix_magit::rebase;

    let Some(git_dir) = helix_magit::status::git_dir(&workdir) else {
        cx.editor.set_error("Not in a git repository");
        return;
    };
    let dir = git_dir.join("helix");
    // Named as git names it, so the buffer gets the git-rebase language.
    let todo_path = dir.join("git-rebase-todo");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        cx.editor
            .set_error(format!("could not prepare the todo-list: {err}"));
        return;
    }
    let _ = std::fs::remove_file(&todo_path);

    let line = plan.command_line();
    let command = rebase::capture_command(&workdir, plan.args.clone(), &todo_path);
    cx.editor.set_status(format!("Preparing {line}…"));

    cx.jobs.callback(async move {
        let head = {
            let workdir = workdir.clone();
            tokio::task::spawn_blocking(move || rebase::head(&workdir))
                .await
                .ok()
                .flatten()
        };
        let outcome = tokio::task::spawn_blocking(move || command.run()).await;

        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| {
                let output = match outcome {
                    Ok(Ok(output)) => output,
                    Ok(Err(err)) => return editor.set_error(format!("{line}: {err}")),
                    Err(err) => return editor.set_error(format!("{line}: {err}")),
                };
                let todo = std::fs::read_to_string(&todo_path).unwrap_or_default();
                if output.success || !rebase::captured(&output.stderr) || todo.is_empty() {
                    // git never asked for the list: it refused, or had
                    // nothing to rebase. Either way its own words say why.
                    return report(editor, compositor, &line, output);
                }

                let with_help = format!("{}\n{}", todo.trim_end(), rebase::HELP);
                if let Err(err) = std::fs::write(&todo_path, with_help) {
                    return editor.set_error(format!("could not write the todo-list: {err}"));
                }
                if let Err(err) =
                    editor.open(&todo_path, helix_view::editor::Action::HorizontalSplit)
                {
                    return editor.set_error(format!("could not open the todo-list: {err}"));
                }
                // The list must be seen.
                step_aside(compositor, editor);
                editor.pending_rebase = Some(helix_view::editor::PendingRebase {
                    todo_path,
                    args: plan.args,
                    working_directory: workdir,
                    head,
                });
                editor.set_status("Edit the todo-list, then `:w` to rebase (`:q!` cancels)");
            },
        )))
    });
}

/// Second run, when the todo-list is written: the edited list replaces
/// git's and the rebase goes ahead.
///
/// Returns whether the write belonged to a pending rebase.
pub fn rebase_if_written(editor: &mut Editor, path: &std::path::Path) -> bool {
    use helix_magit::rebase;

    let Some(pending) = editor.pending_rebase.take() else {
        return false;
    };
    if path != pending.todo_path {
        editor.pending_rebase = Some(pending);
        return false;
    }

    let todo = std::fs::read_to_string(&pending.todo_path).unwrap_or_default();
    if rebase::is_empty(&todo) {
        close_message_buffer(editor, &pending.todo_path);
        editor.set_status("Rebase cancelled: the todo-list is empty");
        return true;
    }
    if rebase::head(&pending.working_directory) != pending.head {
        editor.set_error("HEAD moved since the todo-list was made; start the rebase again");
        return true;
    }

    let (prepared, rewords) = rebase::prepare(&todo);
    let run_path = pending.todo_path.with_file_name("git-rebase-todo.run");
    if let Err(err) = std::fs::write(&run_path, prepared) {
        editor.set_error(format!("could not write the todo-list: {err}"));
        return true;
    }

    let line = format!("git {}", pending.args.join(" "));
    let command = rebase::install_command(&pending.working_directory, pending.args, &run_path);
    let todo_path = pending.todo_path;
    editor.set_status(format!("Running {line}…"));

    tokio::task::spawn_blocking(move || {
        let outcome = command.run();
        crate::job::dispatch_blocking(move |editor, compositor| match outcome {
            Ok(output) => {
                close_message_buffer(editor, &todo_path);
                report(editor, compositor, &line, output);
                if rewords > 0 {
                    editor.set_status(format!(
                        "{rewords} reword(s) run as edit: amend the message (c a), then continue (r c)"
                    ));
                }
            }
            Err(err) => editor.set_error(format!("{line}: {err}")),
        });
    });
    true
}

/// `:rebase-todo <action>`: sets the action of the selected lines of a
/// todo-list, or moves them with `up` and `down`.
pub fn rebase_todo(editor: &mut Editor, action: &str) -> Result<(), String> {
    use helix_core::{Selection, Transaction};

    let (view, doc) = helix_view::current!(editor);
    let text = doc.text().clone();
    let slice = text.slice(..);
    let primary = doc.selection(view.id).primary();
    let (first, last) = primary.line_range(slice);
    let line_text = |line: usize| -> String {
        let line = text.line(line).to_string();
        line.trim_end_matches(['\n', '\r']).to_string()
    };

    let (from, to, replacement, shift): (usize, usize, Vec<String>, isize) = match action {
        "up" | "down" => {
            let up = action == "up";
            if (up && first == 0) || (!up && last + 1 >= text.len_lines().saturating_sub(1)) {
                return Err("Nothing to move past".to_string());
            }
            let block: Vec<String> = (first..=last).map(line_text).collect();
            if up {
                let above = line_text(first - 1);
                let shift = -(text.line(first - 1).len_chars() as isize);
                let mut lines = block;
                lines.push(above);
                (first - 1, last, lines, shift)
            } else {
                let below = line_text(last + 1);
                let shift = text.line(last + 1).len_chars() as isize;
                let mut lines = vec![below];
                lines.extend(block);
                (first, last + 1, lines, shift)
            }
        }
        // A line of its own below the selection: `exec <command>` or `break`.
        action if action == "break" || action.starts_with("exec ") => {
            let below = line_text(last);
            (last, last, vec![below, action.to_string()], 0)
        }
        action => {
            let mut changed = false;
            let lines = (first..=last)
                .map(|line| {
                    let old = line_text(line);
                    match helix_magit::rebase::set_action(&old, action) {
                        Some(new) => {
                            changed = true;
                            new
                        }
                        None => old,
                    }
                })
                .collect();
            if !changed {
                return Err(format!(
                    "`{action}` is not pick, reword, edit, squash, fixup, drop, up or down, \
                     or no selected line is a commit"
                ));
            }
            (first, last, lines, 0)
        }
    };

    let start = text.line_to_char(from);
    let end = text.line_to_char(to) + line_text(to).chars().count();
    let transaction = Transaction::change(
        &text,
        [(start, end, Some(replacement.join("\n").into()))].into_iter(),
    );
    let moved = |pos: usize| (pos as isize + shift).max(0) as usize;
    let transaction = if shift != 0 {
        transaction.with_selection(Selection::single(
            moved(primary.anchor),
            moved(primary.head),
        ))
    } else {
        transaction
    };
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);
    Ok(())
}

/// Puts a destructive plan behind a single-key confirmation.
fn confirm_then_run(compositor: &mut Compositor, plan: Plan, workdir: PathBuf) {
    if plan.args.first().map(String::as_str) == Some("clean") && plan.special.is_none() {
        return confirm_clean(compositor, plan, workdir);
    }
    let mut question = format!("{}? (y/N)", plan.summary);
    // Say what is aborted, or that nothing is.
    if plan.special == Some(helix_magit::Special::Abort) {
        let state = helix_magit::status::git_dir(&workdir)
            .and_then(|dir| helix_magit::status::in_progress(&dir, &|_| None));
        match state {
            Some(state) => question = format!("Abort: {}? (y/N)", state.description),
            None => {
                tokio::spawn(crate::job::dispatch(|editor, _| {
                    editor.set_error("Nothing is in progress");
                }));
                return;
            }
        }
    }
    // What such commands move away from stays in the reflog: say where.
    let recoverable = matches!(
        plan.args.first().map(String::as_str),
        Some("reset" | "rebase" | "branch" | "cherry-pick" | "revert" | "merge" | "am")
    );
    let detail = if recoverable {
        format!(
            "{}   (commits left behind stay in the reflog: L r)",
            plan.command_line()
        )
    } else {
        plan.command_line()
    };

    compositor.push(Box::new(Confirm::new(question, detail, move |cx| {
        run(cx, plan, workdir);
    })));
}

/// `git clean`, confirmed by naming what it would remove — found by
/// asking git the same question with `-n` — or not asked at all when that
/// is nothing.
fn confirm_clean(compositor: &mut Compositor, plan: Plan, workdir: PathBuf) {
    let paths = match command::clean_preview(&workdir, &plan.args) {
        Ok(paths) => paths,
        Err(err) => {
            let line = plan.command_line();
            compositor.push(Box::new(Confirm::new(
                format!("{line} cannot run"),
                err,
                |_| {},
            )));
            return;
        }
    };
    if paths.is_empty() {
        // Said on the next turn of the loop: this runs inside one.
        tokio::spawn(crate::job::dispatch(|editor, _| {
            editor.set_status("Nothing to remove");
        }));
        return;
    }
    let count = paths.len();
    let question = format!(
        "{}: {count} path{}? (y/N)",
        plan.summary,
        if count == 1 { "" } else { "s" }
    );
    compositor.push(Box::new(Confirm::new(
        question,
        name_paths(&paths, 100),
        move |cx| run(cx, plan, workdir),
    )));
}

/// `a.txt, build/, … and 12 more`, within about `width` characters.
fn name_paths(paths: &[String], width: usize) -> String {
    let mut text = String::new();
    for (index, path) in paths.iter().enumerate() {
        let rest = paths.len() - index;
        let more = format!(", … and {rest} more");
        if !text.is_empty() && text.len() + path.len() + 2 + more.len() > width {
            text.push_str(&more);
            return text;
        }
        if !text.is_empty() {
            text.push_str(", ");
        }
        text.push_str(path);
    }
    text
}

/// Asks the plan's questions one after the other, then runs it with the
/// answers. A question the menu's target already answered is skipped.
fn ask_next(
    compositor: &mut Compositor,
    editor: &mut Editor,
    plan: Plan,
    workdir: PathBuf,
    asks: Vec<Ask>,
    mut answers: Vec<String>,
) {
    // A suggested preset — the file an ignore pattern starts from, since
    // `junk.log` is as likely to become `*.log` — is asked with it filled
    // in; any other preset is the answer.
    // So is one the repository answers: the push-remote, the branch.
    while let Some(answer) = asks.get(answers.len()).and_then(|ask| {
        ask.preset
            .clone()
            .filter(|_| !ask.suggested)
            .or_else(|| ask.source.and_then(|source| source.read(&workdir)))
    }) {
        answers.push(answer);
    }
    let Some(ask) = asks.get(answers.len()).cloned() else {
        let plan = plan.answered(&answers);
        // Some plans go on to a message once they know which commit.
        if let Requirement::CommitMessage { seed } = plan.requirement.clone() {
            step_aside(compositor, editor);
            return compose(editor, plan, workdir, seed);
        }
        if plan.destructive {
            confirm_then_run(compositor, plan, workdir);
        } else {
            spawn_run(plan, workdir, false);
        }
        return;
    };

    let names = helix_magit::refs::names(&workdir, ask.kind);
    let label = match (ask.optional, ask.fallback, ask.source) {
        (_, _, Some(source)) => format!("{}: ", source.label(&workdir)),
        (_, Some(fallback), _) => format!("{} (empty for {fallback}): ", ask.label),
        (true, None, _) => format!("{}: ", ask.label),
        (false, None, _) => format!("{} (required): ", ask.label),
    };
    let kind = ask.kind;
    let preset = ask.preset.clone();
    let prompt = crate::ui::Prompt::new(
        label.into(),
        None,
        move |editor, input| match kind {
            AskKind::Path => crate::ui::completers::filename(editor, input),
            _ => names
                .iter()
                .filter(|name| name.contains(input))
                .map(|name| (0.., name.clone().into()))
                .collect(),
        },
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let input = input.trim().to_string();
            if let Some(refusal) = ask.refuse(&input) {
                cx.editor.set_error(refusal);
                return;
            }
            // Kept, so the push-remote is asked once per branch.
            if let Some(source) = ask.source {
                if let Err(err) = source.remember(&workdir, &input) {
                    cx.editor.set_error(err);
                    return;
                }
            }
            let mut answers = answers.clone();
            answers.push(ask.answer(&input));
            let (plan, workdir, asks) = (plan.clone(), workdir.clone(), asks.clone());
            cx.jobs.callback(async move {
                Ok(Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        ask_next(compositor, editor, plan, workdir, asks, answers);
                    },
                )))
            });
        },
    );
    let prompt = match preset {
        Some(preset) => prompt.with_line(preset, editor),
        None => prompt,
    };
    compositor.push(Box::new(prompt));
}

/// Asks for a range, then shows who contributed how many commits to it.
pub fn shortlog_prompt(workdir: PathBuf, args: Vec<String>) -> crate::ui::Prompt {
    crate::ui::Prompt::new(
        "Shortlog of (revision or range, empty for HEAD): ".into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let range = match input.trim() {
                "" => "HEAD".to_string(),
                range => range.to_string(),
            };
            if let Err(err) = helix_magit::log::LogFilter::valid_range(&range) {
                cx.editor.set_error(err);
                return;
            }
            // `--author=` and `--grep=` from the log menu limit it too; a
            // path goes after `--`.
            let filter = helix_magit::log::LogFilter::from_args(&args);
            let mut command: Vec<String> = ["shortlog", "--summary", "--numbered", "--email"]
                .iter()
                .map(|arg| arg.to_string())
                .collect();
            command.extend(filter.author.map(|author| format!("--author={author}")));
            command.extend(filter.grep.map(|grep| format!("--grep={grep}")));
            command.push(range.clone());
            command.push("--".into());
            command.extend(filter.path.map(|path| path.display().to_string()));
            match GitCommand::new(&workdir, command).run() {
                Ok(output) if output.success => {
                    let text = format!("Shortlog of {range}\n\n{}", output.stdout);
                    cx.editor
                        .new_scratch_with_text(helix_view::editor::Action::Replace, &text);
                }
                Ok(output) => cx.editor.set_error(output.summary()),
                Err(err) => cx.editor.set_error(err.to_string()),
            }
        },
    )
}

// ── Conflicts ───────────────────────────────────────────────────────────

/// Opens a conflicted file on its first conflict.
pub fn edit_conflict(editor: &mut Editor, path: &std::path::Path) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let line = helix_magit::conflict::regions(&text)
        .first()
        .map_or(0, |region| region.start);
    if crate::roam::open_at(editor, path, line) {
        let count = helix_magit::conflict::regions(&text).len();
        editor.set_status(format!(
            "{count} conflict(s): ]m / [m move between them, \
             :conflict-take ours|theirs|base|both resolves the one under the cursor"
        ));
    }
}

/// Shows one side of a conflicted file — ours, theirs or the base — in a
/// scratch buffer beside it, highlighted as the file is.
pub fn show_conflict_side(
    editor: &mut Editor,
    workdir: &std::path::Path,
    path: &std::path::Path,
    side: helix_magit::conflict::Side,
) {
    use helix_magit::conflict::Side;
    let name = match side {
        Side::Ours => "ours",
        Side::Theirs => "theirs",
        _ => "base",
    };
    let Some(content) = helix_magit::conflict::stage_content(workdir, path, side) else {
        editor.set_error(format!("{} has no {name} version", path.display()));
        return;
    };
    let doc_id = editor.new_scratch_with_text(helix_view::editor::Action::VerticalSplit, &content);
    let loader = editor.syn_loader.load();
    if let Some(language) = loader.language_for_filename(path) {
        let config = loader.language(language).config().clone();
        helix_view::doc_mut!(editor, &doc_id).set_language(Some(config), &loader);
    }
    editor.set_status(format!("{}: the {name} version", path.display()));
}

/// `]m` / `[m`: the next or previous conflict in the buffer.
pub fn goto_conflict(editor: &mut Editor, forward: bool) {
    let (view, doc) = helix_view::current!(editor);
    let text = doc.text().clone();
    let regions = helix_magit::conflict::regions(&text.to_string());
    let cursor = doc.selection(view.id).primary().cursor(text.slice(..));
    let line = text.char_to_line(cursor);
    let target = if forward {
        regions.iter().find(|region| region.start > line)
    } else {
        regions.iter().rev().find(|region| region.end <= line)
    };
    let Some(target) = target else {
        editor.set_status(if regions.is_empty() {
            "No conflict in this buffer"
        } else {
            "No more conflicts that way"
        });
        return;
    };
    let at = text.line_to_char(target.start);
    doc.set_selection(view.id, helix_core::Selection::point(at));
    helix_view::align_view(doc, view, helix_view::Align::Center);
}

/// `:conflict-take <side>`: replaces the conflict under the cursor with
/// one side of it, or with both.
pub fn conflict_take(editor: &mut Editor, side: &str) -> Result<(), String> {
    use helix_core::{Selection, Transaction};
    use helix_magit::conflict::{kept, region_at, regions, Side};

    let side =
        Side::parse(side).ok_or_else(|| format!("`{side}` is not ours, theirs, base or both"))?;
    let (view, doc) = helix_view::current!(editor);
    let text = doc.text().clone();
    let cursor = doc.selection(view.id).primary().cursor(text.slice(..));
    let line = text.char_to_line(cursor);
    let found = regions(&text.to_string());
    let region = region_at(&found, line).ok_or("The cursor is not in a conflict")?;
    let lines = kept(region, side).ok_or(
        "This conflict has no base section: rewrite the file with the base shown (e then 3)",
    )?;

    let start = text.line_to_char(region.start);
    let end = text.line_to_char(region.end.min(text.len_lines()));
    let mut replacement = lines.join("\n");
    if !lines.is_empty() {
        replacement.push('\n');
    }
    let transaction =
        Transaction::change(&text, [(start, end, Some(replacement.into()))].into_iter())
            .with_selection(Selection::point(start));
    doc.apply(&transaction, view.id);
    doc.append_changes_to_history(view);

    let left = regions(&doc.text().to_string()).len();
    editor.set_status(if left == 0 {
        "No conflict left: write the file, then mark it resolved (s in the status)".to_string()
    } else {
        format!("{left} conflict(s) left")
    });
    Ok(())
}

/// Runs the plan and reports what git said.
fn run(cx: &mut Context, plan: Plan, workdir: PathBuf) {
    cx.editor
        .set_status(format!("Running {}…", plan.command_line()));
    // A command that can lose work saves it to the wip refs first, when
    // they are on.
    let wip = plan.destructive && cx.editor.config().magit.wip;
    spawn_run(plan, workdir, wip);
}

/// The running half of [`run`], for callers without a context. With `wip`,
/// the uncommitted work is saved to the wip refs before anything runs.
fn spawn_run(plan: Plan, workdir: PathBuf, wip: bool) {
    let line = plan.command_line();
    tokio::task::spawn_blocking(move || {
        if wip {
            let message = format!("before {}", plan.summary);
            if let Err(err) = helix_magit::wip::save(&workdir, &message, None) {
                // Not saved: do not go ahead as if it had been.
                crate::job::dispatch_blocking(move |editor, _| {
                    editor.set_error(format!("wip save failed, nothing was run: {err}"));
                });
                return;
            }
        }
        let outcome = command::run_plan(&workdir, &plan);
        crate::job::dispatch_blocking(move |editor, compositor| match outcome {
            Ok(output) => {
                let made = output
                    .success
                    .then(|| new_repository(&workdir, &plan))
                    .flatten();
                report(editor, compositor, &line, output);
                // A repository just cloned or made: its status, at once.
                if let Some(path) = made {
                    match DiffView::new(&path) {
                        Ok(view) => {
                            close_views(compositor);
                            compositor.push(Box::new(view));
                        }
                        Err(err) => editor.set_error(err.to_string()),
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                editor.set_error("git was not found on PATH".to_string());
            }
            Err(err) => editor.set_error(format!("{line}: {err}")),
        });
    });
}

/// After a file is written: saves the repository's uncommitted work for that
/// file to the wip refs, when they are on. Off the editor's thread; a
/// failure is reported, never in the way of the write.
pub fn wip_after_save(editor: &Editor, path: &std::path::Path) {
    if !editor.config().magit.wip {
        return;
    }
    // Git's own files — a commit message, a rebase list — are not work.
    if path.components().any(|part| part.as_os_str() == ".git") {
        return;
    }
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let Ok(repository) = helix_magit::Repository::discover(path.parent().unwrap_or(&path))
        else {
            return;
        };
        let workdir = repository.workdir().to_path_buf();
        let Some(relative) = crate::ui::blame_view::relative_to(&workdir, &path) else {
            return;
        };
        let message = format!("autosave {}", relative.display());
        if let Err(err) = helix_magit::wip::save(&workdir, &message, Some(&[relative])) {
            crate::job::dispatch_blocking(move |editor, _| {
                editor.set_error(format!("wip save failed: {err}"));
            });
        }
    });
}

/// What to say of the documents [`refresh_documents`] could not reload.
pub fn stale_message(stale: &[String]) -> Option<String> {
    (!stale.is_empty()).then(|| format!("Not reloaded from disk: {}", stale.join(", ")))
}

/// Brings open documents back in step with their files after git changed
/// them — a checkout, a reset, a discard, a stash. A document with no
/// unsaved changes is reloaded; one with unsaved changes is left alone,
/// and its name returned, so the caller can say that it is now behind its
/// file rather than overwrite what was typed. A document whose file git
/// removed is named too.
pub fn refresh_documents(editor: &mut Editor) -> Vec<String> {
    use helix_view::DocumentId;

    let scrolloff = editor.config().scrolloff;
    let focus = editor.tree.focus;
    let mut stale = Vec::new();
    let mut reload: Vec<DocumentId> = Vec::new();
    for doc in editor.documents() {
        let Some(path) = doc.path() else {
            continue;
        };
        match std::fs::read(path) {
            Ok(bytes) => {
                // Compared as text the way the buffer holds it; a file that
                // is not UTF-8 is left to Helix's own reload.
                let Ok(disk) = String::from_utf8(bytes) else {
                    continue;
                };
                if *doc.text() == disk.as_str() {
                    continue;
                }
                if doc.is_modified() {
                    stale.push(format!("{} (unsaved changes)", doc.display_name()));
                } else {
                    reload.push(doc.id());
                }
            }
            // Gone from disk. Only worth saying of a buffer that holds a
            // file as it was: an unmodified one with something in it. A
            // buffer never saved has no file either (and holds a lone line
            // ending), and a modified one keeps its text whatever happened
            // on disk.
            Err(err)
                if err.kind() == std::io::ErrorKind::NotFound
                    && !doc.is_modified()
                    && doc.text().chars().any(|c| c != '\n' && c != '\r') =>
            {
                stale.push(format!("{} (deleted)", doc.display_name()));
            }
            Err(_) => {}
        }
    }

    for doc_id in reload {
        let Some(doc) = editor.documents.get_mut(&doc_id) else {
            continue;
        };
        let mut view_ids: Vec<_> = doc.selections().keys().cloned().collect();
        if view_ids.is_empty() {
            doc.ensure_view_init(focus);
            view_ids.push(focus);
        }
        let trust_full = editor
            .workspace_trust
            .query(
                doc.workspace_root(),
                helix_loader::workspace_trust::TrustQuery::Git,
            )
            .is_trusted();
        let view = helix_view::view_mut!(editor, view_ids[0]);
        view.sync_changes(doc);
        if let Err(err) = doc.reload(view, &editor.diff_providers, trust_full) {
            stale.push(format!("{} ({err})", doc.display_name()));
            continue;
        }
        if let Some(path) = doc.path().map(ToOwned::to_owned) {
            editor
                .language_servers
                .file_event_handler
                .file_changed(path);
        }
        for view_id in view_ids {
            let doc = helix_view::doc_mut!(editor, &doc_id);
            let view = helix_view::view_mut!(editor, view_id);
            if view.doc == doc_id {
                view.sync_changes(doc);
                view.ensure_cursor_in_view(doc, scrolloff);
            }
        }
    }
    stale
}

/// Where `git clone` or `git init` put the repository it made.
fn new_repository(workdir: &std::path::Path, plan: &Plan) -> Option<PathBuf> {
    match plan.args.first().map(String::as_str)? {
        "init" => Some(workdir.join(plan.args.get(1).map_or(".", String::as_str))),
        "clone" => {
            let target = match plan.args.get(2) {
                Some(dir) => dir.clone(),
                // git's own choice: the last part of the URL, less `.git`.
                None => {
                    let url = plan.args.get(1)?.trim_end_matches('/');
                    let name = url.rsplit(['/', ':']).next()?;
                    name.strip_suffix(".git").unwrap_or(name).to_string()
                }
            };
            Some(workdir.join(target))
        }
        _ => None,
    }
}

/// Asks for a git command line, or a shell one, and runs it in the
/// repository; the process buffer (`$`) keeps what it printed.
pub fn command_prompt(workdir: PathBuf, shell: bool) -> crate::ui::Prompt {
    crate::ui::Prompt::new(
        if shell { "$ " } else { "git " }.into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let plan = if shell {
                command::typed_shell(input)
            } else {
                command::typed_git(input)
            };
            match plan {
                Ok(plan) => run(cx, plan, workdir.clone()),
                Err(err) => cx.editor.set_error(err),
            }
        },
    )
}

/// The Git views, by what they show, and the id each is found by.
fn view_ids(view: helix_magit::transient::GitView) -> &'static [&'static str] {
    use helix_magit::transient::GitView;
    match view {
        GitView::Status => &[DiffView::ID],
        GitView::Log => &[crate::ui::log_view::LogView::ID],
        GitView::Commit => &[DiffView::COMMIT_ID],
        GitView::Diff => &[DiffView::RANGE_ID],
        GitView::Refs => &[DiffView::REFS_ID],
        GitView::Cherries => &[DiffView::CHERRIES_ID],
        GitView::Blame => &[crate::ui::blame_view::BlameView::ID],
    }
}

fn is_open(compositor: &mut Compositor, view: helix_magit::transient::GitView) -> bool {
    use crate::ui::blame_view::BlameView;
    use crate::ui::log_view::LogView;
    view_ids(view).iter().any(|id| {
        compositor.find_id::<DiffView>(id).is_some()
            || compositor.find_id::<LogView>(id).is_some()
            || compositor.find_id::<BlameView>(id).is_some()
    })
}

/// `J`: a menu of the Git views open now, to bring one to the front.
pub fn views_overlay(
    compositor: &mut Compositor,
    workdir: PathBuf,
) -> crate::ui::transient::TransientOverlay {
    use helix_magit::transient::GitView;
    let open: Vec<GitView> = GitView::ALL
        .into_iter()
        .map(|(_, view, _)| view)
        .filter(|view| is_open(compositor, *view))
        .collect();
    crate::ui::transient::TransientOverlay::new(
        helix_magit::transient::views_menu(&open),
        format!("{} open", open.len()),
        workdir,
    )
}

/// Brings a Git view to the front; the status and the log are opened when
/// they are not open.
pub fn switch_to(
    compositor: &mut Compositor,
    editor: &mut Editor,
    view: helix_magit::transient::GitView,
    workdir: &std::path::Path,
) {
    use helix_magit::transient::GitView;
    for id in view_ids(view) {
        if let Some(layer) = compositor.remove(id) {
            compositor.push(layer);
            return;
        }
    }
    match view {
        GitView::Status => match DiffView::new(workdir) {
            Ok(status) => compositor.push(Box::new(status)),
            Err(err) => editor.set_error(err.to_string()),
        },
        GitView::Log => compositor.push(Box::new(crate::ui::log_view::LogView::new(
            workdir.to_path_buf(),
            helix_magit::log::LogFilter::default(),
        ))),
        _ => editor.set_error("That view is not open"),
    }
}

/// One command the process buffer lists.
struct ProcessRecord {
    line: String,
    output: GitOutput,
}

/// Every command run from the menus, commit and rebase included, newest
/// last. Read-only commands the views run to draw themselves are not here:
/// they would bury the ones that changed something.
static PROCESS: std::sync::Mutex<Vec<ProcessRecord>> = std::sync::Mutex::new(Vec::new());

/// How many commands the process buffer keeps.
const PROCESS_LIMIT: usize = 200;

fn record(line: &str, output: &GitOutput) {
    if let Ok(mut records) = PROCESS.lock() {
        records.push(ProcessRecord {
            line: line.to_string(),
            output: output.clone(),
        });
        let excess = records.len().saturating_sub(PROCESS_LIMIT);
        records.drain(..excess);
    }
}

/// The process buffer: every command run so far and all it printed, in a
/// scratch buffer so it can be searched and copied from.
pub fn show_process(editor: &mut Editor) {
    let text = match PROCESS.lock() {
        Ok(records) if !records.is_empty() => records
            .iter()
            .map(|record| {
                let mut entry = format!(
                    "{} {}\n",
                    if record.output.success { "✓" } else { "✗" },
                    record.line
                );
                for line in record
                    .output
                    .stdout
                    .lines()
                    .chain(record.output.stderr.lines())
                {
                    let line = line
                        .rsplit('\r')
                        .next()
                        .unwrap_or(line)
                        .replace("\x1b[K", "");
                    if !line.trim().is_empty() {
                        entry.push_str(&format!("    {line}\n"));
                    }
                }
                entry
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => "No git command has been run from the menus yet.\n".to_string(),
    };
    editor.new_scratch_with_text(helix_view::editor::Action::Replace, &text);
}

/// Closes the status, log and commit views, which cover the whole editor,
/// before something in the editor itself has to be seen.
/// Makes room for something in the editor: docked, Magit's pane stays and
/// only gives the keys to the documents; otherwise its views close, since
/// they cover the editor.
pub fn step_aside(compositor: &mut Compositor, editor: &mut Editor) {
    if editor.dock.is_docked(crate::ui::dock::MAGIT) {
        editor.dock.focus(None);
    } else {
        close_views(compositor);
    }
}

pub fn close_views(compositor: &mut Compositor) {
    compositor.remove(crate::ui::blame_view::BlameView::ID);
    compositor.remove(DiffView::REFS_ID);
    compositor.remove(DiffView::CHERRIES_ID);
    compositor.remove(DiffView::COMMIT_ID);
    compositor.remove(crate::ui::log_view::LogView::ID);
    compositor.remove(DiffView::ID);
}

/// Shows the outcome and brings the status buffer back in step.
fn report(editor: &mut Editor, compositor: &mut Compositor, line: &str, output: GitOutput) {
    record(line, &output);
    // Whatever ran may have rewritten files that are open.
    let stale = stale_message(&refresh_documents(editor));
    match (output.success, stale) {
        (true, None) => editor.set_status(format!("{line}: {}", output.summary())),
        (true, Some(stale)) => editor.set_error(stale),
        // git's own diagnosis is more useful than anything invented here.
        (false, None) => editor.set_error(format!("{line}: {}", output.summary())),
        (false, Some(stale)) => editor.set_error(format!("{line}: {}; {stale}", output.summary())),
    }

    // The index, HEAD, the working tree or the refs may all have moved.
    for id in [
        DiffView::ID,
        DiffView::REFS_ID,
        DiffView::CHERRIES_ID,
        DiffView::REPOSITORIES_ID,
    ] {
        if let Some(view) = compositor.find_id::<DiffView>(id) {
            view.refresh(editor);
        }
    }
    if let Some(log) =
        compositor.find_id::<crate::ui::log_view::LogView>(crate::ui::log_view::LogView::ID)
    {
        log.reload();
    }
}

/// Opens a buffer for the commit message.
///
/// Writing the buffer commits; quitting without writing abandons it and leaves
/// the index untouched. Closing is not the trigger, because Helix keeps a
/// buffer open when its view goes away, so `:wq` never closes the document —
/// and on a single view it would quit the editor before the commit could run.
fn compose(editor: &mut Editor, mut plan: Plan, workdir: PathBuf, seed: Seed) {
    if seed.amends_head() && command::head_is_pushed(&workdir) {
        // Amending a commit that is already on the remote rewrites history
        // someone else may have; that is worth stopping for.
        editor.set_error("HEAD is already pushed; amending it would rewrite published history");
        return;
    }
    // Squashing in at once rewrites history too: refused before a message
    // is written for nothing. The target is kept as a hash, since the new
    // commit moves what `HEAD~1` names.
    if let Some(target) = &plan.fold_into {
        match helix_magit::absorb::check_fold(&workdir, target) {
            Ok(full) => plan.fold_into = Some(full),
            Err(err) => {
                editor.set_error(err);
                return;
            }
        }
    }

    let message_path = workdir.join(".git").join("COMMIT_EDITMSG");
    // `--verbose` puts the diff below the message; git's own switch would
    // do nothing, since the message comes with `-F` and no editor runs.
    let verbose = plan
        .args
        .iter()
        .any(|arg| arg == "--verbose" || arg == "-v");
    let template = command::commit_template_for(&workdir, &seed, verbose);

    if let Err(err) = std::fs::write(&message_path, &template) {
        editor.set_error(format!("could not write the commit message: {err}"));
        return;
    }

    // Opened in a split rather than in place: closing the message buffer must
    // return to what the user was doing, and `:wq` on the only buffer would
    // quit Helix before the commit could run.
    if let Err(err) = editor.open(&message_path, helix_view::editor::Action::HorizontalSplit) {
        editor.set_error(format!("could not open the commit message: {err}"));
        return;
    }

    // A new message starts the history from its own draft.
    *HISTORY.lock().unwrap() = None;
    editor.pending_commit = Some(PendingCommit {
        message_path,
        args: plan.args,
        working_directory: workdir,
        fold_into: plan.fold_into,
        checked: false,
        prelude: plan.prelude,
    });
    editor.set_status("Write the message, then `:w` to commit (`:q!` aborts)");
}

/// Commits when the message buffer is written.
///
/// Called from the editor's save path. Writing is what confirms; quitting
/// without writing abandons the commit and leaves the index untouched.
///
/// Returns whether the write belonged to a pending commit.
pub fn commit_if_written(editor: &mut Editor, path: &std::path::Path) -> bool {
    let Some(pending) = editor.pending_commit.take() else {
        return false;
    };

    // Any other buffer being written is not this one.
    if path != pending.message_path {
        editor.pending_commit = Some(pending);
        return false;
    }

    editor.set_status("Committing…");
    finish_commit(pending);
    true
}

/// Reads the written message and commits, or reports that it was abandoned.
fn finish_commit(pending: PendingCommit) {
    // Off the editor's thread: a `pre-commit` hook can take a while.
    tokio::task::spawn_blocking(move || {
        let message = std::fs::read_to_string(&pending.message_path).unwrap_or_default();
        let body = command::strip_comments(&message);

        if body.is_empty() {
            crate::job::dispatch_blocking(move |editor, _| {
                editor.set_status("Commit aborted: the message was empty");
            });
            return;
        }

        // git-commit's style checks: asked about, not enforced.
        let problems = helix_magit::message::style_problems(&body);
        if !pending.checked && !problems.is_empty() {
            crate::job::dispatch_blocking(move |editor, compositor| {
                // Still pending: saying no leaves the message to fix and
                // write again.
                editor.pending_commit = Some(pending);
                let question = format!("{}. Commit anyway? (y/N)", sentence(&problems));
                compositor.push(Box::new(Confirm::new(
                    question,
                    "No keeps the message open to fix",
                    |cx| {
                        if let Some(mut pending) = cx.editor.pending_commit.take() {
                            pending.checked = true;
                            cx.editor.set_status("Committing…");
                            finish_commit(pending);
                        }
                    },
                )));
            });
            return;
        }

        let message_path = pending.message_path.clone();

        // git's `-F` does not strip comments — only its own editor path does —
        // so the cleaned body is written back before it is handed over. That
        // also means what was checked for emptiness is exactly what is
        // committed, with no second interpretation.
        if let Err(err) = std::fs::write(&message_path, format!("{body}\n")) {
            crate::job::dispatch_blocking(move |editor, _| {
                editor.set_error(format!("could not write the commit message: {err}"));
            });
            return;
        }

        // The cherry-pick or revert the message is for, taken uncommitted.
        for prelude in &pending.prelude {
            let line = format!("git {}", prelude.join(" "));
            match GitCommand::new(&pending.working_directory, prelude.clone()).run() {
                Ok(output) if output.success => {}
                outcome => {
                    remember_message(&body);
                    crate::job::dispatch_blocking(move |editor, compositor| match outcome {
                        Ok(output) => report(editor, compositor, &line, output),
                        Err(err) => editor.set_error(format!("{line}: {err}")),
                    });
                    return;
                }
            }
        }

        let mut args = pending.args;
        // `-F` keeps the subprocess from wanting an editor of its own.
        args.push("-F".into());
        args.push(message_path.to_string_lossy().into_owned());

        let mut outcome = GitCommand::new(&pending.working_directory, args.clone()).run();
        let line = format!("git {}", args[..args.len() - 2].join(" "));
        // The instant squash: the commit is made, now it goes in.
        if let (Ok(output), Some(target)) = (&mut outcome, &pending.fold_into) {
            if output.success {
                match helix_magit::absorb::fold_in(&pending.working_directory, target) {
                    Ok(()) => output.stdout.push_str("Squashed into its commit\n"),
                    Err(err) => {
                        output.success = false;
                        output.stderr.push_str(&err);
                    }
                }
            }
        }

        // A message whose commit did not go through is kept, to be had
        // back with `:magit-message-previous`.
        let committed = outcome.as_ref().is_ok_and(|output| output.success);
        if !committed && pending.fold_into.is_none() {
            remember_message(&body);
        }

        crate::job::dispatch_blocking(move |editor, compositor| match outcome {
            Ok(output) => {
                let succeeded = output.success;
                report(editor, compositor, &line, output);
                if succeeded {
                    close_message_buffer(editor, &message_path);
                }
            }
            Err(err) => editor.set_error(format!("{line}: {err}")),
        });
    });
}

/// `a, b and c`, capitalised: the style problems as one sentence.
fn sentence(parts: &[String]) -> String {
    let joined = match parts {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    };
    let mut chars = joined.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => joined,
    }
}

// ── The message history ──────────────────────────────────────────────────
//
// Magit's `M-p` / `M-n` in the message buffer: messages that were written
// but not committed, then those of the recent commits.

/// Messages whose commit did not go through, newest first.
static SAVED_MESSAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Where the message buffer is in the history, and the draft it started
/// from, to come back to.
struct History {
    path: PathBuf,
    position: Option<usize>,
    draft: String,
}

static HISTORY: std::sync::Mutex<Option<History>> = std::sync::Mutex::new(None);

fn remember_message(message: &str) {
    let mut saved = SAVED_MESSAGES.lock().unwrap();
    saved.retain(|kept| kept != message);
    saved.insert(0, message.to_string());
    saved.truncate(50);
}

/// `:magit-message-previous` / `-next`: replaces the message being written
/// with an older one, or a newer one, back to the draft.
pub fn message_history(editor: &mut Editor, older: bool) {
    let Some(pending) = &editor.pending_commit else {
        editor.set_error("No commit message is being written");
        return;
    };
    let (path, workdir) = (
        pending.message_path.clone(),
        pending.working_directory.clone(),
    );
    let (view, doc) = helix_view::current!(editor);
    if doc.path() != Some(&path) {
        editor.set_error("Not in the commit message buffer");
        return;
    }

    let mut entries = SAVED_MESSAGES.lock().unwrap().clone();
    for message in helix_magit::message::recent_messages(&workdir, 100) {
        if !entries.contains(&message) {
            entries.push(message);
        }
    }

    let before = doc.text().clone();
    let buffer = before.to_string();
    let mut history = HISTORY.lock().unwrap();
    let state = match history.as_mut() {
        Some(state) if state.path == path => state,
        _ => history.insert(History {
            path,
            position: None,
            draft: helix_magit::message::message_part(&buffer).to_string(),
        }),
    };
    let next = match (older, state.position) {
        (true, None) if entries.is_empty() => None,
        (true, None) => Some(Some(0)),
        (true, Some(at)) if at + 1 < entries.len() => Some(Some(at + 1)),
        (false, Some(0)) => Some(None),
        (false, Some(at)) => Some(Some(at - 1)),
        _ => None,
    };
    let Some(next) = next else {
        let message = if older {
            "No older message"
        } else {
            "Back at the draft already"
        };
        editor.set_status(message);
        return;
    };
    if state.position.is_none() {
        state.draft = helix_magit::message::message_part(&buffer).to_string();
    }
    state.position = next;
    let message = match next {
        Some(at) => entries[at].clone(),
        None => state.draft.clone(),
    };
    let text = helix_magit::message::replace_message(&buffer, &message);
    let after = helix_core::Rope::from(text.as_str());
    let transaction = helix_core::diff::compare_ropes(&before, &after);
    doc.apply(&transaction, view.id);
    editor.set_status(match next {
        Some(at) => format!("Message {} of {}", at + 1, entries.len()),
        None => "Back to the draft".to_string(),
    });
}

/// `:magit-message-diff`: what the commit will record, beside its message —
/// the status buffer at its staged changes.
pub fn message_diff(compositor: &mut Compositor, editor: &mut Editor) {
    let Some(pending) = &editor.pending_commit else {
        editor.set_error("No commit message is being written");
        return;
    };
    let workdir = pending.working_directory.clone();
    let amending = pending.args.iter().any(|arg| arg == "--amend");
    let target = helix_magit::transient::JumpTarget::Staged;
    let shown = match compositor.find_id::<DiffView>(DiffView::ID) {
        Some(view) => {
            view.refresh(editor);
            view.jump_to(target)
        }
        None => match DiffView::new(&workdir) {
            Ok(mut view) => {
                let shown = view.jump_to(target);
                compositor.push(Box::new(view));
                shown
            }
            Err(err) => {
                editor.set_error(err.to_string());
                return;
            }
        },
    };
    match (shown, amending) {
        (true, _) => editor.set_status("The staged changes, which the commit records"),
        (false, true) => {
            editor.set_status("Nothing staged: the amend keeps HEAD's changes as they are")
        }
        (false, false) => editor.set_error("Nothing is staged"),
    }
}

/// Closes the message buffer once its commit has landed.
///
/// Leaving it open would invite a second write, which would commit again.
fn close_message_buffer(editor: &mut Editor, path: &std::path::Path) {
    let Some(id) = editor
        .documents()
        .find(|doc| doc.path().is_some_and(|doc_path| doc_path == path))
        .map(|doc| doc.id())
    else {
        return;
    };
    let _ = editor.close_document(id, true);
}

#[cfg(test)]
mod tests {
    use super::{has_base, sentence};

    #[test]
    fn style_problems_read_as_one_sentence() {
        let parts = |list: &[&str]| -> Vec<String> { list.iter().map(|p| p.to_string()).collect() };
        assert_eq!(sentence(&parts(&["the line is long"])), "The line is long");
        assert_eq!(
            sentence(&parts(&["one", "two", "three"])),
            "One, two and three"
        );
    }

    #[test]
    fn a_rebase_base_is_the_first_argument_that_is_not_a_flag() {
        let args = |list: &[&str]| -> Vec<String> { list.iter().map(|a| a.to_string()).collect() };
        assert!(!has_base(&args(&["rebase", "--interactive"])));
        assert!(!has_base(&args(&[
            "rebase",
            "--interactive",
            "--autosquash"
        ])));
        assert!(has_base(&args(&["rebase", "--interactive", "abc123^"])));
        assert!(has_base(&args(&["rebase", "--interactive", "--root"])));
    }
}

// ── Copying, and the revision stack ──────────────────────────────────────
//
// Revisions looked at — a commit opened, a hash copied — are kept, newest
// first, so a commit message can refer to one without retyping its hash.

/// A remembered revision: its abbreviated hash and subject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    pub hash: String,
    pub subject: String,
}

impl Remembered {
    /// How a commit is referred to in prose: `abc1234 ("Subject")`.
    pub fn reference(&self) -> String {
        format!("{} (\"{}\")", self.hash, self.subject)
    }
}

static REVISIONS: std::sync::Mutex<Vec<Remembered>> = std::sync::Mutex::new(Vec::new());
const REVISION_STACK_SIZE: usize = 30;

/// Adds the commit `rev` names to the top of the stack.
pub fn remember_revision(workdir: &std::path::Path, rev: &str) {
    let args = ["log", "-1", "--format=%h%x00%s", rev, "--"]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    let Ok(output) = GitCommand::new(workdir, args).run() else {
        return;
    };
    let Some((hash, subject)) = output.stdout.trim_end().split_once('\0') else {
        return;
    };
    push_revision(Remembered {
        hash: hash.to_string(),
        subject: subject.to_string(),
    });
}

fn push_revision(revision: Remembered) {
    let Ok(mut stack) = REVISIONS.lock() else {
        return;
    };
    stack.retain(|known| known.hash != revision.hash);
    stack.insert(0, revision);
    stack.truncate(REVISION_STACK_SIZE);
}

pub fn revision_stack() -> Vec<Remembered> {
    REVISIONS
        .lock()
        .map(|stack| stack.clone())
        .unwrap_or_default()
}

/// Copies `value` into the default yank register and the clipboard, and
/// remembers it when it is a revision.
pub fn copy_value(editor: &mut Editor, workdir: &std::path::Path, value: &str, kind: AskKind) {
    let register = editor.config().default_yank_register;
    if let Err(err) = editor.registers.write(register, vec![value.to_string()]) {
        editor.set_error(err.to_string());
        return;
    }
    // The clipboard may not be there (no display, no provider); the
    // register is.
    let _ = editor.registers.write('+', vec![value.to_string()]);
    if matches!(kind, AskKind::Revision | AskKind::Branch | AskKind::Tag) {
        remember_revision(workdir, value);
    }
    editor.set_status(format!("Copied {value}"));
}

/// Asks which remembered revision to insert at the cursors, as
/// `abc1234 ("Subject")`.
pub fn revision_prompt(editor: &mut Editor) -> Option<Box<dyn crate::compositor::Component>> {
    let stack = revision_stack();
    if stack.is_empty() {
        editor.set_error("No revision looked at yet: open a commit, or copy one with C-w");
        return None;
    }
    let lines: Vec<String> = stack
        .iter()
        .map(|revision| format!("{} {}", revision.hash, revision.subject))
        .collect();
    let prompt = crate::ui::Prompt::new(
        "Insert revision (RET: the newest): ".into(),
        None,
        move |_, input| {
            let input = input.to_lowercase();
            lines
                .iter()
                .filter(|line| line.to_lowercase().contains(&input))
                .map(|line| (0.., line.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            // Nothing typed takes the newest, as Magit's does.
            let hash = input.split_whitespace().next();
            let text = match hash {
                None => stack.first().map(Remembered::reference).unwrap_or_default(),
                Some(hash) => stack
                    .iter()
                    .find(|revision| revision.hash == hash)
                    .map(Remembered::reference)
                    .unwrap_or_else(|| input.trim().to_string()),
            };
            if text.is_empty() {
                return;
            }
            let (view, doc) = helix_view::current!(cx.editor);
            let selection = doc.selection(view.id).clone();
            let transaction =
                helix_core::Transaction::change_by_selection(doc.text(), &selection, |range| {
                    (range.head, range.head, Some(text.as_str().into()))
                });
            doc.apply(&transaction, view.id);
        },
    );
    Some(Box::new(prompt))
}

/// Runs `git mergetool` on a conflicted file in an integrated terminal of
/// its own, which the tool — often a terminal program itself — needs. A
/// terminal of its own, so that nothing running in another one gets the
/// command typed into it.
pub fn mergetool(
    editor: &mut Editor,
    compositor: &mut Compositor,
    workdir: &std::path::Path,
    path: &str,
) {
    let Some(view) = crate::commands::new_terminal_view(editor, Some(workdir.to_path_buf())) else {
        return;
    };
    step_aside(compositor, editor);
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let line = format!("git mergetool -- {}\r", quote(path));
    if let Some(entry) = editor.terminals.current_entry_mut() {
        entry.name = Some(format!("mergetool {path}"));
        entry.terminal.write(line.into_bytes());
    }
    crate::ui::terminal::show(compositor, editor, view);
}

/// Stops an interactive rebase at the commit that last changed `line` (from
/// 0) of `path`: blame and rebase together.
pub fn edit_line_commit(
    compositor: &mut Compositor,
    cx: &mut Context,
    workdir: PathBuf,
    path: &str,
    line: usize,
) {
    let blamed = match helix_magit::blame::blame(&workdir, std::path::Path::new(path), None) {
        Ok(lines) => lines,
        Err(err) => return cx.editor.set_error(err),
    };
    let Some(blamed) = blamed.get(line) else {
        return cx
            .editor
            .set_error("That line is not in the committed file");
    };
    if blamed.is_uncommitted() {
        return cx.editor.set_error("That line is not committed yet");
    }
    edit_commit(compositor, cx, workdir, &blamed.hash);
}

/// Confirms, then starts an interactive rebase stopping at `commit`.
pub fn edit_commit(compositor: &mut Compositor, cx: &mut Context, workdir: PathBuf, commit: &str) {
    let short = &commit[..commit.len().min(7)];
    let subject = helix_magit::status::commit_subject(&workdir, commit).unwrap_or_default();
    let mut plan = Plan::new(
        [commit.to_string()],
        &format!("Rebase to edit {short} {subject}"),
    );
    plan.special = Some(helix_magit::Special::EditCommit);
    plan.destructive = true;
    execute(compositor, cx, plan, workdir);
}

/// `rev`'s abbreviated hash.
pub fn short_hash(workdir: &std::path::Path, rev: &str) -> Option<String> {
    let args = ["rev-parse", "--short", "--verify", "--quiet", rev]
        .iter()
        .map(|arg| arg.to_string())
        .collect();
    let output = GitCommand::new(workdir, args).run().ok()?;
    output
        .success
        .then(|| output.stdout.trim().to_string())
        .filter(|hash| !hash.is_empty())
}

// ── Trailers in the message buffer ───────────────────────────────────────

/// The repository a document belongs to — the commit message file lives in
/// the git directory, whose parent is the working tree.
pub fn document_repository(editor: &Editor) -> Option<PathBuf> {
    let doc = helix_view::doc!(editor);
    let dir = doc.path()?.parent()?.to_path_buf();
    let dir = if dir.file_name().is_some_and(|name| name == ".git") {
        dir.parent()?.to_path_buf()
    } else {
        dir
    };
    helix_magit::Repository::discover(&dir)
        .ok()
        .map(|repository| repository.workdir().to_path_buf())
}

/// Adds `trailer` to the message in the current buffer, where git expects
/// trailers.
pub fn insert_trailer(editor: &mut Editor, trailer: &str) {
    let (view, doc) = helix_view::current!(editor);
    let before = doc.text().clone();
    let original = before.to_string();
    let text = helix_magit::trailers::add(&original, trailer);
    if text == original {
        editor.set_status(format!("Already there: {trailer}"));
        return;
    }
    let after = helix_core::Rope::from(text.as_str());
    let transaction = helix_core::diff::compare_ropes(&before, &after);
    doc.apply(&transaction, view.id);
    editor.set_status(format!("Added {trailer}"));
}

/// `:magit-trailer`: asks for the kind of trailer, then the person — from
/// the history, most frequent first, the user first for a sign-off — and
/// adds it. `kind` and `person` skip their question when given.
pub fn trailer_prompt(
    editor: &mut Editor,
    kind: Option<String>,
    person: Option<String>,
) -> Option<Box<dyn crate::compositor::Component>> {
    let Some(workdir) = document_repository(editor) else {
        editor.set_error("This buffer is not in a git repository");
        return None;
    };
    let kind = match kind.map(|kind| helix_magit::trailers::resolve_kind(&kind)) {
        Some(Ok(kind)) => Some(kind),
        Some(Err(err)) => {
            editor.set_error(err);
            return None;
        }
        None => None,
    };
    match (kind, person) {
        (Some(kind), Some(person)) => {
            insert_trailer(editor, &format!("{kind}: {person}"));
            None
        }
        (Some(kind), None) => Some(person_prompt(workdir, kind)),
        (None, _) => {
            let prompt = crate::ui::Prompt::new(
                "Trailer (RET: Signed-off-by): ".into(),
                None,
                |_, input| {
                    let input = input.to_lowercase();
                    helix_magit::trailers::KINDS
                        .iter()
                        .filter(|kind| kind.to_lowercase().contains(&input))
                        .map(|kind| (0.., (*kind).into()))
                        .collect()
                },
                move |cx, input, event| {
                    if event != crate::ui::PromptEvent::Validate {
                        return;
                    }
                    let kind = match input.trim() {
                        "" => "Signed-off-by".to_string(),
                        kind => match helix_magit::trailers::resolve_kind(kind) {
                            Ok(kind) => kind,
                            Err(err) => return cx.editor.set_error(err),
                        },
                    };
                    let workdir = workdir.clone();
                    // The next question, once this prompt has closed.
                    cx.jobs.callback(async move {
                        Ok(Callback::EditorCompositor(Box::new(
                            move |_: &mut Editor, compositor: &mut Compositor| {
                                compositor.push(person_prompt(workdir, kind));
                            },
                        )))
                    });
                },
            );
            Some(Box::new(prompt))
        }
    }
}

fn person_prompt(workdir: PathBuf, kind: String) -> Box<dyn crate::compositor::Component> {
    let mut people = helix_magit::trailers::people(&workdir);
    let me = helix_magit::trailers::me(&workdir);
    // One signs off oneself; one is not one's own co-author.
    if let Some(me) = me {
        people.retain(|person| *person != me);
        if kind == "Signed-off-by" {
            people.insert(0, me);
        } else if !kind.starts_with("Co-") {
            people.push(me);
        }
    }
    let default = people.first().cloned();
    let label = match &default {
        Some(default) => format!("{kind} (RET: {default}): "),
        None => format!("{kind}: "),
    };
    Box::new(crate::ui::Prompt::new(
        label.into(),
        None,
        move |_, input| {
            let input = input.to_lowercase();
            people
                .iter()
                .filter(|person| person.to_lowercase().contains(&input))
                .take(50)
                .map(|person| (0.., person.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let person = match input.trim() {
                "" => match &default {
                    Some(default) => default.clone(),
                    None => return,
                },
                person => person.to_string(),
            };
            insert_trailer(cx.editor, &format!("{kind}: {person}"));
        },
    ))
}

// ── Ediff: versions of a file side by side ───────────────────────────────
//
// See `helix_magit::ediff`. Each version opens in a split, the diff gutter
// set against the version it is compared with; `]g` / `[g` move between the
// changes as in any buffer.

use helix_magit::ediff::Version;
use helix_magit::transient::EdiffKind;
use helix_view::editor::Action;

/// The index versions being edited, by the file they were written to:
/// writing one stages it.
static INDEX_EDITS: std::sync::Mutex<Vec<(PathBuf, PathBuf, PathBuf)>> =
    std::sync::Mutex::new(Vec::new());

/// Opens `version` of `path` with `action`, its gutter against `against`.
fn open_version(
    editor: &mut Editor,
    workdir: &std::path::Path,
    version: &Version,
    path: &std::path::Path,
    action: Action,
    against: Option<&Version>,
) -> Result<helix_view::ViewId, String> {
    let file = match version {
        Version::Worktree => workdir.join(path),
        version => {
            // A version the file is missing from shows as empty.
            let content = helix_magit::ediff::content(workdir, version, path).unwrap_or_default();
            helix_magit::ediff::materialize(workdir, version, path, &content)?
        }
    };
    let doc_id = editor
        .open(&file, action)
        .map_err(|err| format!("could not open {}: {err}", file.display()))?;
    let doc = helix_view::doc_mut!(editor, &doc_id);
    // Only the index's version is edited, and only when staging.
    doc.readonly = !matches!(version, Version::Worktree | Version::Index);
    if let Some(against) = against {
        let base = helix_magit::ediff::content(workdir, against, path).unwrap_or_default();
        doc.set_diff_base(base.into_bytes());
    }
    Ok(helix_view::view!(editor).id)
}

/// Two versions side by side, the older on the left, the cursor on the
/// newer.
fn compare(
    editor: &mut Editor,
    workdir: &std::path::Path,
    path: &std::path::Path,
    old: Version,
    new: Version,
) {
    let result = open_version(editor, workdir, &old, path, Action::Replace, None).and_then(|_| {
        open_version(
            editor,
            workdir,
            &new,
            path,
            Action::VerticalSplit,
            Some(&old),
        )
    });
    match result {
        Ok(_) => editor.set_status(format!(
            "{}: {} | {}; ]g / [g move between the changes",
            path.display(),
            old.label(),
            new.label()
        )),
        Err(err) => editor.set_error(err),
    }
}

/// Ours, the merged file and theirs; the cursor on the merged file's first
/// conflict, where `:conflict-take` picks a side.
fn resolve3(editor: &mut Editor, workdir: &std::path::Path, path: &std::path::Path) {
    let base = Version::Stage(1);
    let result = (|| {
        open_version(
            editor,
            workdir,
            &Version::Stage(2),
            path,
            Action::Replace,
            Some(&base),
        )?;
        let merged = open_version(
            editor,
            workdir,
            &Version::Worktree,
            path,
            Action::VerticalSplit,
            None,
        )?;
        open_version(
            editor,
            workdir,
            &Version::Stage(3),
            path,
            Action::VerticalSplit,
            Some(&base),
        )?;
        editor.focus(merged);
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => edit_conflict(editor, &workdir.join(path)),
        Err(err) => editor.set_error(err),
    }
}

/// HEAD, the index and the working tree; the cursor on the index's
/// version, which is edited and written to stage it.
fn stage3(editor: &mut Editor, workdir: &std::path::Path, path: &std::path::Path) {
    let result = (|| {
        open_version(editor, workdir, &Version::Head, path, Action::Replace, None)?;
        let index = open_version(
            editor,
            workdir,
            &Version::Index,
            path,
            Action::VerticalSplit,
            Some(&Version::Head),
        )?;
        let index_file = helix_view::doc!(editor)
            .path()
            .map(|path| path.to_path_buf())
            .ok_or("the index version has no file")?;
        open_version(
            editor,
            workdir,
            &Version::Worktree,
            path,
            Action::VerticalSplit,
            Some(&Version::Index),
        )?;
        editor.focus(index);
        let mut edits = INDEX_EDITS.lock().unwrap();
        edits.retain(|(file, _, _)| *file != index_file);
        edits.push((index_file, workdir.to_path_buf(), path.to_path_buf()));
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => editor.set_status(format!(
            "{}: HEAD | index | worktree. Bring changes into the index's version, then :w stages it",
            path.display()
        )),
        Err(err) => editor.set_error(err),
    }
}

/// Stages an edited index version when its buffer is written. Returns
/// whether the write was one.
pub fn index_if_written(editor: &mut Editor, path: &std::path::Path) -> bool {
    let entry = INDEX_EDITS
        .lock()
        .unwrap()
        .iter()
        .find(|(file, _, _)| file == path)
        .cloned();
    let Some((file, workdir, relative)) = entry else {
        return false;
    };
    let content = std::fs::read_to_string(&file).unwrap_or_default();
    match helix_magit::ediff::write_index(&workdir, &relative, &content) {
        Ok(()) => {
            // The working tree's gutter is against the index: it moved.
            let worktree = workdir.join(&relative);
            if let Some(doc) = editor
                .documents_mut()
                .find(|doc| doc.path().is_some_and(|p| *p == worktree))
            {
                doc.set_diff_base(content.into_bytes());
            }
            editor.set_status(format!("Staged {} as edited", relative.display()));
        }
        Err(err) => editor.set_error(err),
    }
    true
}

/// Asks to choose among `options`, then goes on with the choice. With
/// `strict`, only one of them is taken: a file has to be one listed, where
/// a revision can be any the user types.
fn choose(
    compositor: &mut Compositor,
    label: String,
    options: Vec<String>,
    strict: bool,
    then: impl Fn(&mut Editor, &mut Compositor, String) + Send + Sync + Clone + 'static,
) {
    let listed = options.clone();
    let prompt = crate::ui::Prompt::new(
        label.into(),
        None,
        move |_, input: &str| {
            options
                .iter()
                .filter(|option| option.contains(input))
                .map(|option| (0.., option.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate || input.trim().is_empty() {
                return;
            }
            if strict && !listed.iter().any(|option| option == input.trim()) {
                cx.editor
                    .set_error(format!("{} is not one of the choices", input.trim()));
                return;
            }
            let (answer, then) = (input.trim().to_string(), then.clone());
            cx.jobs.callback(async move {
                Ok(Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        then(editor, compositor, answer)
                    },
                )))
            });
        },
    );
    compositor.push(Box::new(prompt));
}

/// The Ediff menu's actions. `target` is what the menu was opened on: a
/// file, or a commit or stash.
pub fn ediff(
    compositor: &mut Compositor,
    editor: &mut Editor,
    workdir: PathBuf,
    kind: EdiffKind,
    target: Option<(String, AskKind)>,
) {
    use helix_magit::ediff::{changed_paths, unmerged_paths};
    let path = target
        .as_ref()
        .filter(|(_, kind)| *kind == AskKind::Path)
        .map(|(path, _)| path.clone());
    let revision = target
        .as_ref()
        .filter(|(_, kind)| matches!(kind, AskKind::Revision | AskKind::Stash))
        .map(|(rev, _)| rev.clone());
    step_aside(compositor, editor);

    // Runs `open` on the path at the cursor, or on one chosen from `paths`.
    let with_path =
        |compositor: &mut Compositor,
         editor: &mut Editor,
         paths: Vec<String>,
         what: &str,
         open: fn(&mut Editor, &std::path::Path, &std::path::Path)| {
            let workdir = workdir.clone();
            match &path {
                Some(path) => open(editor, &workdir, std::path::Path::new(path)),
                None if paths.is_empty() => editor.set_error(format!("No {what}")),
                None => choose(
                    compositor,
                    format!("File ({what}): "),
                    paths,
                    true,
                    move |editor, _, path| open(editor, &workdir, std::path::Path::new(&path)),
                ),
            }
        };

    match kind {
        EdiffKind::Dwim => {
            let Some(path) = path.clone() else {
                // On a commit or stash, its change to one of its files.
                return match revision {
                    Some(_) => ediff(compositor, editor, workdir, EdiffKind::Commit, target),
                    None => editor.set_error("Ediff what? Open the menu on a file or a commit"),
                };
            };
            match helix_magit::ediff::dwim(&workdir, &path) {
                Some(helix_magit::ediff::Dwim::Resolve) => {
                    resolve3(editor, &workdir, std::path::Path::new(&path))
                }
                Some(helix_magit::ediff::Dwim::Unstaged) => compare(
                    editor,
                    &workdir,
                    std::path::Path::new(&path),
                    Version::Index,
                    Version::Worktree,
                ),
                Some(helix_magit::ediff::Dwim::Staged) => compare(
                    editor,
                    &workdir,
                    std::path::Path::new(&path),
                    Version::Head,
                    Version::Index,
                ),
                None => editor.set_error(format!("{path} has no changes to compare")),
            }
        }
        EdiffKind::Resolve => with_path(
            compositor,
            editor,
            unmerged_paths(&workdir),
            "in conflict",
            resolve3,
        ),
        EdiffKind::Stage => {
            let mut paths = changed_paths(&workdir, &[]);
            for staged in changed_paths(&workdir, &["--cached"]) {
                if !paths.contains(&staged) {
                    paths.push(staged);
                }
            }
            with_path(compositor, editor, paths, "with changes", stage3)
        }
        EdiffKind::Unstaged => with_path(
            compositor,
            editor,
            changed_paths(&workdir, &[]),
            "with unstaged changes",
            |editor, workdir, path| {
                compare(editor, workdir, path, Version::Index, Version::Worktree)
            },
        ),
        EdiffKind::Staged => with_path(
            compositor,
            editor,
            changed_paths(&workdir, &["--cached"]),
            "with staged changes",
            |editor, workdir, path| compare(editor, workdir, path, Version::Head, Version::Index),
        ),
        EdiffKind::Worktree => with_path(
            compositor,
            editor,
            changed_paths(&workdir, &["HEAD"]),
            "changed since HEAD",
            |editor, workdir, path| {
                compare(editor, workdir, path, Version::Head, Version::Worktree)
            },
        ),
        EdiffKind::Commit | EdiffKind::Stash | EdiffKind::Compare => {
            let label = match kind {
                EdiffKind::Commit => "Commit: ",
                EdiffKind::Stash => "Stash: ",
                _ => "Compare A..B (B empty for the working tree): ",
            };
            let names = helix_magit::refs::names(
                &workdir,
                if kind == EdiffKind::Stash {
                    AskKind::Stash
                } else {
                    AskKind::Revision
                },
            );
            let ask_file = move |editor: &mut Editor,
                                 compositor: &mut Compositor,
                                 answer: String| {
                // Commit and stash: their parent to them; else A..B.
                let (old, new) = match kind {
                    EdiffKind::Commit | EdiffKind::Stash => (
                        Version::Rev(format!("{answer}^")),
                        Version::Rev(answer.clone()),
                    ),
                    _ => match answer.split_once("..") {
                        Some((a, "")) => (Version::Rev(a.to_string()), Version::Worktree),
                        Some((a, b)) => (Version::Rev(a.to_string()), Version::Rev(b.to_string())),
                        None => (Version::Rev(answer.clone()), Version::Worktree),
                    },
                };
                let spec = |version: &Version| match version {
                    Version::Rev(rev) => rev.clone(),
                    _ => String::new(),
                };
                let (a, b) = (spec(&old), spec(&new));
                let mut args = vec![a.as_str()];
                if !b.is_empty() {
                    args.push(b.as_str());
                }
                let workdir = workdir.clone();
                let paths = helix_magit::ediff::changed_paths(&workdir, &args);
                if paths.is_empty() {
                    return editor.set_error(format!("No file changed in {answer}"));
                }
                choose(
                    compositor,
                    format!("File changed in {answer}: "),
                    paths,
                    true,
                    move |editor, _, path| {
                        compare(
                            editor,
                            &workdir,
                            std::path::Path::new(&path),
                            old.clone(),
                            new.clone(),
                        )
                    },
                );
            };
            match revision {
                Some(rev) if kind != EdiffKind::Compare => ask_file(editor, compositor, rev),
                _ => choose(compositor, label.to_string(), names, false, ask_file),
            }
        }
    }
}

// ── Blobs: a file as it was at a commit ──────────────────────────────────

/// The blob buffers open, by their file: the repository, the commit (full
/// hash) and the path in it. What `:magit-blob-previous` and the rest read.
static BLOBS: std::sync::Mutex<Vec<(PathBuf, PathBuf, String, PathBuf)>> =
    std::sync::Mutex::new(Vec::new());

/// Opens `path` as it was at `rev`, read-only, at `line` (0-based).
pub fn open_blob(
    editor: &mut Editor,
    workdir: &std::path::Path,
    rev: &str,
    path: &std::path::Path,
    line: usize,
    action: Action,
) {
    let Some(hash) = helix_magit::ediff::commit_hash(workdir, rev) else {
        return editor.set_error(format!("{rev} is not a commit"));
    };
    let Some(content) = helix_magit::ediff::content(workdir, &Version::Rev(hash.clone()), path)
    else {
        return editor.set_error(format!("{} is not in {rev}", path.display()));
    };
    let short = &hash[..hash.len().min(7)];
    let file = match helix_magit::ediff::materialize_blob(workdir, short, path, &content) {
        Ok(file) => file,
        Err(err) => return editor.set_error(err),
    };
    // A commit's version never changes, so one already open is reused.
    if let Err(err) = editor.open(&file, action) {
        return editor.set_error(format!("could not open {}: {err}", file.display()));
    }
    let (view, doc) = helix_view::current!(editor);
    doc.readonly = true;
    let text = doc.text().slice(..);
    let line = line.min(text.len_lines().saturating_sub(1));
    doc.set_selection(
        view.id,
        helix_core::Selection::point(text.line_to_char(line)),
    );
    helix_view::align_view(doc, view, helix_view::Align::Center);

    let mut blobs = BLOBS.lock().unwrap();
    blobs.retain(|(open, ..)| *open != file);
    blobs.push((
        file,
        workdir.to_path_buf(),
        hash.clone(),
        path.to_path_buf(),
    ));
    let subject = helix_magit::status::commit_subject(workdir, &hash).unwrap_or_default();
    editor.set_status(format!(
        "{} at {short} {subject} — :magit-blob-previous / -next, :magit-blob-commit",
        path.display()
    ));
}

/// The blob in the current buffer: its repository, commit and path.
fn current_blob(editor: &Editor) -> Option<(PathBuf, String, PathBuf)> {
    let path = helix_view::doc!(editor).path()?.to_path_buf();
    BLOBS
        .lock()
        .unwrap()
        .iter()
        .find(|(file, ..)| *file == path)
        .map(|(_, workdir, hash, path)| (workdir.clone(), hash.clone(), path.clone()))
}

/// `:magit-blob-previous` / `-next`: the same file at the commit before or
/// after the one shown, among those that changed it, the cursor on the
/// same line.
pub fn blob_step(editor: &mut Editor, older: bool) {
    let Some((workdir, hash, path)) = current_blob(editor) else {
        return editor.set_error("Not a file at a revision: see :magit-find-file");
    };
    let history = helix_magit::ediff::file_history(&workdir, &path);
    // A blob of a commit that did not change the file stands for the one
    // that last did.
    let at = history
        .iter()
        .position(|(commit, _)| *commit == hash)
        .or_else(|| {
            let last = helix_magit::ediff::last_change(&workdir, &hash, &path)?;
            history.iter().position(|(commit, _)| *commit == last)
        });
    let Some(at) = at else {
        return editor.set_error(format!("{} has no history here", path.display()));
    };
    let target = if older {
        history.get(at + 1)
    } else {
        at.checked_sub(1).and_then(|newer| history.get(newer))
    };
    let Some((commit, name)) = target.cloned() else {
        return editor.set_status(if older {
            "This is the oldest version".to_string()
        } else {
            format!(
                "This is the newest version; the file itself is {}",
                path.display()
            )
        });
    };
    let (view, doc) = helix_view::current!(editor);
    let line = doc.text().char_to_line(
        doc.selection(view.id)
            .primary()
            .cursor(doc.text().slice(..)),
    );
    open_blob(editor, &workdir, &commit, &name, line, Action::Replace);
}

/// `:magit-blob-commit`: the commit the shown version comes from.
pub fn blob_commit(compositor: &mut Compositor, editor: &mut Editor) {
    let Some((workdir, hash, _)) = current_blob(editor) else {
        return editor.set_error("Not a file at a revision: see :magit-find-file");
    };
    match DiffView::commit(&workdir, &hash) {
        Ok(view) => compositor.push(Box::new(view)),
        Err(err) => editor.set_error(err),
    }
}

/// `:magit-find-file` and the file menu's `f`: asks a revision, then a file
/// in it (`path` when given), and opens it as it was then.
pub fn find_file(compositor: &mut Compositor, workdir: PathBuf, path: Option<String>, line: usize) {
    let names = helix_magit::refs::names(&workdir, AskKind::Revision);
    choose(
        compositor,
        "Visit at revision: ".to_string(),
        names,
        false,
        move |editor, compositor, rev| {
            let workdir = workdir.clone();
            match &path {
                Some(path) => open_blob(
                    editor,
                    &workdir,
                    &rev,
                    std::path::Path::new(path),
                    line,
                    Action::Replace,
                ),
                None => {
                    let files = helix_magit::ediff::files_at(&workdir, &rev);
                    if files.is_empty() {
                        return editor.set_error(format!("{rev} has no files"));
                    }
                    choose(
                        compositor,
                        format!("File in {rev}: "),
                        files,
                        true,
                        move |editor, _, path| {
                            open_blob(
                                editor,
                                &workdir,
                                &rev,
                                std::path::Path::new(&path),
                                0,
                                Action::Replace,
                            )
                        },
                    );
                }
            }
        },
    );
}

/// The file menu's `R`: `git mv`, and the open buffer follows the file.
pub fn rename_file(compositor: &mut Compositor, editor: &Editor, workdir: PathBuf, path: String) {
    let start = path.clone();
    let prompt = crate::ui::Prompt::new(
        format!("Rename {path} to: ").into(),
        None,
        crate::ui::completers::filename,
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate {
                return;
            }
            let target = input.trim();
            if target.is_empty() || target == path {
                return;
            }
            let output = GitCommand::new(
                &workdir,
                vec!["mv".into(), "--".into(), path.clone(), target.into()],
            )
            .run();
            match output {
                Ok(output) if output.success => {
                    let (old, new) = (workdir.join(&path), workdir.join(target));
                    if let Some(doc_id) = cx.editor.document_by_path(&old).map(|doc| doc.id()) {
                        cx.editor.set_doc_path(doc_id, &new);
                    }
                    cx.editor.set_status(format!("Renamed {path} to {target}"));
                }
                Ok(output) => cx.editor.set_error(output.summary()),
                Err(err) => cx.editor.set_error(err.to_string()),
            }
        },
    )
    .with_line(start, editor);
    compositor.push(Box::new(prompt));
}

/// The file menu's `t`: the log of some of the file's lines, `git log -L`.
/// Offered the line at the cursor; `10,20` or `:funcname` are git's.
pub fn trace_lines(
    compositor: &mut Compositor,
    editor: &Editor,
    workdir: PathBuf,
    path: String,
    line: usize,
) {
    let prompt = crate::ui::Prompt::new(
        "Trace lines (start,end or :funcname): ".into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate || input.trim().is_empty() {
                return;
            }
            let filter = helix_magit::log::LogFilter {
                path: Some(PathBuf::from(&path)),
                lines: Some(input.trim().to_string()),
                ..helix_magit::log::LogFilter::default()
            };
            let workdir = workdir.clone();
            cx.jobs.callback(async move {
                Ok(Callback::EditorCompositor(Box::new(
                    move |_: &mut Editor, compositor: &mut Compositor| {
                        compositor
                            .push(Box::new(crate::ui::log_view::LogView::new(workdir, filter)));
                    },
                )))
            });
        },
    )
    .with_line(format!("{},{}", line + 1, line + 1), editor);
    compositor.push(Box::new(prompt));
}

/// The menus' actions that open a view or a buffer rather than run git:
/// the todo list of a stopped rebase, a merge's preview, the stash list,
/// another worktree, the submodules.
pub fn open_list(
    compositor: &mut Compositor,
    editor: &mut Editor,
    workdir: PathBuf,
    command: helix_magit::transient::MagitCommand,
) {
    use helix_magit::transient::{JumpTarget, MagitCommand};
    match command {
        MagitCommand::RebaseEditTodo => {
            let Some(todo) = helix_magit::status::git_dir(&workdir)
                .map(|dir| dir.join("rebase-merge").join("git-rebase-todo"))
                .filter(|todo| todo.exists())
            else {
                return editor.set_error("No interactive rebase is stopped");
            };
            step_aside(compositor, editor);
            match editor.open(&todo, Action::HorizontalSplit) {
                Ok(_) => editor
                    .set_status("The commits still to do: edit, :w, then continue the rebase (C)"),
                Err(err) => editor.set_error(err.to_string()),
            }
        }
        // What the merge would bring: from the merge base to the branch.
        MagitCommand::MergePreview => {
            let names = helix_magit::refs::names(&workdir, AskKind::Revision);
            choose(
                compositor,
                "Preview merging: ".to_string(),
                names,
                false,
                move |editor, compositor, rev| {
                    let base = GitCommand::new(
                        &workdir,
                        vec!["merge-base".into(), "HEAD".into(), rev.clone()],
                    )
                    .run();
                    match base {
                        Ok(output) if output.success => compositor.push(Box::new(DiffView::range(
                            &workdir,
                            output.stdout.trim().to_string(),
                            Some(rev),
                        ))),
                        Ok(output) => editor.set_error(output.summary()),
                        Err(err) => editor.set_error(err.to_string()),
                    }
                },
            );
        }
        MagitCommand::StashList => {
            let filter = helix_magit::log::LogFilter {
                range: Some("refs/stash".into()),
                reflog: true,
                ..helix_magit::log::LogFilter::default()
            };
            compositor.push(Box::new(crate::ui::log_view::LogView::new(workdir, filter)));
        }
        MagitCommand::WorktreeVisit => {
            let trees = helix_magit::status::read(&workdir).worktrees;
            if trees.is_empty() {
                return editor.set_error("No other worktree");
            }
            let paths: Vec<String> = trees
                .iter()
                .map(|tree| tree.path.display().to_string())
                .collect();
            choose(
                compositor,
                "Visit worktree: ".to_string(),
                paths,
                true,
                |editor, compositor, path| match DiffView::new(std::path::Path::new(&path)) {
                    Ok(view) => {
                        compositor.remove(DiffView::ID);
                        compositor.push(Box::new(view));
                    }
                    Err(err) => editor.set_error(err.to_string()),
                },
            );
        }
        MagitCommand::SubmoduleList => {
            let shown = match compositor.find_id::<DiffView>(DiffView::ID) {
                Some(view) => {
                    view.refresh(editor);
                    view.jump_to(JumpTarget::Submodules)
                }
                None => match DiffView::new(&workdir) {
                    Ok(mut view) => {
                        let shown = view.jump_to(JumpTarget::Submodules);
                        compositor.push(Box::new(view));
                        shown
                    }
                    Err(err) => return editor.set_error(err.to_string()),
                },
            };
            if !shown {
                editor.set_error("This repository has no submodules");
            }
        }
        _ => {}
    }
}

/// `:magit-save-patch <file>`: writes the diff shown — a commit, a range,
/// or the status's section at the cursor — as a patch file.
pub fn save_patch(compositor: &mut Compositor, editor: &mut Editor, file: &str) {
    let id = [DiffView::COMMIT_ID, DiffView::RANGE_ID, DiffView::ID]
        .into_iter()
        .find(|id| compositor.find_id::<DiffView>(id).is_some());
    let Some(view) = id.and_then(|id| compositor.find_id::<DiffView>(id)) else {
        return editor.set_error("No diff is shown");
    };
    let (workdir, patch) = view.patch_text();
    if patch.is_empty() {
        return editor.set_error("Nothing to save: move to a section with changes");
    }
    let path = workdir.join(helix_stdx::path::expand_tilde(std::path::Path::new(file)));
    match std::fs::write(&path, patch) {
        Ok(()) => editor.set_status(format!("Saved the diff to {}", path.display())),
        Err(err) => editor.set_error(format!("could not write {}: {err}", path.display())),
    }
}
