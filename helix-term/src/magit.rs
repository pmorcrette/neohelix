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
        Requirement::CommitMessage { amend } => compose(cx, plan, workdir, amend),
    }
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
