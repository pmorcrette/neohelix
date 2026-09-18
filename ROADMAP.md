# ROADMAP: Helix Fork (Org-Roam, Magit & Built-in Terminal)

## Context & Objectives
You are developing a custom fork of Helix in Rust. The goal is to integrate:
1. **Org-Roam v2 clone**: In-memory knowledge graph using `petgraph` without C SQLite dependencies.
2. **Magit clone**: Interactive Git client with transient menus, diff navigation, line-level staging, and Tree-sitter syntax highlighting.
3. **Built-in Terminal**: Integrated PTY terminal using `portable-pty` and `alacritty_terminal`.

---

## Phase 1: Org-Mode & Org-Roam (`crates/helix-roam`)

### Task 1.1: Tree-sitter Org Integration
- [ ] Update `languages.toml` in Helix to include `tree-sitter-org`.
- [ ] Add queries for highlights (`highlights.scm`) and folds (`folds.scm`).
- [ ] Verify that `.org` files parse correctly and support section folding.

  The folding half of this item cannot be finished here: Helix has no folding
  at all, and nothing reads a `folds.scm`. The query is written and correct,
  but inert until Task 1.4 gives the editor somewhere to use it.

### Task 1.2: Internal Crate `helix-roam` & Data Model
- [ ] Create `crates/helix-roam` in the workspace.
- [ ] Add `petgraph` and `uuid` to `crates/helix-roam/Cargo.toml`.
- [ ] Implement `Node` struct (`id`, `title`, `file_path`, `tags`, `aliases`).
- [ ] Implement `Link` enum (`Id`, `Ref`) and `RoamGraph` struct wrapping `petgraph::DiGraph`.
- [ ] Add thread-safe methods to query incoming backlinks and outgoing links in $O(1)$.

### Task 1.3: Background Indexer & UI Integration
- [ ] Implement an async directory scanner (`tokio::task::spawn_blocking`) to parse `.org` files and populate `RoamGraph`.
- [ ] Connect `RoamGraph` to the `Editor` state in `helix-view`.
- [ ] Extend `Picker` in `helix-term/src/ui/picker.rs` to create `:roam-node-find`.
- [ ] Add a sidebar/popup widget to display backlinks for the active buffer.
- [ ] Hook graph re-indexing to `Document::save` events.

### Task 1.4: Section Folding

Task 1.1 shipped `folds.scm`, but nothing reads it: Helix has no folding at
all — no fold command, and no code anywhere consuming a folds query. The
`.org` folds file is inert, so "support section folding" is not currently
true, however correct the query is.

Folding is an editor-wide feature, not an Org one, which is what makes this
expensive: it touches the view, the rendering of line numbers and gutters,
and every command that counts lines.

- [ ] Decide whether to implement folding in the fork or wait for upstream.
      Upstream Helix has wanted it for years; carrying our own is a permanent
      merge cost on the same files as Phase 5.
- [ ] A fold model on the document: which ranges are folded, surviving edits.
- [ ] Rendering: collapsed ranges, a marker, and correct line numbers.
- [ ] Commands and bindings, including Org's visibility cycling (`TAB` on a
      headline, `S-TAB` for the whole buffer).
- [ ] Feed it from `folds.scm`, so every language gets it and not just Org.

### Task 1.5: Org Structure and Metadata Editing

Nothing edits Org structure today: the fork parses `.org` files and indexes
them, but a headline is only ever plain text to the editor. These are the
commands that make Org feel like Org rather than like a text file with stars.

- [ ] Structure: insert a headline at the same level, promote and demote a
      headline, promote and demote a whole subtree, move a subtree up and down.
- [ ] TODO state cycling, honouring `#+TODO:` keyword sequences rather than a
      hardcoded TODO/DONE pair.
- [ ] Priority cookies (`[#A]`): set, raise, lower, remove. The parser already
      strips them from titles, so the reading half exists.
- [ ] Tags: add and remove on a headline, with completion from tags already in
      the graph.
- [ ] `SCHEDULED:` and `DEADLINE:` timestamps: insert, edit, and a way to pick
      a date that is not typing it by hand.
- [ ] Refile a subtree to another file or headline, and archive one.

### Task 1.6: Tables, Lists and Checkboxes

- [ ] Plain lists: insert an item, renumber an ordered list, promote and demote
      an item.
- [ ] Checkboxes (`- [ ]`): toggle, and update the statistics cookie
      (`[2/5]`, `[40%]`) on the parent.
