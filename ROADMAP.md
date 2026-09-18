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
