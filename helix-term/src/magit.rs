//! Running Magit's git commands from the editor.
//!
//! Everything here happens off the editor's thread: the command runs on a
//! blocking task and the result comes back as a compositor callback, so a
//! slow push never freezes Helix.

use std::path::PathBuf;

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
        Requirement::CommitMessage { amend } => {
            // The views cover the editor; the message must be seen.
            close_views(compositor);
            compose(cx, plan, workdir, amend)
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

/// Asks the plan's questions one after the other, then runs it with the
/// answers. A question the menu's target already answered is skipped.
fn ask_next(
    compositor: &mut Compositor,
    editor: &Editor,
    plan: Plan,
    workdir: PathBuf,
    asks: Vec<Ask>,
    mut answers: Vec<String>,
) {
    // A suggested preset — the file an ignore pattern starts from, since
    // `junk.log` is as likely to become `*.log` — is asked with it filled
    // in; any other preset is the answer.
    while let Some(preset) = asks
        .get(answers.len())
        .filter(|ask| !ask.suggested)
        .and_then(|ask| ask.preset.clone())
    {
        answers.push(preset);
    }
    let Some(ask) = asks.get(answers.len()).cloned() else {
        let plan = plan.answered(&answers);
        if plan.destructive {
            confirm_then_run(compositor, plan, workdir);
        } else {
            spawn_run(plan, workdir);
        }
        return;
    };

    let names = helix_magit::refs::names(&workdir, ask.kind);
    let label = if ask.optional {
        format!("{}: ", ask.label)
    } else {
        format!("{} (required): ", ask.label)
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
            let mut answers = answers.clone();
            answers.push(input);
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
    spawn_run(plan, workdir);
}

/// The running half of [`run`], for callers without a context.
fn spawn_run(plan: Plan, workdir: PathBuf) {
    let line = plan.command_line();
    tokio::task::spawn_blocking(move || {
        let outcome = command::run_plan(&workdir, &plan);
        crate::job::dispatch_blocking(move |editor, compositor| match outcome {
            Ok(output) => report(editor, compositor, &line, output),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                editor.set_error("git was not found on PATH".to_string());
            }
            Err(err) => editor.set_error(format!("{line}: {err}")),
        });
    });
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
pub fn close_views(compositor: &mut Compositor) {
    compositor.remove(DiffView::COMMIT_ID);
    compositor.remove(crate::ui::log_view::LogView::ID);
    compositor.remove(DiffView::ID);
}

/// Shows the outcome and brings the status buffer back in step.
fn report(editor: &mut Editor, compositor: &mut Compositor, line: &str, output: GitOutput) {
    record(line, &output);
    if output.success {
        editor.set_status(format!("{line}: {}", output.summary()));
    } else {
        // git's own diagnosis is more useful than anything invented here.
        editor.set_error(format!("{line}: {}", output.summary()));
    }

    // The index, HEAD, the working tree or the refs may all have moved.
    for id in [DiffView::ID, DiffView::REFS_ID, DiffView::CHERRIES_ID] {
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