- [ ] Tables: re-align on edit, move between cells and rows, insert and delete
      rows and columns.
- [ ] Table formulas are a language of their own and are deliberately not in
      this task; decide separately whether the fork wants them at all.

### Task 1.7: The Agenda

The single largest thing Org gives that the fork does not, and the reason many
people use Org at all. It needs Task 1.5's timestamps to exist first.

- [ ] Parse `SCHEDULED:`, `DEADLINE:`, plain and repeating timestamps into a
      date model, including repeaters (`+1w`, `.+1m`) and ranges.
- [ ] An agenda buffer: a day and a week view, built from the whole notes
      directory rather than the open file.
- [ ] A global TODO list, filtered by keyword, tag and priority.
- [ ] Jump from an agenda line to its headline, and act on it in place
      (change state, reschedule) without losing the agenda.
- [ ] Decide where the agenda lives on screen — it is another pane, so it
      shares Phase 5's blocker.

### Task 1.8: Org-Roam Beyond the Graph

The graph, the picker and the backlinks panel exist. What is missing is
everything that *writes* to the graph: today a node can only be created by
typing an `:ID:` drawer by hand.

- [ ] `roam-node-insert`: pick a node and insert an `[[id:…]]` link to it,
      creating the node if the title does not exist yet. This is the command
      Org-Roam users press most.
- [ ] Capture templates: create a node from a template into a configured file,
      rather than one node per file with a fixed shape.
- [ ] Daily notes: `roam-dailies` for today, a chosen date, and moving between
      them.
- [ ] `roam-ref-find`: the graph already indexes `:ROAM_REFS:` and can resolve
      them, but nothing exposes a search over them.
- [ ] Add and remove aliases and tags on the node at point, keeping the
      property drawer and the graph in step.
- [ ] Unlinked references: occurrences of a node's title or alias in other
      files that are not yet links, and a way to turn one into a link.
- [ ] Renaming a node's title, updating the link descriptions that named it.

### Task 1.9: Export, Babel and Clocking

The far horizon: large, self-contained, and none of it needed for the notes
workflow the fork is built around. Listed so the gap is explicit rather than
forgotten.

- [ ] Export to HTML, Markdown and LaTeX.
- [ ] Source block execution (Babel), which is an arbitrary-code-execution
      surface and needs a trust decision before a single line is written.
- [ ] Clocking: clock in and out, `:LOGBOOK:` drawers, and time reports.
- [ ] Attachments and column view.

---

## Phase 2: Magit Client (`crates/helix-magit`)

### Task 2.1: Git Engine & Diff Data Model
- [ ] Create `crates/helix-magit` in the workspace.
- [ ] Integrate `git2-rs` or `gix` (Gitoxide) for repository operations.
- [ ] Implement diff parser to extract structural AST: `FileDiff`, `DiffHunk`, `DiffLine`.

### Task 2.2: Transient Menu System
- [ ] Create `TransientMenu`, `TransientGroup`, `TransientArgument`, and `TransientAction` models.
- [ ] Implement `TransientOverlay` component in `helix-term` to render transient menus.
- [ ] Support modal keybindings for toggling switches (`--autostash`, `--interactive`) and executing actions.
- [ ] Implement default menu constructors for `Commit`, `Rebase`, `Push`, and `Pull`.

### Task 2.3: `DiffView` Component & Line-Level Staging
- [ ] Create `DiffView` component in `helix-term` supporting hierarchical navigation (File -> Hunk -> Line).
- [ ] Implement `Tab` key behavior to collapse/expand files and hunks.
- [ ] Implement line-level patch generation algorithm for staging (`s`) and unstaging (`u`).
- [ ] Render diff text using Helix's native Tree-sitter highlighter and active theme (`Theme`/`Style`).

### Task 2.4: Executing the Transient Commands

The transient menus resolve a git command line from their switches and options,
but running it is deliberately not implemented: actions report the command they
would run instead. Executing them needs a message editor, credentials, progress
and async process handling, none of which the menu system itself covers.

- [ ] Run a resolved command line asynchronously, on a job, so the editor never
      blocks on git; stream stdout/stderr and surface failures in the status
      buffer rather than discarding them.
- [ ] `Commit`: open a scratch buffer seeded with the commit template and the
      status comment block, and commit when it is written and closed — with a
      way to abort that leaves the index untouched.
