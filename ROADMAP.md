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
