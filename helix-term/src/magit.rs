//! Running Magit's git commands from the editor.
//!
//! Everything here happens off the editor's thread: the command runs on a
//! blocking task and the result comes back as a compositor callback, so a
//! slow push never freezes Helix.

use std::path::PathBuf;

use helix_magit::command::{self, GitCommand, GitOutput};
use helix_magit::{Plan, Requirement};
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
        Requirement::BranchName => ask_for(compositor, plan, workdir, "Branch: "),
        Requirement::Remote => ask_for(compositor, plan, workdir, "Remote: "),
        Requirement::CommitMessage { amend } => {
            // The views cover the editor; the message must be seen.
            close_views(compositor);
            compose(cx, plan, workdir, amend)
        }
        Requirement::Revision => ask_for(compositor, plan, workdir, "Reset to: "),
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
                // The views cover the editor; the list must be seen.
                close_views(compositor);
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
    let question = format!("{}? (y/N)", plan.summary);
    let detail = plan.command_line();

    compositor.push(Box::new(Confirm::new(question, detail, move |cx| {
        run(cx, plan, workdir);
    })));
}

/// Reads the missing word, then runs the plan with it appended.
fn ask_for(compositor: &mut Compositor, plan: Plan, workdir: PathBuf, label: &'static str) {
    let prompt = crate::ui::Prompt::new(
        label.into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| {
            if event != crate::ui::PromptEvent::Validate || input.trim().is_empty() {
                return;
            }
            let mut plan = plan.clone();
            plan.args.push(input.trim().to_string());
            plan.requirement = Requirement::None;

            if plan.destructive {
                // The confirmation needs the compositor, which a prompt
                // callback does not have, so it goes through a job.
                let workdir = workdir.clone();
                cx.jobs.callback(async move {
                    Ok(Callback::EditorCompositor(Box::new(
                        move |_editor: &mut Editor, compositor: &mut Compositor| {
                            confirm_then_run(compositor, plan, workdir);
                        },
                    )))
                });
            } else {
                run(cx, plan, workdir.clone());
            }
        },
    );
    compositor.push(Box::new(prompt));
}

/// Runs the command and reports what git said.
fn run(cx: &mut Context, plan: Plan, workdir: PathBuf) {
    let command = GitCommand::new(workdir, plan.args.clone());
    let line = plan.command_line();
    cx.editor.set_status(format!("Running {line}…"));

    cx.jobs.callback(async move {
        let outcome = tokio::task::spawn_blocking(move || command.run()).await;

        Ok(Callback::EditorCompositor(Box::new(
            move |editor: &mut Editor, compositor: &mut Compositor| match outcome {
                Ok(Ok(output)) => report(editor, compositor, &line, output),
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => {
                    editor.set_error("git was not found on PATH".to_string());
                }
                Ok(Err(err)) => editor.set_error(format!("{line}: {err}")),
                Err(err) => editor.set_error(format!("{line}: {err}")),
            },
        )))
    });
}

/// Closes the status, log and commit views, which cover the whole editor,
/// before something in the editor itself has to be seen.
pub fn close_views(compositor: &mut Compositor) {
    compositor.remove(DiffView::COMMIT_ID);
    compositor.remove(crate::ui::log_view::LogView::ID);
    compositor.remove(DiffView::ID);
}

/// Shows the outcome and brings the status buffer back in step.
fn report(editor: &mut Editor, compositor: &mut Compositor, line: &str, output: GitOutput) {
    if output.success {
        editor.set_status(format!("{line}: {}", output.summary()));
    } else {
        // git's own diagnosis is more useful than anything invented here.
        editor.set_error(format!("{line}: {}", output.summary()));
    }

    // The index, HEAD or the working tree may all have moved.
    if let Some(view) = compositor.find_id::<DiffView>(DiffView::ID) {
        view.refresh(editor);
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
fn compose(cx: &mut Context, plan: Plan, workdir: PathBuf, amend: bool) {
    if amend && command::head_is_pushed(&workdir) {
        // Amending a commit that is already on the remote rewrites history
        // someone else may have; that is worth stopping for.
        cx.editor
            .set_error("HEAD is already pushed; amending it would rewrite published history");
        return;
    }

    let message_path = workdir.join(".git").join("COMMIT_EDITMSG");
    let template = command::commit_template(&workdir, amend);

    if let Err(err) = std::fs::write(&message_path, &template) {
        cx.editor
            .set_error(format!("could not write the commit message: {err}"));
        return;
    }

    // Opened in a split rather than in place: closing the message buffer must
    // return to what the user was doing, and `:wq` on the only buffer would
    // quit Helix before the commit could run.
    if let Err(err) = cx
        .editor
        .open(&message_path, helix_view::editor::Action::HorizontalSplit)
    {
        cx.editor
            .set_error(format!("could not open the commit message: {err}"));
        return;
    }

    cx.editor.pending_commit = Some(PendingCommit {
        message_path,
        args: plan.args,
        working_directory: workdir,
    });
    cx.editor
        .set_status("Write the message, then `:w` to commit (`:q!` aborts)");
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

        let mut args = pending.args;
        // `-F` keeps the subprocess from wanting an editor of its own.
        args.push("-F".into());
        args.push(message_path.to_string_lossy().into_owned());

        let outcome = GitCommand::new(&pending.working_directory, args.clone()).run();
        let line = format!("git {}", args[..args.len() - 2].join(" "));

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
    use super::has_base;

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