- [ ] `Commit --amend` / `Extend` / `Fixup`: seed the buffer from HEAD's
      message, and refuse to amend a pushed commit without confirmation.
- [ ] `Push` / `Pull` / `Fetch`: decide between `gix`'s own transport and
      invoking the `git` binary for network operations. `gix` keeps the fork
      free of a `git` dependency, but the user's credential helpers, SSH agent
      configuration and `~/.gitconfig` `url.*.insteadOf` rules come for free
      only with the binary. This decision gates the whole group.
- [ ] `Rebase`: `--interactive` needs `GIT_SEQUENCE_EDITOR` pointed back at
      Helix, which means the integrated terminal or a spawned instance; decide
      which before starting.
- [ ] Refresh the `DiffView` after any command that changes the index, HEAD or
      the working tree.
- [ ] Guard destructive actions (`--force`, `branch -d`, `rebase --abort`)
      behind a confirmation that names what will be lost.

### Task 2.5: The Rest of the Status Buffer

The status buffer shows two sections, Unstaged and Staged. Magit shows the
repository's whole situation, and the missing sections are the ones that tell
you whether you need to push, pull or recover something. Untracked files are
already handled — they appear as whole-file additions under Unstaged — but
they have no section of their own.

- [ ] A head section: the current branch, its upstream, and the message of
      `HEAD`.
- [ ] Unpushed and unpulled sections: commits the upstream does not have, and
      commits it has that the working copy does not.
- [ ] A stashes section, listing entries and letting one be shown.
- [ ] A recent-commits section.
- [ ] An untracked section of its own, separate from unstaged changes.
- [ ] In-progress state: a merge, a rebase or a cherry-pick under way is what a
      user most needs the status buffer to tell them, and it currently says
      nothing.

### Task 2.6: Discarding and Bulk Staging

Three holes in the daily loop the fork otherwise covers end to end.

- [ ] Discard (`x`): throw away a hunk, a line selection or a whole file's
      changes. The patch machinery for this already exists — it is the same
      reverse-apply staging uses — but discarding cannot be undone, so it needs
      the confirmation Task 2.4 introduced.
- [ ] Stage and unstage everything (`S`, `U`).
- [ ] Visit the file at point (`RET`), landing on the line under the cursor
      rather than at the top of the file.

### Task 2.7: Interactive Rebase

Deferred from Task 2.4 with the blocker already identified: every git
subprocess runs with `GIT_SEQUENCE_EDITOR=true`, so that nothing can hang the
editor waiting on a terminal that is not there. That setting accepts the
todo-list unchanged, which means `rebase --interactive` currently runs but
cannot be steered.

- [ ] Decide how git reaches back into Helix for the todo-list: the integrated
      terminal, a spawned instance, or an edit-server the fork runs. This gates
      the rest, and the same mechanism would serve `commit --verbose` and any
      other command wanting an editor.
- [ ] A todo-list buffer: reorder commits, and set pick, reword, edit, squash,
      fixup and drop.
- [ ] Drive a rebase that stops: show why it stopped, and offer continue, skip
      and abort from the status buffer.
- [ ] `--autosquash`, so the `Fixup` action Task 2.4 already produces has
      something that consumes it.

### Task 2.8: The Log

Magit's second buffer, and the one the fork has no equivalent of at all.

- [ ] A log buffer: a commit list with the graph, refs and dates.
- [ ] Show a commit: its message and its diff, reusing the `DiffView` rendering
      rather than a second implementation.
- [ ] Filter by file, author, range and free text.
- [ ] Act on the commit at point: cherry-pick, revert, reset to it, start an
      interactive rebase from it, all of which depend on Tasks 2.7 and 2.9.

### Task 2.9: The Remaining Transients

What is left of Magit's dispatch menu. Each is a transient the existing menu
system can already express and a command line `resolve` can already build, so
these are mostly breadth rather than new mechanism — with the exceptions noted.

- [ ] Stash: save, pop, apply, drop, and stash only the index or only the
      worktree.
- [ ] Merge, with `--no-ff`, `--squash` and abort; conflict resolution is its
      own problem and is not covered by this item.
- [ ] Reset: soft, mixed and hard, with hard behind a confirmation.
- [ ] Tag: create, delete and push tags.
- [ ] Cherry-pick and revert, including their continue and abort states.
- [ ] Remote: add, rename, remove, and set a branch's upstream. This also
      supplies the remote picker Task 2.4 left missing, which is what
      `PushElsewhere` is waiting on.
