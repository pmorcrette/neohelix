//! Running Magit's git commands from the editor.
//!
//! Everything here happens off the editor's thread: the command runs on a
//! blocking task and the result comes back as a compositor callback, so a
//! slow push never freezes Helix.

use std::path::PathBuf;

use helix_magit::command::{GitCommand, GitOutput};
use helix_magit::{Plan, Requirement};
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
        // Composing a message is the caller's business; it needs a buffer
        // rather than a one-line prompt.
        Requirement::CommitMessage { .. } => {
            cx.editor
                .set_error("Composing a commit message is not wired up yet");
        }
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