- [ ] Bisect: start, good, bad, reset, and show where it is.
- [ ] Worktrees and submodules.
- [ ] Apply and format patches (`am`, `format-patch`).
- [ ] A process buffer: every command the fork has run and what it printed.
      Task 2.4 shows only the first useful line in the status line, so the rest
      of git's output is currently discarded.

---

## Phase 3: Integrated Terminal (`crates/helix-pty`)

### Task 3.1: PTY Engine & VT100 Emulator
- [ ] Create `crates/helix-pty` in the workspace.
- [ ] Add `portable-pty` and `alacritty_terminal` dependencies.
- [ ] Implement `PtyTerminal` wrapper managing shell spawning (`SHELL`), PTY I/O, and grid state thread-safety (`Arc<Mutex<Term>>`).
- [ ] Set up background reader thread forwarding PTY ANSI output to Alacritty's processor.

### Task 3.2: `TerminalView` UI Component
- [ ] Implement `TerminalView` component in `helix-term`.
- [ ] Translate Alacritty grid cells to Helix `Surface` rendering calls.
- [ ] Implement full key pass-through from `crossterm` to the PTY writer.
- [ ] Implement escape key sequence (e.g., `Ctrl-a Esc`) to toggle focus back to Helix normal mode.
- [ ] Handle dynamic terminal resizing (`Pty::resize`).

---

## Phase 4: Commands & Keybindings

### Task 4.1: Command Registration & Shortcuts
- [ ] Register commands in `helix-term/src/commands.rs`:
  - `:roam-node-find`, `:roam-backlinks`
  - `:magit`
  - `:terminal`
- [ ] Add default space-leader keybindings in `helix-term/src/keymap/default.rs`:
  - `space + n + f` -> Roam Find Node
  - `space + n + b` -> Roam Toggle Backlinks
  - `space + m`     -> Open Magit Status
  - `space + t`     -> Open Terminal

  Upstream Helix already binds `space + r` (`rename_symbol`) and `space + g`
  (`changed_file_picker`), so the originally planned `space + r + f`,
  `space + r + b` and `space + g + m` are not available without displacing
  them. Keeping every upstream binding untouched is a deliberate constraint:
  it preserves a Helix user's muscle memory and keeps the keymap diff against
  upstream purely additive, so future merges have nothing to conflict with.
  The fork's commands therefore sit on keys the default keymap leaves free —
  `n` for notes, `m` for Magit, `t` for the terminal.

---

## Phase 5: Docked Panes (`helix-view` layout)

### Task 5.1: Non-document Panes in the `Tree`

The Org-Roam backlinks panel, the Magit status buffer and the terminal are all
rendered as compositor overlays: they sit on top of the editor rather than
beside it, so they cover the document instead of making room next to it. Every
one of them wants to be a real pane, and they are blocked by the same thing.

`helix-view::tree::Content` has exactly two shapes, `View` and `Container`, and
a `View` is hard-bound to a `DocumentId`. A pane that is not a document
therefore cannot become a node of the tree at all. `EditorView::render`
iterates `tree.views()` and draws each node as a document, which assumes the
same thing.

Doing this once serves all three components, and it is the last structural
difference between the fork's UI and a native one.

- [ ] Add a third `Content` variant for a non-document pane, keeping
      `Tree::views()` returning only document views so the existing call sites
      do not silently change meaning.
- [ ] Teach the tree's traversal, focus and geometry about the new variant:
      `jump_view_left/right/up/down`, `rotate_view`, `transpose_view`,
      `swap_view_*`, `wclose` and `wonly`.
- [ ] Give panes a minimum and a preferred size, and have `Tree::recalculate`
      honour them rather than splitting a container's area evenly.
- [ ] Dispatch rendering per node kind in `EditorView::render`.
- [ ] Route keys to the focused pane instead of the document keymap, and define
      what `Esc` and the window commands mean while a pane has focus.
- [ ] Decide what closing the last document view means while a pane is open.
- [ ] Port the three overlays onto panes: backlinks, Magit status, terminal.

This is the one task in the roadmap that rewrites a file the fork currently
takes from upstream untouched. `tree.rs` and `ui/editor.rs` are both actively
maintained upstream, so this trades a native-feeling UI for a permanent merge
cost — worth deciding explicitly before starting, and worth keeping the diff as
narrow as the list above allows.
