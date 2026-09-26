# ROADMAP: Helix Fork (Org-Roam, Magit & Built-in Terminal)

## Context & Objectives
You are developing a custom fork of Helix in Rust. The goal is to integrate:
1. **Org-Roam v2 clone**: In-memory knowledge graph using `petgraph` without C SQLite dependencies.
2. **Magit clone**: Interactive Git client with transient menus, diff navigation, line-level staging, and Tree-sitter syntax highlighting.
3. **Built-in Terminal**: Integrated PTY terminal using `portable-pty` and `alacritty_terminal`.

---

## Phase 1: Org-Mode & Org-Roam (`crates/helix-roam`)

### Task 1.1: Tree-sitter Org Integration
- [x] Update `languages.toml` in Helix to include `tree-sitter-org`.
- [x] Add queries for highlights (`highlights.scm`) and folds (`folds.scm`).
- [x] Verify that `.org` files parse correctly and support section folding.

  Both true since Task 1.4: `folds.scm` drives the fold commands, and a
  section collapses onto its headline.


  The folding half of this item could not be finished here: at the time
  Helix had no folding at all and nothing read a `folds.scm`. Task 1.4 gave
  the editor somewhere to use it, and this query is what drives it.

### Task 1.2: Internal Crate `helix-roam` & Data Model
- [x] Create `crates/helix-roam` in the workspace.
- [x] Add `petgraph` and `uuid` to `crates/helix-roam/Cargo.toml`.
- [x] Implement `Node` struct (`id`, `title`, `file_path`, `tags`, `aliases`).
- [x] Implement `Link` enum (`Id`, `Ref`) and `RoamGraph` struct wrapping `petgraph::DiGraph`.
- [x] Add thread-safe methods to query incoming backlinks and outgoing links in $O(1)$.

### Task 1.3: Background Indexer & UI Integration
- [x] Implement an async directory scanner (`tokio::task::spawn_blocking`) to parse `.org` files and populate `RoamGraph`.
- [x] Connect `RoamGraph` to the `Editor` state in `helix-view`.
- [x] Extend `Picker` in `helix-term/src/ui/picker.rs` to create `:roam-node-find`.
- [x] Add a sidebar/popup widget to display backlinks for the active buffer.
- [x] Hook graph re-indexing to `Document::save` events.

*Tasks 1.4 to 1.16 were derived from Org's own default keymap and its
`org-modules` list, command by command, rather than from recollection. What
remains after them is Emacs integration rather than Org, and is named at the
end of Task 1.16. Tasks 1.17 to 1.20 were derived the same way from Org-Roam's
own sources: the interactive commands its modules define, its extensions, and
the schema its database stores. Task 1.21 was derived from Org's
`org-options-keywords` and `org-startup-options` declarations directly. All of
Phase 1 was then re-checked against local clones of the Org and Org-Roam
sources, which added Task 1.22 and confirmed the rest.*

### Task 1.4: Section Folding

Task 1.1 shipped `folds.scm`, but nothing reads it: Helix has no folding at
all — no fold command, and no code anywhere consuming a folds query. The
`.org` folds file is inert, so "support section folding" is not currently
true, however correct the query is.

Folding is an editor-wide feature, not an Org one, which is what makes this
expensive: it touches the view, the rendering of line numbers and gutters,
and every command that counts lines.

- [x] Decide whether to implement folding in the fork or wait for upstream.
      Upstream Helix has wanted it for years; carrying our own is a permanent
      merge cost on the same files as Phase 5.

  Decided: in the fork, and in `helix-core` rather than the Org layer. The
  cost feared above is not where it looked. The gutter is a decoration fed
  `LinePos { doc_line }` straight from the document formatter, every
  char-to-visual mapping in `position.rs` runs through that same formatter,
  and `View::text_annotations` is the single place all eleven callers build
  annotations from. Teaching the formatter to skip a range is one change, and
  line numbers, scrolling and cursor movement follow from it.

- [x] A fold model on the document: which ranges are folded, surviving edits.
- [x] Rendering: collapsed ranges, a marker, and correct line numbers.
- [x] Commands and bindings, including Org's visibility cycling (`TAB` on a
      headline, `S-TAB` for the whole buffer).

  Seven commands, all typable and all bound under `z`, on the keys Vim uses:
  `za` toggles, `zf` closes, `zo` opens, `zM` closes every fold, `zR` opens
  every fold. `zc` and `zm` are upstream's view commands, so closing took
  `zf` and `zM` — no upstream binding is moved, as Task 4.1 requires.

  Cycling is `z` then tab, and the whole buffer is `z` then shift-tab. It
  cannot be tab itself: upstream binds that to `jump_forward`. The three
  states are Org's — the subtree hidden, then its children's headlines with
  every body hidden, then everything — and the whole-buffer cycle is Org's
  overview, contents, show-all.

  Nothing remembers which state a range is in: it is read back from the folds
  themselves. A stored cycle position goes stale the moment an edit or
  another fold command changes what is closed, and there is no edit that can
  make the folds disagree with themselves.

- [x] Feed it from `folds.scm`, so every language gets it and not just Org.

  A cursor never stays inside folded text. Until Task 1.21 made files open
  folded, nothing put it there except the fold commands, which move it onto
  the marker. Opening `notes.org:12`, a search, or following a link to a
  second-level node in a file that opens in overview all can, and left the
  cursor somewhere nothing is drawn. The rule is enforced in
  `View::ensure_cursor_in_view`, which every command's effect on the cursor
  passes through: the fold hiding the cursor opens, as Vim's `zv` does. A
  cursor on a marker is not hidden, so folding from a headline does not
  undo itself — and the commands that fold without the `z` prefix (sparse
  trees, narrowing, `:org-startup-visibility`) now move the cursor onto the
  marker the way the `z` commands always did, or the next redraw would reopen
  what they just closed.

*Three things the editor caught that the library tests did not. A fold is
asked for from the headline above it, which is **before** the first hidden
character, so a fold that could only be found at its exact start could be
closed from a line and never opened from it again. Folding everything means
folding to the top level: handing every nested range to a model where a later
fold replaces the one containing it folds the file to its leaves, which hides
almost nothing. And showing "just the children" has to hide the body as well
as collapse the children, or the entry shows its prose while claiming to show
only its children.*

*Where this differs from Emacs, and knowingly: the three states are derived
from the fold hierarchy `folds.scm` gives, not from Org's headline structure.
For an ordinary outline the two agree — a section's direct children are its
subsections. For a section whose body opens with a list or a table, that list
counts as a child, so the "children" state collapses it instead of hiding it
with the rest of the body. Org's own reader was not reachable to check the
exact rule against (see Task 1.13's note), and this is the deviation to look
at first when it is.*

### Task 1.5: Org Structure and Metadata Editing

Nothing edits Org structure today: the fork parses `.org` files and indexes
them, but a headline is only ever plain text to the editor. These are the
commands that make Org feel like Org rather than like a text file with stars.

- [x] Structure: insert a headline at the same level, promote and demote a
      headline, promote and demote a whole subtree, move a subtree up and down.
- [x] TODO state cycling, honouring `#+TODO:` keyword sequences rather than a
      hardcoded TODO/DONE pair.
- [x] Priority cookies (`[#A]`): set, raise, lower, remove. The parser already
      strips them from titles, so the reading half exists.
- [x] Tags: add and remove on a headline, with completion from tags already in
      the graph.
- [x] `SCHEDULED:` and `DEADLINE:` timestamps: insert, edit, and a way to pick
      a date that is not typing it by hand.
- [x] Refile a subtree to another file or headline, and archive one.

### Task 1.6: Tables, Lists and Checkboxes

- [x] Plain lists: insert an item, renumber an ordered list, promote and demote
      an item.
- [x] Checkboxes (`- [ ]`): toggle, and update the statistics cookie
      (`[2/5]`, `[40%]`) on the parent.
- [x] Tables: re-align on edit, move between cells and rows, insert and delete
      rows and columns.
- [ ] Table formulas are a language of their own and are deliberately not in
      this task; decide separately whether the fork wants them at all.

### Task 1.7: The Agenda

The single largest thing Org gives that the fork does not, and the reason many
people use Org at all. It needs Task 1.5's timestamps to exist first.

- [x] Parse `SCHEDULED:`, `DEADLINE:`, plain and repeating timestamps into a
      date model, including repeaters (`+1w`, `.+1m`) and ranges.
- [x] An agenda buffer: a day and a week view, built from the whole notes
      directory rather than the open file.
- [x] A global TODO list, filtered by keyword, tag and priority.
- [x] Jump from an agenda line to its headline, and act on it in place
      (change state, reschedule) without losing the agenda.

  Jumping works. Acting *in place* did not, because Helix's `Picker`
  consumes its own keys and offers no hook for an action that leaves it
  open. The way out turned out to need neither a patch to the upstream
  file nor a whole agenda component. The fork's `AgendaView` wraps the
  picker, takes the few keys it acts on before the picker sees them, and
  rebuilds the picker afterwards, on the same entry. The keys are Org
  agenda's letters on Alt, which the picker does not use: `Alt-t`/`Alt-T`
  state, `Alt-s` schedule, `Alt-d` deadline, `Alt-+`/`Alt--` priority,
  `Alt-i` clock in. The status line lists them when the agenda opens.

  An action edits the entry's file without opening it: its buffer if it is
  open (left unsaved), else the file on disk. The file is then re-indexed,
  which is what the rebuilt view reads. The action goes through the same
  code as the commands — logging, repeaters, `:BLOCKER:`s — and a note it
  asks for goes into that file, not into the buffer behind the agenda.

  Checked in the editor, with the agenda open throughout:
  - on a file not open, `Alt-+`, a reschedule of `+3` and `Alt-t` gave
    `[#A]`, the new date and `DONE` on disk, and the finished task left
    the list;
  - on the open file, `Alt-t` changed the buffer, marked it modified, and
    left the disk alone.

  The picker's typed filter is lost when the view is rebuilt.
- [x] Decide where the agenda lives on screen — it is another pane, so it
      shares Phase 5's blocker.

  Decided for now: a picker overlay, like the node and ref pickers. That makes
  it searchable and jumpable immediately, at the cost of not being readable
  beside a document. A docked agenda is Phase 5's to give.
- [x] Manage which files the agenda reads: add and remove them, cycle through
      them, and restrict a view to one file or subtree.

### Task 1.8: Org-Roam Beyond the Graph

The graph, the picker and the backlinks panel exist. What is missing is
everything that *writes* to the graph: today a node can only be created by
typing an `:ID:` drawer by hand.

- [x] `roam-node-insert`: pick a node and insert an `[[id:…]]` link to it,
      creating the node if the title does not exist yet. This is the command
      Org-Roam users press most.
- [x] Capture templates: create a node from a template into a configured file,
      rather than one node per file with a fixed shape.
- [x] Daily notes: `roam-dailies` for today, a chosen date, and moving between
      them.
- [x] `roam-ref-find`: the graph already indexes `:ROAM_REFS:` and can resolve
      them, but nothing exposes a search over them.
- [x] Add and remove aliases, tags *and refs* on the node at point, keeping the
      property drawer and the graph in step.
- [x] Open a random node, which is how a large set of notes gets revisited.
- [x] Dailies come in two forms upstream, and the difference matters: *goto*
      opens the day's note, *capture* adds an entry to it through a template
      without leaving the current buffer. Both, plus opening the dailies
      directory itself.

  *Capture* is `:roam-dailies-capture <entry>` (or a prompt without an
  argument). It appends `* <entry>` to today's note, which is Org-Roam's
  default daily template, creating the note if needed, and the cursor stays
  where it was. If today's note is open, its buffer gets the entry, unsaved;
  if not, the file does, and the index follows. `:roam-dailies-directory`
  opens a file picker there. Checked in the editor: two captures from
  `rust.org` landed in a note created for them, the cursor never left, and
  the picker listed it. Not built: capture templates other than the
  headline one.
- [x] Unlinked references: occurrences of a node's title or alias in other
      files that are not yet links, and a way to turn one into a link.
- [x] Renaming a node's title, updating the link descriptions that named it.

### Task 1.9: Export, Babel and Clocking

The far horizon: large, self-contained, and none of it needed for the notes
workflow the fork is built around. Listed so the gap is explicit rather than
forgotten.

- [x] Export to HTML, Markdown and LaTeX.

  `:org-export md|html|latex` writes `name.md`, `name.html` or `name.tex`
  next to the file, from the buffer as it is. The three formats share one
  reader, which covers what notes are written with:
  - headlines, with their TODO keyword, priority and tags;
  - paragraphs and Org's emphasis rules (so `2*3*4` and `a/b/c` stay text);
  - links, including bare URLs and images;
  - plain, ordered, description and checkbox lists, nested;
  - tables with a header row;
  - source, example, quote, verse, center and export blocks;
  - fixed-width lines, rules and footnotes, inline ones included.

  It reads `#+OPTIONS:` for `toc`, `num`, `todo`, `tags` and `pri`, and
  `#+TITLE:`, `#+AUTHOR:`, `#+DATE:` and `#+EXCLUDE_TAGS:`. Subtrees tagged
  `:noexport:` or marked `COMMENT` are left out, as are drawers, planning
  lines and comments. A source block's `:exports` decides whether its code
  and its `#+RESULTS:` go out; by default only the code does, as in Org.

  `id:` links are what make this worth having on a notes directory; see Task
  1.20. On the test notes, `[[id:…][Ownership]]` came out as
  `[Ownership](rust.md#ownership)` and `rust.html#ownership`. A link whose
  node is not in the index keeps its text, drops the link, and is named in
  the status message. In Markdown, a heading whose shown text would give a
  renderer a different anchor (a TODO keyword, a `:CUSTOM_ID:`) carries an
  explicit `<a id>`, so the anchors links use always exist. In LaTeX, links
  to other files point at their `.pdf`.

  Checked: the exported HTML of a document using every construct has
  balanced tags. Not checked: that the LaTeX compiles and that the Markdown
  renders as intended, since neither `pdflatex` nor `pandoc` is installed
  here.

  Not read: macros, `#+INCLUDE:`, entities and sub/superscripts,
  `#+CAPTION:` and `#+ATTR_*:`, inline images' sizing, LaTeX fragments,
  `<<targets>>` as anchors, timestamps (exported as their text), and export
  of a subtree alone. Headlines deeper than a format has levels for
  (5 in LaTeX, 6 in HTML) are flattened to the deepest one rather than
  turned into lists as Org does.
- [x] Source block execution (Babel), which is an arbitrary-code-execution
      surface and needs a trust decision before a single line is written.

  **The trust decision.** A block runs only on `:org-babel-execute`, from
  the cursor. Nothing else runs one: not opening a file, not exporting it
  (Org's export evaluates blocks by default, and this fork's does not), not
  tangling. Two gates must both pass:
  - *Workspace trust.* A new `TrustQuery::CodeExecution` in Helix's
    workspace-trust system. Unlike language servers, it is not implied by
    the default `servers` level: a server is a binary the user installed,
    while a block is code someone wrote into a file, which is what
    `.helix/` config is too. So it takes the same explicit
    `:workspace-trust`, and a stale grant (changed `.helix/`) demotes it,
    as it does local config.
  - *A confirmation for every run,* naming the language, the program and
    the directory, as `org-confirm-babel-evaluate` does by default.

  Checked in the editor:
  - untrusted, the error names the workspace and `:workspace-trust`;
  - trusted and answered `n`, nothing ran and nothing was written;
  - answered `y`, a shell block with `:var` printed its variable and its
    directory, a named Python block wrote `#+RESULTS: answer` / `: 42` from
    its `return`, and a failing one wrote what it printed and reported
    `python3 exited with code 1 after 0.02 s: ValueError: bad input`;
  - `:workspace-untrust` brought the refusal back.

  How blocks run: in the background, killed after 60 seconds. Languages:
  `sh`, `bash`, `zsh`, `fish`, `python` (as `python3`), `ruby`, `perl`,
  `lua`, `node`, `awk`, `R` and `julia`. The script goes in a temporary
  file, removed afterwards. Results are written by finding the block again
  by its body, since the buffer may change while the block runs.

  Header arguments read:
  - `:results` — `output`, `value` (Python's from `return`, as Org wraps
    it; a shell's is its output; any other language says it gives its
    output instead), `silent`, `raw`;
  - `:dir`, `:cmdline`, and `:var` with numbers and quoted strings.

  Results follow Org's layout: `: ` lines, or an example block from ten
  lines on (`org-babel-min-lines-for-block-output`), replacing the old
  ones.

  Not built:
  - sessions;
  - compiled languages;
  - `:var` references to other blocks or tables;
  - tables and lists as results;
  - `:results file`, `append`, `prepend`;
  - `:cache`;
  - inline `src_lang{…}` and `call_` lines;
  - a way to turn the confirmation off;
  - a timeout set per block. The 60-second limit was not tested live.

  Also found and fixed on the way: the header-argument reader from Task
  1.15 dropped every quote, so `:var who="you"` reached the script as
  `who=you` and `:cmdline a "b c"` as three words. It now removes quotes
  only from a value that is one quoted string.
- [x] Clocking: clock in and out, `:LOGBOOK:` drawers, and time reports.

  `:org-clock-in`, `:org-clock-out`, `:org-clock-cancel`, `:org-clock-goto`
  and `:org-clock-report`. A clock is a `CLOCK:` line at the top of the
  entry's `:LOGBOOK:`, closed as `CLOCK: [a]--[b] =>  1:30`. The file is the
  only record: the running clock is whichever line has no end, so one
  started before a restart is still found. There is one clock at a time, as
  in Org, so clocking in elsewhere clocks the running one out first, even in
  another file. If that file is open, its buffer is changed and left
  unsaved; if not, the change is written on disk. All three cases were
  checked in the editor. Cancelling removes the line, and the drawer too if
  it held nothing else.

  The report is Org's `clocktable` dynamic block: `:maxlevel` (default 2),
  `:scope file|subtree`, `:block today|yesterday|thisweek|lastweek|thismonth`
  and `:tstart`/`:tend`. Clocks crossing the span's edge are clipped, so one
  running past midnight counts in both days. Each deeper level gets its own
  column, with `\_  ` indentation, as Org lays the table out. That layout,
  the caption and Monday as the first day of the week are written from
  memory of Org.

  Not built: showing the running clock in the status line (Org puts it in
  the mode line); idle detection and resolving a clock left running (Org
  asks on restart, here it simply stays running until clocked out); clock
  history; effort warnings; reports across files (`:scope agenda`); and the
  per-headline time overlays of `org-clock-display`.
- [x] Attachments and column view.

  **Attachments.** `:org-attach <file>` copies a file into the entry's
  attachment directory, `:org-attach-open` lists that directory in a file
  picker, and `[[attachment:name]]` links open from the entry they are in.
  The directory follows Org's defaults since 9.3, as I remember them: a
  `:DIR:` property, or else `data/` next to the file and then the entry's
  `:ID:` split after two characters (`data/6b/a7b810-…`). The entry gets an
  `:ID:` if it has none, and the `ATTACH` tag. Checked in the editor: the
  file was copied, the entry was tagged, and the link opened it.

  This found a bug in Task 1.10's `org-id` creation. On an entry that
  already had a property drawer, it added a *second* `:PROPERTIES:` drawer
  for the `:ID:`, which Org does not read. The id now goes into the
  existing drawer, before its `:END:`.

  Not built: other attach methods (`mv`, links, symlinks), deleting
  attachments, attaching from a URL, inheriting the directory from a parent
  entry, and `attachment:` links in export.

  **Column view.** `:org-columns` shows each headline's row after its text,
  from `#+COLUMNS:` or Org's default `%25ITEM %TODO %3PRIORITY %TAGS`.
  - *Values:* `ITEM`, `TODO`, `PRIORITY`, `TAGS`, `CLOCKSUM` (from Task
    1.9's clocks), and any property.
  - *Summaries:* on a headline with children, a summarised column shows the
    summary of its children rather than its own value: `{:}` adds
    durations, `{+}` numbers, and `{min}`/`{max}` pick one.

  Org draws the table over the headlines with overlays, which Helix does
  not have. Its virtual text does the same job here: each row is appended
  after the headline and padded so the columns line up. The view is
  recomputed on every change while it shows. Checked in the editor:
  changing `:Effort: 1:30` to `3:45` by hand moved the entry's cell and its
  parent's summary (`3:30` → `5:45`) as it was typed.

  Differences from Org:
  - *Header:* the column titles show in the status line when the view is
    switched on, not as a header line.
  - *Editing cells:* a cell is not edited in place; editing the drawer, or
    `:org-set-property`, updates it.
  - *Wide characters:* widths are counted in characters, so a headline with
    double-width characters pushes its row out of line.
  - *Not read:* `{X}` checkbox summaries, `{mean}`, time-stamp summaries,
    and `COLUMNS` as a property of a subtree.

### Task 1.10: Links

Every link the fork understands is an `[[id:…]]` link inside the Roam graph.
Org's own link system — the thing that makes an Org file more than an outline —
is absent: nothing stores a link, follows one, or knows that `file:`, `http:`
and internal targets exist.

- [x] Follow the link at point, dispatching on its type, and a mark ring so
      that following one can be undone by going back.
- [x] Store a link to the current location and insert a stored link elsewhere,
      which is how links get made in Org without typing them.
- [x] The built-in types: `file:` with a line or search target, `http(s):`,
      `mailto:`, and internal links to a headline, a `CUSTOM_ID` or a `<<target>>`.
- [x] Move to the next and previous link in the buffer.
- [x] Link abbreviations (`[[gh:owner/repo]]`), which are per-file configuration
      the parser already has to read anyway.
- [x] `org-id` proper: create an ID on demand for the entry at point, rather
      than requiring the user to type a drawer by hand. Task 1.8's
      `roam-node-insert` needs this underneath it.
- [ ] Inline preview of images and of link descriptions. Blocked for the same
      reason as Task 1.14's LaTeX preview: this wants overlays or images that
      Helix does not have, and the capability has to exist before the feature
      can.

### Task 1.11: Properties, Drawers and Logging

The parser reads property drawers, and Task 1.8 will write the two Roam
properties. Nothing edits properties in general, and nothing records what
happened to an entry.

- [x] Set, change and remove a property on the entry at point, with completion
      over the keys already used in the file.
- [x] Effort estimates, and incrementing one.
- [x] Property inheritance, which changes what a query over the graph returns
      and so belongs with the indexer rather than only the UI.
- [x] Insert a drawer, and fold drawers by default the way Org does.

  Done with Task 1.21's `#+STARTUP:`. "By default" turned out to mean less
  than it sounds: since Org 9.4 a file that declares nothing opens with
  everything showing, drawers included. Drawers close on opening under every
  other visibility option unless the file says `showdrawers`.
- [x] Logging: record state changes and timestamps into `:LOGBOOK:`, and add a
      dated note to an entry.

### Task 1.12: Sparse Trees, Narrowing and Structural Motion

Org's way of reading a large file: hide everything that does not match, or
narrow to one part of it. Both are built on Task 1.4's folding.

- [x] Sparse trees: show only the entries matching a regexp, a TODO state, a
      tag or a property query, with the rest folded away.

  `:org-sparse-tree` takes the agenda's own sigils — `:work:` a tag, `#A` a
  priority, a bare word a TODO keyword — plus `/text` for the headline's
  text. A match lights up the path down to it, so an entry three levels deep
  appears with the headings that say where it is. Everything else goes,
  bodies included, which is what makes a sparse tree a tree. A property
  query is not there: the filter matches what a headline carries, and a
  property lives in the drawer below it.

- [x] Narrow to a subtree, a block or an element, and widen again.

  `:org-narrow` takes the subtree; `:narrow-to-selection` takes whatever is
  selected, which is how a block or an element is reached without a command
  per kind — `A-o` climbs the syntax tree until it holds what you mean.
  Widening clears every fold, so it is the same operation as `:unfold-all`
  under the name Org uses.

- [x] Structural motion by element: forward, backward, up and down, which is
      what makes editing Org feel structural rather than textual.

  Already upstream, and verified on an Org file rather than assumed: `A-o`
  and `A-i` expand and shrink, `A-n` and `A-p` move between siblings, `A-b`
  and `A-e` to a parent's ends. With the org grammar built, expanding from a
  word climbs to the paragraph, then the subsection, then the section. The
  fork adds nothing here; the grammar is what makes it structural.

- [x] Heading motion: next and previous visible heading, next and previous at
      the same level, and up to the parent.

  Sibling motion stops at a shallower heading rather than running on to a
  cousin further down the file, and going up lands on the parent, which is
  not the same entry as the previous heading whenever the cursor is on a
  first child.

- [x] Jump to a heading in the current file by name.

  Over the buffer rather than the graph: this is for finding your way around
  the file you are in, headings with no `:ID:` included — they are not nodes,
  and a picker over nodes would not show them.

- [x] Show the outline path of the entry at point, for when the headline has
      scrolled off.

*Narrowing and sparse trees replace the fold set rather than adding to it: a
view of the file is a view, not a layer over the folds you had. The one thing
the editor caught that the tests did not was an off-by-one — a selection's
end is one past its last character, so on a range ending at a line break it
names the line below, and narrowing to it kept a line nobody selected.*

### Task 1.13: Subtree Clipboard, Sorting and Dynamic Blocks

- [x] Structure-aware cut, copy and paste of a subtree, where pasting adjusts
      the level to where it lands rather than pasting raw text.
- [x] Clone a subtree a number of times, shifting its timestamps — the standard
      way of creating a recurring set of entries.
- [x] Copy only the visible text of a region, so a folded outline can be shared
      as an outline. Unblocked by Task 1.4: there is now such a thing as
      visible text, and `Folds::hidden` says which characters are not.

  `org_copy_visible` (and `:org-copy-visible`) yanks each selection without
  the text folds hide, into the chosen register, `"` by default like a
  yank. A selection of one character means the whole buffer, the usual
  thing to share. A fold hides from its first line's newline to its last
  line's, so what is copied is whole lines. Checked in the editor: a file
  opened in overview copied as its two top-level headlines, and with one
  of them cycled to its children, as those headlines with the children's
  under them.
- [x] Sort entries, list items or table rows by a chosen key.
- [x] Dynamic blocks: a block whose contents are regenerated by a named
      function, and the command that refreshes it. Column view and clock
      reports are both built on this.

*Two identity rules settled while implementing this, both for the same reason
— two nodes holding one UUID is a corrupt graph. A **copy** loses its `:ID:`
and a **cut** keeps it: cutting moves an entry that links already point at,
copying makes a second one. **Clones** lose it too, and keep their inactive
timestamps: `[…]` records when something happened, and a recurring set must
not rewrite its own history. Left out: table formulas (Task 1.6's note still
stands), `#+COLUMNS:` formats and the clock reports built on them (Task 1.9).*

*Unlike the rest of Phase 1, this task could not be checked against upstream:
the egress proxy refused Savannah, the GitHub mirror and orgmode.org alike, so
the dynamic-block syntax and the clone semantics here were written from the
form Org uses rather than from Org's own reader. Worth a re-reading when a
source is reachable again.*

### Task 1.14: Markup, Footnotes and Citations

- [x] Toggle emphasis on a region: bold, italic, underline, code, verbatim,
      strike-through.

  Whitespace at the edges of the selection stays outside the markers. Org
  will not render a closing marker that follows a space, and selecting a word
  takes its trailing space in most editors — including Helix — so wrapping
  the selection as given produces markup that shows its own stars.

- [x] Structure templates, so a source or quote block is inserted rather than
      typed.

  With nothing selected it inserts an empty block and puts the cursor inside
  it; with a selection it wraps the lines. A cursor is a one-character
  selection in Helix, so "nothing selected" has to mean a span of at most one
  character rather than an empty one.

- [x] Footnotes: create, jump between the reference and the definition, and
      renumber.

  Renumbering follows the order the references appear in and leaves a named
  footnote named: a name is a name, and renumbering it would be renaming it.
  New definitions go at the end of the buffer, which is what a file without a
  `* Footnotes` heading gets from Org too.

- [x] Citations (`[cite:@key]`): insert one with completion over a bibliography,
      and follow it.

  `#+BIBLIOGRAPHY:` is read here rather than in Task 1.21, which deferred it.
  Completion offers the keys of the declared `.bib` files *and* the keys the
  graph has already seen: a notes directory often cites keys that no `.bib`
  beside it declares, because the bibliography lives with the paper. Only the
  keys are read from BibTeX — a full parser is a different piece of work.

- [ ] LaTeX fragment preview and pretty entities, both of which need an image
      or an overlay mechanism Helix does not currently have — worth checking
      before committing to them.

  Checked, and the two answers differ. **Preview is out of reach**: it needs a
  terminal graphics protocol, and Helix has none — the only `kitty` in the
  tree is the keyboard protocol. Adding one is a `helix-tui` undertaking, not
  Org work. **Pretty entities are within reach now**: since Task 1.4 a fold
  hides a range and draws a marker in its place, so `\alpha` can fold to `α`.
  What is missing is a marker *per fold*; today it is one string on
  `TextFormat` for the whole buffer. That is a small, contained change to the
  fold model, and it is the thing to do before this item rather than after.

### Task 1.15: Source Blocks as Code

The one Org feature where a code editor should beat Emacs rather than catch up
with it. `injections.scm` already highlights a source block in its own
language; what is missing is treating it as code.

- [x] Edit a source block with the full language tooling — LSP, completion,
      diagnostics, formatting — rather than only its highlighting.

  `:org-edit-src` opens the block at the cursor in a split. Writing that
  buffer puts the code back into the block; the Org buffer is changed but
  not saved, as in Org. Checked in the editor with a Python block: ruff,
  started by Helix's normal Python configuration, reported "`os` imported
  but unused" and "Undefined name" on the block's code. A Rust block made
  Helix start rust-analyzer the same way, but this environment has only
  rustup's proxy for it, so nothing came back. Editing 42 into 43 in the
  buffer and writing it put `43` back in the block with the block's own
  indentation.

- [x] Decide the mechanism: a scratch buffer bound to the block and written
      back, or making the language server see the block in place. The first is
      how Emacs does it; the second is better and harder.

  The scratch buffer, and it has to be a **file**: a language server takes a
  document by its path and picks a language by its extension, so the block
  is written to a temporary directory under the block's `#+NAME:` (or
  `block`) with the language's extension. The block is found again by its
  body, not its line, when the buffer is written back. If it changed in the
  Org buffer in the meantime, nothing is written back and the message says
  why. The temporary file goes when its buffer closes, or when the editor
  exits, since `:q` closes a view and leaves the buffer open. In place would
  mean presenting the language server with a virtual document made from
  each block and mapping every position both ways, and that is left for
  later.

  Unlike Org, this keeps the indentation the body already had rather than
  re-indenting it by `org-edit-src-content-indentation`, so editing one line
  does not re-indent the whole block. Lines starting with `*` or `#+` are
  escaped with a comma going back and unescaped coming out, as Org does.

- [x] Navigate between blocks, and between a block and its result.

  `:org-src-next`, `:org-src-previous`, and `:org-src-result`, which goes
  from a block to its `#+RESULTS:` (directly after it, or by name anywhere)
  and back, including from any line of the output. Previous from inside a
  block goes to that block's own `#+begin_src` first, as Org's backward
  search does.

- [x] Tangling: write the blocks out to their target files, which is the half of
      literate programming that needs no code execution and no trust decision.

  `:org-tangle` writes every block with a `:tangle` target. Header
  arguments are merged from `#+PROPERTY: header-args[:lang]`, then each
  enclosing subtree's `:header-args[:lang]:` drawer property, then
  `#+HEADER:` lines, then the block's own line. It handles `:tangle yes`
  (named after the Org file with the language's extension, as
  `org-babel-tangle-lang-exts` gives it), `:mkdirp`, `:shebang` (which
  also makes the file executable), `:padline`, and noweb: `<<name>>`
  expands from `#+NAME:` and `:noweb-ref`, with the reference line's
  prefix repeated on every line, only under `:noweb yes` or `tangle`. An
  unknown reference or a cycle is an error, and in that case nothing is
  written. Checked in the editor: `src/lib.rs` was created under a
  directory that did not exist, and `run.sh` came out executable with its
  noweb reference expanded.

  Not read: `:comments` (links back from the tangled file), detangling,
  `:tangle-mode`, and a `:tangle` path computed by Lisp, which is Babel.
  How `:padline` separates blocks is written from memory: one blank line
  between blocks going to the same file.

### Task 1.16: The Optional Modules Worth Having

Org ships a long list of optional modules. Most are links into Emacs
applications and mean nothing here; these are the ones that do.

- [x] Habits: a repeating task with a consistency graph in the agenda.

  An entry with `:STYLE: habit` and a repeating `SCHEDULED:` is a habit,
  and a maximum after the repeater is read (`.+2d/4d`: due two days after
  the last time, late after four). Its history is the completions in its
  logbook (`State "DONE" …`), which Task 1.21 writes by default when a
  repeating task is marked done. The agenda has a habit column showing
  Org's default window, 21 days back and 7 ahead. Each day is `*` where it
  was done, `!` for today, `·` otherwise, and is coloured by how due the
  habit was that day, from the theme's `hint`, `diff.plus`, `warning` and
  `error` (Org's blue, green, yellow and red).

  How due a past day was follows from the last completion before it; days
  to come follow `SCHEDULED:`. The graph is read from the file, since the
  index keeps no logbook: from the buffer if it is open, else from disk.
  Checked in the editor: completions on the 12th, 17th and 23rd landed
  where they should, and `!` was on today. The colours were not checked:
  the test terminal records characters, not their attributes.

  Not built: Org's option to show habits only on today's line (a habit
  appears on each day its repeater lands on this week), the habit-specific
  agenda sorting, and hiding habits from the agenda. Months and years count
  as 30 and 365 days in the graph.
- [x] TODO dependencies: an entry that cannot be done before its children or a
      named other entry.

  Marking an entry done is refused, with the reason, when:
  - *a child is still open,* if the new `todo-dependencies` setting is on
    (Org's `org-enforce-todo-dependencies`, off by default there too);
  - *an earlier sibling is open under an `:ORDERED:` parent,* including
    through a parent's own ordering;
  - *an entry named in `:BLOCKER:` is open.* The property is read in
    `org-depend`'s form (`id1 id2`, `previous-sibling`) and in `org-edna`'s
    (`ids(…)`). A named entry is looked up in the index, since it can be in
    any file, and one the index does not know blocks rather than passing
    unchecked.

  `:ORDERED:` and `:BLOCKER:` apply whatever the setting says, which is a
  choice: they are written into the entry, and a property that blocked
  nothing would be misleading. Reopening is never blocked. Checked in the
  editor: `Blocked by "Prerequisite"` across a `:BLOCKER:`, and
  `Blocked by the earlier "One"` under `:ORDERED:`. Not read: the rest of
  `org-edna`'s language (triggers, `children`, conditions), and greying
  out blocked entries in the agenda.
- [x] Inline tasks: a task that does not break the outline it sits in.

  A headline of 15 stars or more is an inline task (Org's
  `org-inlinetask-min-level`), optionally closed by a line of the same
  stars and `END`.
  - *To the outline, it is not a headline.* The text after it still belongs
    to the entry above, so subtrees, folding on open, sparse trees, the
    heading list, column view, clock attribution and TODO dependencies all
    pass over it, and the `END` line is never read as a headline called
    "END".
  - *To what edits the entry at the cursor, it is its own entry,* from its
    line to its `END`: cycling a state or setting a priority there changes
    the inline task. After the `END`, the entry is the headline above
    again. This is one function (`entry_start`), which replaced twelve
    hand-written "headline above the cursor" lookups.
  - *Export* draws it apart from the body, as Org's exporters do: a
    `<div class="inlinetask">` with the task in bold.

  `:org-inline-task` inserts one with its `END` line, and no keyword, as
  Org's `org-inlinetask-default-state` does. Checked in the editor:
  inserting and cycling it, and a `#+STARTUP: content` file keeping it
  hidden in its entry's body. Not handled: `z` folding follows the
  tree-sitter grammar's sections, and that grammar knows nothing of inline
  tasks, so it folds one as a section of its own.
- [x] Encrypted subtrees.

  Org's `org-crypt`, as of 9.4: `:org-encrypt-entry`, `:org-encrypt-entries`
  (every `:crypt:` entry in clear) and `:org-decrypt-entry`. The body is
  encrypted: everything below the headline's planning line and property
  drawer, through the end of the subtree, children included. The
  headline, dates and properties stay readable, so an encrypted note keeps
  its `:ID:` and stays a node. Entries are encrypted to `:CRYPTKEY:` (or a
  file-wide `#+PROPERTY: CRYPTKEY`), and with a passphrase when there is no
  key.

  **The terminal problem, and what was done about it.** gpg asks for a
  passphrase through its agent's pinentry, and a terminal pinentry draws
  over the editor. So the passphrase is asked for in the editor instead:
  - in a new *masked* prompt mode, which draws `*` per character and keeps
    no history;
  - asked twice when encrypting, since a typo would lose the text;
  - handed to gpg as the first line of its input (`--pinentry-mode
    loopback --passphrase-fd 0`), never on the command line, where any
    user could read it, and never in a file.

  **What differs from Org.** Org re-encrypts `:crypt:` entries before every
  save. Here a save cannot ask for a passphrase, so `:w` (and `:wa`)
  refuses to write a `:crypt:` entry in clear and says so; `:w!` writes it
  anyway. Keys gpg does not trust are refused as gpg refuses them: no
  `--trust-model always`.

  Checked in the editor:
  - `:w` was refused with the entry in clear, and the passphrase showed as
    `*******`;
  - after encrypting and saving, the file held no plain text and kept its
    `:ID:`;
  - a wrong passphrase gave `decryption failed: Bad session key`, and the
    right one restored the body;
  - `:w` was refused again until `:w!`, and the round trip gave back the
    original file byte for byte;
  - encrypting to a key asked for nothing, and decrypting with an empty
    passphrase for a key that has none worked.

  Not handled: the passphrase `String` is dropped rather than wiped from
  memory, since the fork has no zeroizing dependency; gpg-agent's own
  passphrase cache is not used, so a passphrase is asked for every
  decryption; and auto-save still goes through the plain save path.
- [x] A protocol handler, so a browser or another program can capture into the
      notes directory. Org-Roam users lean on this heavily.

  Emacs receives `org-protocol://` URLs through `emacsclient`. Helix has no
  server to hand one to, so the fork takes the URL as an argument,
  `hx 'org-protocol://…'`, and `contrib/Helix-org-protocol.desktop`
  registers that as the handler for the scheme. The file passes
  `desktop-file-validate`, and its comments give the install steps and a
  capture bookmarklet. The query form and the older path form are both
  read. What each request does:
  - `capture` appends the page to `inbox.org` in the notes directory: a
    link headline, a `:CAPTURED:` stamp, and the selected text as a quote.
    With `template=` naming a configured node template, it makes a node
    from that template instead.
  - `roam-ref` opens the note whose `:ROAM_REFS:` already has the page, or
    makes one, as Org-Roam's default ref template does.
  - `roam-node` opens a node.
  - `store-link` keeps the link for `:org-insert-link`.

  The index is still being built when the URL arrives, so notes are found
  by reading the directory. Checked in the editor with each request: the
  inbox entry, with a selected line starting `*` escaped in the quote; an
  existing ref found in `rust.org`; a new ref node written as
  `zig-language.org`; `roam-node` landing on the node's `:ID:` line; and
  `open-source` refused by name.

  Not built: `open-source` (it needs a mapping from web addresses to local
  files), capture templates in Org's own format (`%a`, `%i`, `%:link`)
  rather than the fork's node templates, and handing a URL to an editor
  that is already running: each URL starts a new one.

*What is left after Task 1.16 is Emacs, not Org: link types into Gnus, BBDB,
Rmail, VM, MH-E, w3m, Eshell and the macOS applications, mouse support, the
Info reader integration, and the contrib modules built on them. As with
Task 2.19, these are deliberately not listed as gaps.*

### Task 1.17: Node Lifecycle and Restructuring

Org-Roam's answer to notes growing: a heading that has outgrown its file
becomes its own node, and a file that should be one node becomes one. None of
this exists in the fork, and it is what people reach for once a notes
directory is more than a few weeks old.

- [x] Extract the subtree at point into a node of its own, in a new file. This
      is the command that keeps a notes directory from turning into a handful
      of enormous files.

  This item first said the extraction leaves a link behind. Reading upstream's
  implementation showed it does not, and does not need to: the subtree carries
  its `:ID:` into the new file, so links that already pointed at it keep
  resolving.
- [x] Promote the whole buffer to a single file-level node, and demote a
      file-level node so its content becomes a subtree.
- [x] Refile a node into another node, which is not Org's own refile: the
      target is chosen from the graph, and the links pointing at the moved node
      must keep resolving.
- [x] Replace legacy `roam:` title links with `id:` links across a buffer, the
      migration path for notes written before v2.

### Task 1.18: What the Index Stores

The fork's node carries an id, a title, a path, tags, aliases and a line.
Org-Roam's schema stores considerably more per node — level, position, TODO
state, priority, scheduling, properties and the outline path — and keeps
citations in a table of their own.

This is not bookkeeping: it is what makes a query like "every unfinished node
tagged `project`, due this week" possible. Without those fields, the graph can
answer what links to what and nothing else.

- [x] Extend `Node` with the outline level, the TODO state, the priority, the
      `SCHEDULED:`/`DEADLINE:` timestamps, the outline path and the arbitrary
      properties of its drawer.
- [x] Index citations (`[cite:@key]`) as their own relation, so a bibliography
      key can be asked what cites it.
- [x] A query layer over the graph that these fields make worth having: filter
      by tag, state, priority and date, not only by title.
- [x] Decide how much of this the parser should do eagerly. Every field here is
      another thing to re-parse on every save, and the indexer is currently
      fast because it reads very little.

### Task 1.19: The Rest of the Org-Roam Buffer

The panel showed backlinks in one list. It now has sections.

- [x] A reflinks section: nodes that reach this one through a `:ROAM_REFS:`
      key rather than through an `id:` link. The graph already resolves refs,
      so this is the display half of something that exists.

  Less than it looked: the panel already drew `⇢` beside a ref link and `→`
  beside an `id:` one, so the two were distinguished and merely mixed. What
  was missing was the heading that separates them.

- [x] The unlinked-references section, already filed in Task 1.8, belongs to
      this same buffer and should share its rendering.

  Filled by the command rather than by the panel: finding unlinked references
  reads every file in the notes directory, which is not a thing to do sixty
  times a second. The section says which command fills it while it is empty,
  and the cache is tied to the node it was computed for — showing another
  node's references under this one's heading would be a lie.

- [x] A dedicated buffer pinned to a chosen node, alongside the one that
      follows the cursor. Comparing two nodes needs both.

  One panel showing both rather than two panels. Two components cannot see
  each other, so a second panel would have no way to know whether the first
  is open and where to place itself; the pinned node's sections simply go
  above the current one's.

- [x] An inline overlay showing a node's backlink count next to its headline,
      so the graph is visible while writing rather than only in a panel.

  A snapshot, taken when the counts are switched on. They do not follow the
  index: re-indexing is asynchronous, so a count that updated itself would
  need an event the editor does not raise, and reading the graph every frame
  to find out costs a lock and a scan of the buffer sixty times a second.

  Since Task 1.9's column view, the counts at least stay where they belong:
  an edit moves them with the headline they sit on, as it moves folds.
  Before, typing above a headline left its count where the headline used to
  be.

- [x] Diagnose the node at point: what the index believes about it, which is
      the only way to tell a parser bug from a malformed drawer.

  It reports the title twice, from the buffer and from the index, and says
  whether the buffer has changed since the index last read it — which is the
  answer most of the time, and the one that stops the hunt for a parser bug
  that is not there.

### Task 1.20: Graph Visualisation and Export

Two extensions upstream ships that the fork has no equivalent of.

- [x] Render the graph — the whole one, or a neighbourhood around a node at a
      chosen depth — and open it. Upstream shells out to Graphviz; doing the
      same avoids a layout engine in-process, at the cost of a dependency the
      fork can detect and report rather than require.

  `:roam-graph` draws the whole graph. `:roam-graph N` (or the
  `roam_graph_neighbourhood` command with a count, default 2) draws the
  nodes within N links of the one at the cursor. Links are followed both
  ways, since a note that links here is as much a neighbour as one this
  links to, and the centre is highlighted. An edge that exists only
  through `:ROAM_REFS:` is dashed. Repeated links between the same two
  notes are one edge, long titles wrap, and the DOT output is sorted, so
  the same graph always gives the same file.

  The DOT file is always written, to the system's temporary directory. If
  `dot` is on `PATH`, it is rendered to SVG in the background and handed to
  the system opener. What is missing is reported rather than failing:
  without Graphviz, the message gives the DOT file's path; without an
  opener, the SVG's.

  Checked in the editor, three times:
  - *without `dot`*, the DOT file was written and the reason given;
  - *with Graphviz 2.42*, installed for the test, the whole test graph
    rendered with 5 nodes and the depth-1 neighbourhood of "On systems"
    with 3; the rendered picture shows the dashed ref edge and the isolated
    node;
  - *without `xdg-open`* (this environment has none), the message gave the
    SVG's path.

  Not built: a clickable node that opens the note, which upstream does
  through `org-protocol` (Task 1.16); filtering links by type; and
  Graphviz layout options in the configuration.
- [x] Export: resolve `id:` links to something meaningful in the exported
      output rather than leaving a raw UUID. This is a prerequisite for
      Task 1.9's export being useful on a notes directory at all.

  Built with Task 1.9's export. An `id:` link becomes a link to the exported
  file of the node's Org file, relative to the one being exported, and to
  the node's anchor when the node is a headline. The anchor is its
  `:CUSTOM_ID:` or its title as a GitHub-style slug, computed the same way
  on both sides so that it exists in the target. A link without a
  description shows the node's title rather than its UUID.

### Task 1.21: In-Buffer Settings

Org defines 33 `#+KEYWORD:` settings and 70 `#+STARTUP:` options driving 29
variables. The fork's parser reads two of them, `#+title:` and `#+filetags:`,
and silently ignores the rest.

This matters more than a missing feature would, because several of these change
how a file must be *read*. Ignoring them does not remove a capability; it
produces a wrong index that nothing reports.

- [x] `#+TODO:`, `#+SEQ_TODO:`, `#+TYP_TODO:`. The parser currently leaves TODO
      keywords in the headline title on purpose — the code says so — because
      the keyword set is per-file and guessing would be worse. The consequence
      is that a node reads `TODO Write the thing` in the picker, and matches
      that way when Task 1.8 looks for unlinked references. Reading the keyword
      declaration is what makes stripping them safe.
- [x] `#+SETUPFILE:`, which pulls settings in from another file. This one is
      structural: it means a file's meaning depends on a second file, and the
      indexer currently reparses exactly one file per save. A setupfile change
      has to invalidate every file that includes it.

  `FileSettings::scan_at` reads a file's settings with its setup files'
  keywords counted as if written where the `#+SETUPFILE:` line is.
  - *Resolution:* relative to the file naming it, `~` for home, nested
    setup files included, a cycle cut short. A URL is not fetched, and a
    missing file is skipped.
  - *Who uses it:* the indexer, so the index is right; and the editor's
    commands, so cycling states, priorities, efforts, link abbreviations,
    the bibliography and the `#+STARTUP:` on opening all follow it.
  - *Invalidation:* the graph records which files read which setup file.
    Saving any file — a setup file is often `.setup`, not `.org` —
    re-indexes from disk the files that read it.

  Checked: a test changes a setup file's `#+TODO:` and watches the node's
  title lose the keyword after the re-index. In the editor, a file opened
  folded from its setup file's `#+STARTUP: overview` and cycled `NEXT` to
  `WAIT` from its `#+TODO:`.

  Not followed: the library's helpers that read settings from the text
  alone, without a path to resolve against — column view, export, clock
  reports, the sparse tree's keyword matching — and `#+FILETAGS:` from a
  setup file, which the parser reads in its main pass rather than from the
  settings.
- [x] `#+PROPERTY:` for file-level property defaults, which Task 1.11's
      inheritance builds on.
- [x] `#+TAGS:` and `#+PRIORITIES:`, which define the tag alist and the
      priority range a file uses. Priority cookies are currently stripped for
      any letter, without knowing the declared range.
- [x] `#+CATEGORY:` and `#+ARCHIVE:`, needed by Tasks 1.7 and 1.5 respectively.
- [x] `#+DRAWERS:` for custom drawer names, so a drawer the file declares is
      not parsed as content.
- [x] `#+STARTUP:` folding and visibility options (`overview`, `content`,
      `showeverything`, `hidedrawers`, `hideblocks`, …), which is how a file
      says how it wants to open. Task 1.4 built what these drive; the parser
      already collects them, and nothing applies them yet.

  Applied when a file is first opened, and again on `:org-startup-visibility`
  (Org's `C-u C-u TAB`). Read: `overview`/`fold`, `content`, `showall`/
  `nofold`, `show2levels` to `show5levels`, `showeverything`, the drawer and
  block pairs, the per-entry `:VISIBILITY:` property, and archived subtrees
  staying closed. The ranges are computed from the text, the way a sparse
  tree's are, and come out identical to what folding by hand produces — so
  `z` tab on a headline the file opened folded carries on cycling from there,
  which the editor confirmed.

- [x] `#+STARTUP:` logging options (`logdone`, `logdrawer`, `logrepeat`, …),
      which Task 1.11 needs to know what to record.

  Cycling a state recorded nothing before this: no `CLOSED:`, no log line —
  and a task with a repeater marked done simply **stayed done**, which is the
  one thing a repeater exists to prevent. Now, as in Org's `org-todo`:
  leaving a done state drops `CLOSED:`; `logdone` adds it and `lognotedone`
  asks for a closing note; a keyword's own `!` or `@` in `#+TODO:`
  (`WAIT(w@/!)`) logs entering or leaving it; and a repeating entry marked
  done moves every active repeating timestamp on (`+`, `++` and `.+` each as
  Org defines them), goes back to its first TODO keyword or its
  `REPEAT_TO_STATE`, and records `LAST_REPEAT` and a state line instead of
  closing. `logreschedule` and `logredeadline` log changing a date that was
  already set. A note is asked for in a prompt after the change is made;
  escaping it records nothing and keeps the change, as aborting does in Org.

  One default differs on purpose: log lines go into `:LOGBOOK:` unless the
  file says `nologdrawer`, where Org writes into the body. The fork's logging
  commands have always used the drawer, and moving where notes land under a
  user's feet would be worse than the difference.

  Not read: `logstatesreversed` (entries are always newest first),
  `lognoteclock-out` (no clocking until Task 1.9), `logrefile`, the
  per-entry `LOGGING` property, and a `#+TODO:` with several sequences
  resetting a repeat to the head of its *own* sequence — the settings keep
  one flat keyword list, so it goes to the first keyword of the file.

  The log headings (`State %-12s from %-12S %t`, `CLOSING NOTE %t`,
  `Rescheduled from %S on %t`, old dates quoted as inactive) and the order of
  the startup steps are written from memory of Org's `org-log-note-headings`
  and `org-cycle-set-startup-visibility`: the sources are unreachable from
  this environment, as they were for Task 1.13.

  Building this found three bugs in code already marked done, all from
  assuming the property drawer comes straight after the headline, when Org
  puts the planning line there. On any scheduled entry, the property drawer
  was invisible to every property command (`:EFFORT:` reported "no drawer");
  a new `:LOGBOOK:`, `:ID:` drawer or custom drawer was inserted *above*
  `SCHEDULED:`, which stops being a planning line once it is not directly
  under the headline; and setting `DEADLINE:` silently dropped a `CLOSED:`
  stamp on the same line. Each has a regression test.
- [x] The export and citation keywords — `#+OPTIONS:`, `#+INCLUDE:`,
      `#+MACRO:`, `#+BIBLIOGRAPHY:`, `#+CITE_EXPORT:` — belong with Tasks 1.9
      and 1.14 rather than here.

  All read by the export (Task 1.9), which already read `#+OPTIONS:`:
  - `#+MACRO:` with `$1…` arguments and escaped commas, plus Org's
    built-ins `title`, `author`, `date`, `input-file` and `keyword(…)`.
    Macros are expanded in text but not in code, and an unknown one stays as
    written and is reported.
  - `#+INCLUDE:` of Org text (itself expanded, with `:minlevel`), or wrapped
    as `src`, `example` or `export`, and `:lines "a-b"`. A missing file is
    reported.
  - Citations. `[cite…]` with styles (`/t`, `/a`, `/na`, `/nocite`) and
    prefixes and suffixes is read against `#+BIBLIOGRAPHY:` files, with a
    small BibTeX reader. How it is written depends on `#+CITE_EXPORT:`:
    - `basic`, the default and the only one outside LaTeX, writes
      "(Doe and Smith, 2020, p. 5)", linked to the entry in HTML, and
      `#+PRINT_BIBLIOGRAPHY:` lists the cited entries.
    - `biblatex` writes `\autocite`, `\textcite` and the rest, with
      `\addbibresource` and `\printbibliography`.
    - `natbib` writes `\citep`/`\citet` and `\bibliography`.
    A key not in the bibliography is reported.

  All three are tested against real files. Not read: CSL styles (Org's
  `csl` processor), `@string` macros in the bibliography, and `#+INCLUDE:`
  of a named block or a headline.
- [x] Keyword matching must be case-insensitive, as Org's is. The parser
      already lowercases keys, so this is a property to keep rather than add.

*On the global variables: Org ships 1037 `defcustom` declarations, concentrated
in `org.el` (165), `org-agenda.el` (119) and the export backends (`ox-html`
69, `ox-latex` 59, `ox` 54). Porting that surface is not a goal and is not
filed as a gap. Most of it is Emacs presentation or export tuning that the fork
will re-decide in Helix's own configuration idiom. What is filed above is the
subset that changes how a file parses, because those are the ones where being
wrong is silent.*

### Task 1.22: Index Maintenance and Inspection

Upstream ships a set of commands for when the index and the files disagree.

- [x] Rebuild the whole index from scratch, distinct from reindexing one file,
      for when an incremental update has gone wrong.

  It already was one: `scan_directory_async` assigns a new graph rather than
  updating the old, so `:roam-reindex` never trusted what was there. What was
  missing was the user learning the outcome — it logged and said nothing —
  and the rebuild now reports its counts, and refreshes the backlink counts
  drawn beside headlines, which were read from the index it just replaced.

- [x] Resolve `id:` links whose target lives outside the notes directory.
      Upstream keeps a separate cache of id locations for exactly this; the
      fork's graph only knows the files it scanned, so such a link silently
      fails to resolve.

  Every `.org` file the editor opens now leaves its ids in the graph, so a
  link into a file you have visited resolves even though the indexer has
  never seen it. A node answers for its own location; the cache is only for
  ids the index does not hold. Re-reading a file forgets the locations that
  pointed at it, and a rebuild carries the rest across — those files are
  never scanned, so a location dropped there has no way back. A link that
  still fails now says which of the two places was looked in.

- [x] Browse the index: which nodes, links and refs it holds, as a way of
      telling a parser bug from a malformed file.

  Nodes with their in and out counts, refs and citations in one list rather
  than three views: the case worth seeing is usually the one where a kind is
  missing entirely, and a view per kind hides exactly that.

- [x] Report the fork's own state — versions, notes directory, file and node
      counts — so a bug report can carry it.

  Two of the numbers are there to be compared. *Unresolved links* counts
  links whose target the index has not seen; *ids outside the index* counts
  the ones this session can nonetheless reach. When the first stays high
  after a rebuild, the links point outside the notes directory, and the
  second says how many of those are already answerable.

*A note on validating any of this in the editor, paid for once. The pty
harness must **read the editor's output continuously while it types**. Sleeping
between keystrokes without reading fills the pty's output buffer; Helix then
blocks on write and stops reading input, so every keystroke queues up and
arrives in one burst at the end. That made a correct editor look broken — a
command opening a prompt asynchronously appeared to lose focus, because the
keys meant for the prompt had already been consumed while it did not yet
exist. Tracing the compositor stack showed the prompt being pushed 10 ms after
the key that asked for it, and the keys arriving 2 ms apart although the
harness had spaced them 900 ms. Reading while typing made the same sequence
pass.*

*A second one, cheaper: an Escape followed by another key must go out as two
writes. Written together, `\x1b:` is Alt-`:` to a terminal, so "escape the
prompt, then `:w`" silently became neither — the file was never saved, and
the run looked like a command that had changed nothing.*

---

## Phase 2: Magit Client (`crates/helix-magit`)

### Task 2.1: Git Engine & Diff Data Model
- [x] Create `crates/helix-magit` in the workspace.
- [x] Integrate `git2-rs` or `gix` (Gitoxide) for repository operations.
- [x] Implement diff parser to extract structural AST: `FileDiff`, `DiffHunk`, `DiffLine`.

### Task 2.2: Transient Menu System
- [x] Create `TransientMenu`, `TransientGroup`, `TransientArgument`, and `TransientAction` models.
- [x] Implement `TransientOverlay` component in `helix-term` to render transient menus.
- [x] Support modal keybindings for toggling switches (`--autostash`, `--interactive`) and executing actions.
- [x] Implement default menu constructors for `Commit`, `Rebase`, `Push`, and `Pull`.

### Task 2.3: `DiffView` Component & Line-Level Staging
- [x] Create `DiffView` component in `helix-term` supporting hierarchical navigation (File -> Hunk -> Line).
- [x] Implement `Tab` key behavior to collapse/expand files and hunks.
- [x] Implement line-level patch generation algorithm for staging (`s`) and unstaging (`u`).
- [x] Render diff text using Helix's native Tree-sitter highlighter and active theme (`Theme`/`Style`).

### Task 2.4: Executing the Transient Commands

The transient menus resolve a git command line from their switches and options,
but running it is deliberately not implemented: actions report the command they
would run instead. Executing them needs a message editor, credentials, progress
and async process handling, none of which the menu system itself covers.

- [x] Run a resolved command line asynchronously, on a job, so the editor never
      blocks on git; stream stdout/stderr and surface failures in the status
      buffer rather than discarding them.
- [x] `Commit`: open a scratch buffer seeded with the commit template and the
      status comment block, and commit when it is written and closed — with a
      way to abort that leaves the index untouched.
- [x] `Commit --amend` / `Extend` / `Fixup`: seed the buffer from HEAD's
      message, and refuse to amend a pushed commit without confirmation.
- [x] `Push` / `Pull` / `Fetch`: decide between `gix`'s own transport and
      invoking the `git` binary for network operations. `gix` keeps the fork
      free of a `git` dependency, but the user's credential helpers, SSH agent
      configuration and `~/.gitconfig` `url.*.insteadOf` rules come for free
      only with the binary. This decision gates the whole group.
- [x] `Rebase`: `--interactive` needs `GIT_SEQUENCE_EDITOR` pointed back at
      Helix, which means the integrated terminal or a spawned instance; decide
      which before starting. *(Neither: see Task 2.7.)*
- [x] Refresh the `DiffView` after any command that changes the index, HEAD or
      the working tree.
- [x] Guard destructive actions (`--force`, `branch -d`, `rebase --abort`)
      behind a confirmation that names what will be lost.

*Tasks 2.5 to 2.14 were derived from the actual definition of Magit's
`magit-dispatch` transient, entry by entry, rather than from recollection of
it. Tasks 2.15 to 2.19 cover the long tail below that menu, derived the same
way: from Magit's own module list and from the interactive commands its
grab-bag module defines. What remains after them is Emacs integration rather
than Git, and is named at the end of Task 2.19. The whole phase was then
re-checked against a local clone, enumerating all 51 of Magit's transients,
which added Task 2.20 and four items to Task 2.9 and confirmed the rest.*

### Task 2.5: The Rest of the Status Buffer

The status buffer shows two sections, Unstaged and Staged. Magit shows the
repository's whole situation, and the missing sections are the ones that tell
you whether you need to push, pull or recover something. Untracked files are
already handled — they appear as whole-file additions under Unstaged — but
they have no section of their own.

- [x] A head section: the current branch, its upstream, and the message of
      `HEAD`.
- [x] Unpushed and unpulled sections: commits the upstream does not have, and
      commits it has that the working copy does not.
- [x] A stashes section, listing entries and letting one be shown.
- [x] A recent-commits section.
- [x] An untracked section of its own, separate from unstaged changes.
- [x] In-progress state: a merge, a rebase or a cherry-pick under way is what a
      user most needs the status buffer to tell them, and it currently says
      nothing.

*Done.* `helix_magit::status::read` asks the `git` binary — the one the
transient commands run — for the branch, `@{upstream}`, `@{push}` (shown only
when it differs from the upstream), `HEAD..@{upstream}`, `@{upstream}..HEAD`,
the stash list and the last ten commits, which Magit shows only when nothing
is unpushed and so does this. The operation under way is read from the state
files git leaves in its directory (`rebase-merge/`, `rebase-apply/`,
`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`), with the step
and the commit a rebase stopped at, and a line naming the commands that get out
of it. Rebases are not called "interactive": git's merge backend writes that
marker for a plain `git rebase` too. Untracked files have their own section and
start folded, listed by name; `s` still adds them. `RET` on a commit or a stash
opens `git show` / `git stash show -p` in a scratch buffer highlighted as a
diff, closing the status overlay, since it covers the whole editor. Checking
this in the editor showed that a conflicted file appeared under *Staged
changes* once per index stage; those entries are now skipped there and listed
in an *Unmerged paths* section instead (Task 2.10's first item). Folding of
sections now survives a refresh as file folding already did. The overview
costs about eight short `git` processes per refresh, run on the editor thread
like the rest of the refresh.

### Task 2.6: Discarding and Bulk Staging

Three holes in the daily loop the fork otherwise covers end to end.

- [x] Discard (`x`): throw away a hunk, a line selection or a whole file's
      changes. The patch machinery for this already exists — it is the same
      reverse-apply staging uses — but discarding cannot be undone, so it needs
      the confirmation Task 2.4 introduced.
- [x] Stage and unstage everything (`S`, `U`).
- [x] Visit the file at point (`RET`), landing on the line under the cursor
      rather than at the top of the file.
- [x] Apply (`a`) and reverse (`v`) the hunk or selection at point, the two
      operations that share discard's reverse-apply machinery.

*Done.* `x` asks first, naming what goes ("Discard this hunk of f.txt?"),
behind the same one-key confirmation as Task 2.4. An unstaged change is
reverse-applied to the working tree; a staged one leaves both the index and the
working tree, as in Magit, and the working tree is checked before either is
touched, so a file that has moved on since it was staged is refused rather than
half-discarded. `x` on a whole untracked file deletes it, and on a stash drops
it. `S` is `git add -u` (tracked files only, as Magit's `S`) and `U` resets
the index, with `git rm --cached` before the first commit. `RET` opens the file
at the line under the cursor — for a deleted line, the line now in its place;
for a hunk, its first change. `v` reverses a staged change out of the working
tree and leaves the index. On a change in the status buffer `a` has nothing to
do (the change is already in the working tree), so, as in Magit, `a` and `v`
act on the commit or stash at point: `cherry-pick --no-commit`,
`stash apply`, `revert --no-commit`.

Writing this turned up four bugs in the staging Task 2.3 shipped, each now
covered by a test against a real repository: reverse patches (unstaging, and
now discarding) were built for the wrong side, so unstaging *some lines* of a
hunk failed whenever the hunk had unselected changes; staging a whole deletion
left an empty file in the index instead of removing the path, and unstaging a
whole new file left it tracked and empty; and a staged deletion was invisible,
because the staged section walked only the index and never met a path HEAD has
and the index does not.

### Task 2.7: Interactive Rebase

Deferred from Task 2.4 with the blocker already identified: every git
subprocess runs with `GIT_SEQUENCE_EDITOR=true`, so that nothing can hang the
editor waiting on a terminal that is not there. That setting accepts the
todo-list unchanged, which means `rebase --interactive` currently runs but
cannot be steered.

- [x] Decide how git reaches back into Helix for the todo-list: the integrated
      terminal, a spawned instance, or an edit-server the fork runs. This gates
      the rest, and the same mechanism would serve `commit --verbose` and any
      other command wanting an editor.
- [x] A todo-list buffer: reorder commits, and set pick, reword, edit, squash,
      fixup and drop.
- [x] Drive a rebase that stops: show why it stopped, and offer continue, skip
      and abort from the status buffer.
- [x] `--autosquash`, so the `Fixup` action Task 2.4 already produces has
      something that consumes it.

*Done. Decision: git does not reach back into Helix at all.* The rebase runs
twice (`helix_magit::rebase`). The first run's sequence editor is a shell
command that copies git's todo-list out and empties it; an empty list is git's
documented way to cancel, so git stops with "nothing to do" and the repository
is exactly as before, autostash included. Because the list is git's own,
`--autosquash`, `--rebase-merges` and the rest shape it as they would anywhere.
The list opens in a split (`.git/helix/git-rebase-todo`, so it gets the
`git-rebase` grammar); `:w` runs git again with a sequence editor that copies
the edited list in, and `:q!` or an empty list cancels. HEAD is recorded at the
first run and the second is refused if it moved. This needs no server, no
second instance and no terminal, and it works for any command whose input can
be prepared before git runs; it would not serve `commit --verbose`, whose diff
git writes into the message file itself.

`:rebase-todo <action>` sets the selected lines to pick, reword, edit, squash,
fixup or drop, and `:rebase-todo up` / `down` moves them; the list is also
ordinary text. `reword` needs a message editor mid-rebase, and every git
subprocess runs with `GIT_EDITOR=true`, so it runs as `edit`: the rebase stops,
the message is amended with the commit menu (`c a`), and the rebase continued
(`r c`). The status buffer now opens the menus by Magit's dispatch keys (`c`,
`r`, `P`, `F`, `b`, `?`); `r` on a commit makes an interactive rebase start
from it (its parent, or `--root`), otherwise it asks. A stopped rebase shows
where it stopped, the unmerged paths, and `r then c to continue, s to skip, z to
abort`; `Skip` is new in the rebase menu, behind a confirmation, and
`--autosquash` (`-A`) is a switch there. The plain rebase with the
`--interactive` switch goes through the same list instead of running with an
editor that accepts it unchanged. Checked in the editor: autosquash ordering,
moving and dropping lines, a reword stopped, amended and continued, and a
conflict stopped and skipped.

Doing this found that the transient menus looked arguments and actions up by
the same key, arguments first, so the commit menu's Amend (`a`) and Extend (`e`)
could never run — `--all` and `--allow-empty` took the keys. Arguments are now
reached through `-` as in Magit (`-a` toggles `--all`, `a` amends), and the
menus show them that way.

### Task 2.8: The Log

Magit's second buffer, and the one the fork has no equivalent of at all.

- [x] A log buffer: a commit list with the graph, refs and dates.
- [x] Show a commit: its message and its diff, reusing the `DiffView` rendering
      rather than a second implementation.
- [x] Filter by file, author, range and free text.
- [x] Act on the commit at point: cherry-pick, revert, reset to it, start an
      interactive rebase from it, all of which depend on Tasks 2.7 and 2.9.

*Done.* The log (`l` in the main menu, `L` in the status buffer — `l` folds
there, as elsewhere in Helix) is `git log --graph` read line by line
(`helix_magit::log`): the graph as git draws it, then hash, refs, subject, and
author and date on the right when the window is wide enough. The cursor steps
over the lines that only draw the graph. The log menu offers the current
branch, all references and another revision or range, with `-A` author, `-G`
message and `-F` file to limit it; inside the log, `/`, `@`, `f` and `o` change
those, `=` clears them, and `+` doubles the 256 commits read at first.

`RET` on a commit shows it in the status buffer's own view: the message and
metadata in the header, the files, hunks and lines below, highlighted and
folded the same way. It reads `git show --diff-merges=first-parent`, so a merge
shows what it brought in, and a stash, being a commit, opens the same way from
the status buffer (instead of the scratch buffer Task 2.5 used). In a commit,
`a` applies the change under the cursor — file, hunk or line — to the working
tree and `v` takes it back out, as in Magit; staging keys and `x` say why they
do not apply.

On a commit, in the log or the status buffer: `A` cherry-picks it, `V` reverts
it, `X` opens the new reset menu aimed at it, `r` the rebase menu with the
interactive rebase starting from it. After any of them the log is read again
and the cursor stays on the same commit. Checked in the editor: the graph of a
merge, every filter, a commit and a stash shown, a hunk reversed and reapplied,
a revert, a hard reset (confirmed), a cherry-pick from the all-references log,
and an interactive rebase started from the log.

Transient options (`--author=`, `--strategy=`, …) could not be given a value
until now: toggling one only ever cleared it. `-` and an option that is off
now asks for its value, and the menu shows it.

### Task 2.9: The Remaining Transients

What is left of Magit's dispatch menu. Each is a transient the existing menu
system can already express and a command line `resolve` can already build, so
these are mostly breadth rather than new mechanism — with the exceptions noted.

- [x] Stash: save, pop, apply, drop, and stash only the index or only the
      worktree.
- [x] Merge, with `--no-ff`, `--squash` and abort; conflict resolution is its
      own problem and is not covered by this item.
- [x] Reset: soft, mixed and hard, with hard behind a confirmation. *(With
      Task 2.8: `X`, plus keep; the target is the commit at point, or asked.)*
- [x] Tag: create, delete and push tags.
- [x] Cherry-pick and revert, including their continue and abort states.
- [x] Remote: add, rename, remove, and set a branch's upstream. This also
      supplies the remote picker Task 2.4 left missing, which is what
      `PushElsewhere` is waiting on.
- [x] Bisect: start, good, bad, reset, and show where it is.
- [x] Worktrees and submodules.
- [x] Apply and format patches (`am`, `format-patch`).
- [x] A process buffer: every command the fork has run and what it printed.
      Task 2.4 shows only the first useful line in the status line, so the rest
      of git's output is currently discarded.
- [x] Subtree (`O`), which is separate from the submodule and worktree work
      above.
- [x] Notes (`T`): add, edit and remove git notes.
- [x] Show refs (`y`): every branch and tag with its relationship to `HEAD`.
- [x] Cherries (`Y`): the commits one branch has that another does not.
- [x] Gitignore helpers (`i`): ignore the file at point, in the repository's
      `.gitignore` or in the private exclude file.
- [x] Branch management the Task 2.4 menu left out: rename, reset, `spinoff`
      and `spinout`.
- [x] The configuration transients: a branch's own git config (upstream, rebase
      behaviour, description) and a remote's, both of which upstream gives a
      menu of their own rather than a single "set upstream" action.
- [x] Shortlog: who contributed what over a range.
- [x] Fetch submodules as an operation distinct from fetching the repository.

*Done.* One mechanism carries most of it: an action now declares the values it
needs as a list of questions (`Requirement::Ask`), each with a kind — branch,
remote, revision, tag, stash, path, name, message — that decides how it is
completed (branches, remotes, tags and stashes from the repository, paths from
the file system) and whether what the menu was opened on answers it. Opened on
a commit, a stash, a branch in the refs view or a file, a menu fills the first
question of a fitting kind and asks only the rest; a path is offered as an
editable suggestion instead, since `junk.log` is as likely to become `*.log`.
Answers fill `{0}`, `{1}` placeholders in the command line, and an answer that
would land as an argument of its own is refused if it starts with a dash, where
git would read it as an option (a tag named `-v1` was the case that showed it).
The status buffer and the log open every menu by Magit's dispatch key, listed
in the main menu (`?`): `z m t A V M B % o w W O T i`, plus `y` (refs), `Y`
(cherries) and `$` (process output).

What needed more than a command line:

- *Stashing only the worktree*: git has no switch for it. The index is stashed
  aside, the rest stashed, and the index popped back, with the stash count
  checked at each step so a step that saved nothing never makes a later one pop
  the wrong stash; with nothing staged it is a plain stash. Only the index is
  `--staged`.
- *Spinoff* and *spinout* move the unpushed commits to a new branch (checked out
  or not) and put the current branch back at its upstream, refusing before
  touching anything when there is no upstream.
- *Ignoring* appends to `.gitignore` or `info/exclude`, once.
- *The process buffer* (`$`) is a scratch buffer of every command the menus ran
  — commits and rebases included — with all it printed, newest last, capped at
  200. The read-only commands the views run to draw themselves are left out;
  they would bury the rest.
- *Refs* (`y`) and *cherries* (`Y`) are the status buffer's view again:
  branches, remote branches and tags with their upstream and how far ahead or
  behind HEAD each is (counted for the first 100 refs), and `git cherry`'s list
  with `-` for a change already upstream in another form. `RET` shows the
  commit; the menus act on the ref under the cursor.
- The status buffer lists other worktrees and submodules (state included:
  not initialized, out of date, conflict); `RET` opens that one's status.
- Merge, cherry-pick, revert, `am` and bisect in progress each say in the
  status buffer which menu keys get out of them; bisect also shows the commit
  being tested.
- A failed command's status line now shows its reason (`CONFLICT…`, `error:`,
  `fatal:`) rather than its first line, which for a merge was
  "Auto-merging f".

The configuration menus (`b C`, `M C`) set a branch's description, upstream,
`rebase` and `pushRemote`, and a remote's URL, push URL and fetch refspec, but
unlike Magit's they do not show the current values. Shortlog is `s` in the log
menu, in a scratch buffer, limited by the log menu's author, message and file.

Checked in the editor: stash of the worktree only, ignore from an untracked
file, spinoff, push elsewhere with `--set-upstream`, the refs view and a rename
from it, cherries, the process buffer, a conflicting merge shown and aborted,
a whole bisect, a worktree added, opened and removed, a tag on the commit at
point (and the dash refusal), a note, a stash popped from the stash under the
cursor, a remote added and reconfigured, format-patch then `am`, a subtree
added, the submodule section, a cherry-pick conflict shown and aborted, branch
`rebase` config and shortlog. Not checked in the editor: `submodule add` from a
local path, which git itself refuses unless `protocol.file.allow` is set
globally (the refusal is what the status line shows); worktree-only stash with
a file that has staged and unstaged changes to the same lines.

### Task 2.10: Conflict Resolution

Task 2.9 lists merge but explicitly excludes resolving the conflicts it
produces, because that is a buffer and a workflow rather than a command line.
Magit reaches for Ediff here (`e`, `E`); the fork has no equivalent and needs
its own answer.

- [x] A conflicts section in the status buffer, listing unmerged paths and
      their stage. *(With Task 2.5: "both modified", "deleted by them", … as
      `git status` words it, from which of base, ours and theirs the index
      holds.)*
- [x] Open a conflicted file with the three sides available — ours, theirs and
      the base — and a way to take either side for a region.
- [x] Mark a path resolved, and drive the surrounding operation to its end
      (`merge --continue`, `rebase --continue`, `cherry-pick --continue`).
- [x] This applies to every operation that can stop on a conflict, not just
      merge, so it belongs with none of them in particular.

*Done — the fork's answer to Ediff is the conflicted file itself, with the
sides one key away rather than three windows at once.* `e` on an unmerged path
in the status buffer opens a resolve menu for it:

- `e` opens the file on its first conflict. `]m` / `[m` move between
  conflicts (both free under Helix's bracket keys), and
  `:conflict-take ours|theirs|base|both` replaces the conflict under the
  cursor with that side, or with ours then theirs; the status line counts what
  is left. The regions are read from the markers (`helix_magit::conflict`),
  and a half-edited region — an opening marker with no closing one — is not
  taken for one.
- `O`, `B`, `T` open the file and, beside it, the whole of our, the base's or
  their version, from index stages 2, 1 and 3, highlighted as the file is.
- `3` rewrites the file with the base section in every conflict
  (`checkout --conflict=diff3`), so `base` can be taken per region; it discards
  edits made to the file, so it asks first.
- `o` / `t` resolve the whole file with one side, including a side that
  deleted it, which resolves as the deletion; they ask first too.
- `s` — here and on an unmerged path in the status buffer — marks it resolved,
  and refuses while a conflict marker is left, naming the line.
- `c` here, or `C` anywhere in the status buffer, continues whichever operation
  stopped: merge, rebase, cherry-pick, revert or `am`, read from git's state
  files rather than tied to any one menu. The status buffer's hint for each
  says so.

Checked in the editor on a merge stopped on a both-modified file and a file
their side deleted (one region taken as theirs, one as both, written, marked
resolved; the deletion taken as theirs; `C` committed the merge) and on a
rebase stopped on a conflict (rewritten with the base, the base taken,
resolved, `C` finished the rebase), plus the base shown beside the file.

### Task 2.11: File-Scoped Commands and Blame

Magit has a second dispatch for the file you are editing, reachable without
opening the status buffer at all. The fork has nothing equivalent: every Git
operation currently starts from `<space>m`.

- [x] A file dispatch: stage, unstage, diff, log and blame for the current
      buffer's file.
- [x] Blame: annotate each line with its commit, author and date, and move
      between revisions of the same line.
- [x] Log and diff restricted to the current file, which Task 2.8 should build
      on rather than duplicate.

*Done.* `<space>M` (free in Helix's space mode, beside `<space>m`) or
`:magit-file` opens the file dispatch for the current buffer's file: `s` / `u`
stage and unstage the whole file, `c` opens the commit menu, `d` its diff, `l`
its log, `b` its blame, `g` the status and `?` the main menu.

- The diff is the status buffer restricted to the file — its unstaged and
  staged changes, staged, unstaged and discarded by hunk or line the same way.
- The log is Task 2.8's log with a path, now followed through renames
  (`--follow`, which git allows for a single file).
- The blame (`helix_magit::blame`, from `git blame --line-porcelain`) opens on
  the cursor's line: each run of lines from one commit is headed by its hash,
  author date (in the author's zone), author and subject, beside the code
  highlighted as the file is; lines not committed yet say so. `n` / `p` move
  between runs, `RET` shows the commit in the commit view, `l` shows the log,
  `v` visits the file at that line. `b` blames the version before the commit
  under the cursor — the commit and file name git records as the line's
  `previous`, so a rename on the way is crossed — landing on the line's number
  in that commit's version; repeated, it walks a line's history back one change
  at a time, and `q` walks forward again, closing the blame at the start. On a
  line with no earlier version it says so.

The blame is a view of its own over the editor rather than annotations inside
the buffer: Helix has no line-annotation layer to put them in, and building one
is outside this task. Checked in the editor on a file with three authors and a
rename: stepping back from the working tree through two earlier versions of one
line and across the rename, forward again, the commit and file visit from the
blame, the file's diff, log (through the rename) and staging.

### Task 2.12: Diff Presentation Controls

The `DiffView` renders with fixed settings. Magit puts these on a transient
(`d`, `D`) because the right answer changes with the diff you are reading.

- [x] Context lines, adjustable while reading.
- [x] Whitespace handling (`-w`, `--ignore-space-change`), which is the
      difference between a readable diff and an unreadable one after a
      reindent.
- [x] Diff algorithm (`--histogram`, `--patience`).
- [x] Word-level diff, and `--stat` as a summary view.
- [x] Diff against an arbitrary revision or between two, rather than only
      worktree-against-index and index-against-`HEAD`.

*Done.* Every diff view — status, a file's diff, a commit, a range — carries
its own `DiffOptions` (context, whitespace, algorithm, word marking, summary),
shown in its title when they differ from the defaults. In a view, `+` / `-`
widen and narrow the context and `0` puts it back to 3, as in Magit's diff
buffers; `D` opens the diff settings menu showing the view's settings as they
are, and `g` there applies them to the open diffs.

- The status buffer's diffs are computed in-process, so the options reach
  that computation: context and algorithm directly (histogram, Myers, minimal;
  patience, which only git has, is shown as histogram, its refinement), and
  whitespace by comparing lines through the setting's key while printing them
  as they are — context and deleted lines from the old side, added lines from
  the new. A reindented line is then context, printed as the index has it, so
  staging from a diff that ignores whitespace stages exactly the changes shown
  and keeps the index's indentation (tested against a real repository).
  Unstaging or discarding from such a diff can meet a line whose whitespace
  differs from what the view shows; the patch is then refused rather than
  applied wrongly.
- Commits and ranges come from `git show` / `git diff` with the same options
  as arguments, `--diff-algorithm=patience` included.
- Word marking pairs each run of deleted lines with the run of added lines
  after it, line by line, and marks the words that differ (imara-diff over
  words, with changes that only whitespace separates joined). The marks
  toggle reverse video rather than setting it, because in a theme whose popups
  are reversed already — base16, which Helix falls back to without true colour
  — setting it showed nothing; that is how it was found.
- The summary shows each file's size of change as a `+++---` bar scaled to
  the largest, and no hunks.
- `d` opens the diff menu: between two revisions, the working tree against a
  revision, or one commit, with the revisions completed and the first filled
  from the commit under the cursor. The range view is the commit view's: `a` /
  `v` apply or reverse a change from it in the working tree.

Checked in the editor: a reindent plus one real change shown plain, with `-b`
(only the real change), with no context, as a summary, staged under `-b`
(only the real change reached the index); a range between two revisions; a
commit with `-w`; word marking, whose escape sequences were checked in the
terminal output since the harness's text rendering cannot show them.

### Task 2.13: Repository Entry Points and Arbitrary Commands

Small dispatch entries with nowhere else to sit; each is short on its own.

- [x] Clone (`C`) and init (`I`).
- [x] Jump to a section of the status buffer (`j`), and switch between the
      fork's Git buffers (`J`).
- [x] Run an arbitrary git command in the repository (`Q`) and a shell command
      (`!`), both reporting into Task 2.9's process buffer.

*Done.* `<space>m` outside any repository used to stop at "no git repository
found"; it now opens a menu with clone and init. Both are in the main menu too
(`C`, `I`), and run from the editor's directory rather than inside the
repository the menu belongs to; clone asks for the URL and an optional
directory, init for an optional directory, and once either succeeds its status
opens. `Q` asks for a git command line and `!` for a shell one (run by `sh -c`
in the repository, with pagers, editors and credential prompts off as for every
command here); the line is split as a shell would split it — quotes, escapes —
and both go through the process buffer (`$`), where a command shows quoted as it
ran. Two keys differ from Magit, because in the fork's views `j` moves down as
everywhere in Helix: jumping to a section of the status buffer is `'`, then the
section's key (`u` unstaged, `s` staged, `n` untracked, `z` stashes, `p`
unpushed, `f` unpulled, `r` recent, `m` unmerged, `w` worktrees, `o`
submodules); `J` lists the Git views open now — status, log, commit, diff,
refs, cherries, blame — and brings the one chosen to the front, opening the
status or the log when they are not open. The status line no longer reports
git's `hint:` lines as what happened (init's summary was a hint about branch
names).

Checked in the editor: init in a directory outside any repository (the setup
menu, then the new repository's status), a clone into a named directory
(its status opened), a quoted `git tag` typed with `Q` (git's own refusal
shown, the command quoted as typed), a shell command with a redirection and a
pipe via `!`, a jump to the untracked section, and switching between the status
and the log with `J`.

### Task 2.14: The Transient Arguments Left Out

Task 2.4 shipped 19 arguments across five menus. These are the ones Magit
offers that it did not.

- [x] Commit: `--gpg-sign=`, `--date=`, `--reset-author`.
- [x] Push: tags and explicit refspecs.
- [x] Rebase: `--onto` an arbitrary revision, rather than only the upstream.
- [x] The commit menu offers `--verbose`, which asks git to put the diff in the
      message buffer. Task 2.4 supplies the message with `-F`, so no editor
      runs and the switch currently does nothing. Either the fork seeds the
      diff into the buffer itself, or the switch should go; leaving a flag that
      silently does nothing is the worst of the three.

*Done.*

- The commit menu gains `-R` `--reset-author`, `-D` `--date=`, `-S`
  `--gpg-sign=` with a key and `-G` `--gpg-sign` with the default one. Signing
  needs a key the agent can use without a terminal; with a passphrase and no
  graphical pinentry, gpg fails and the status line says so.
- The push menu gains `-t` `--tags`, `r` for explicit refspecs — a remote,
  then one or more refspecs, split at spaces (a question can now take several
  words) — and `T` for all tags.
- The rebase menu gains `o`: onto a revision, of the commits after another
  (`git rebase --onto new old`), behind the rebase confirmation. It is never
  interactive; the todo-list flow asks only for a base.
- *`--verbose`: the fork seeds the diff itself.* With it on, the message
  buffer gets git's scissors line and the diff being committed below it (for
  an amend, everything since HEAD's parent, as git shows); everything from the
  scissors down is dropped from the message, including diff lines that do not
  start with `#`. The buffer is `COMMIT_EDITMSG`, so Helix's `git-commit`
  grammar highlights the diff.

Checked in the editor: a `--verbose` commit (the diff in the buffer, not in the
message), a commit signed with a passphrase-less key and dated with `-D` (git
reports the signature good), `--onto` moving a branch from one base to another,
and a push of two refspecs, one a renamed branch.

### Task 2.15: The Reflog

Nothing in the fork exposes the reflog, and it is the one thing that gets a
user's work back after a bad reset, a dropped branch or a rebase that went
wrong. Every destructive action Tasks 2.4 and 2.9 put behind a confirmation is
recoverable through it, which is what makes its absence worth a task of its
own.

- [x] A reflog buffer for `HEAD` and for a chosen ref.
- [x] Act on the entry at point: check it out, reset to it, or create a branch
      there.
- [x] Reach it from the status buffer, so it is findable at the moment it is
      needed rather than only by someone who knows it exists.

*Done.* The reflog is the log view walking the reflog (`git log
--walk-reflogs`, without the graph git refuses to draw there): each entry's
commit, its selector (`HEAD@{2}`) where the refs go, and why the ref moved
(`reset: moving to HEAD~1`, `rebase (finish): …`). It is `r` (HEAD) and `R`
(another ref, asked for) in the log menu, so `L r` from the status buffer. On
an entry, the log's keys act on its commit: `RET` shows it, `X` opens the reset
menu aimed at it, `b n` creates a branch there, and `b b` checks it out — the
checkout question now takes any revision and offers the one under the cursor
for editing rather than checking it out at once. To make it findable when it
is needed, every confirmation for a command that moves a ref (reset, rebase,
branch, cherry-pick, revert, merge, am) now says that the commits it leaves
behind stay in the reflog, and where.

Checked in the editor: a hard reset confirmed (the reflog pointer shown in the
confirmation), the reflog listing it and the rebase before it, a branch
created on the commit the reset left behind, and the checkout offered from an
entry. The reflog's own selectors are right only without `--date`, which makes
git name entries by date instead; that is why the date comes from `%as`.

### Task 2.16: Work-in-Progress Refs

Magit can commit the working tree and the index to hidden refs on every save
and every operation, so that uncommitted work is recoverable even though it was
never committed. It is off by default there and would be here too.

- [x] Write worktree and index wip refs on save and before destructive
      operations.
- [x] A log over the wip refs, and a way to restore from one.
- [x] Decide the default. This writes to the repository on every save, so it is
      opt-in or it is a surprise.

*Done. Decision: off by default*, turned on with `[editor.magit] wip = true`
(documented in the book's editor page). Asked for, it writes to the repository
on every save; unasked, that is the surprise the item warns about.

- `helix_magit::wip` keeps Magit's layout: `refs/wip/index/<branch>` for the
  index and `refs/wip/wtree/<branch>` for the working tree's tracked files
  (`HEAD` in place of the branch when detached). The working tree's commit is
  built in a copy of the index, so the real index, the working tree and every
  branch are left exactly as they were (tested). Each save goes on top of the
  previous one while the chain still builds on HEAD, and starts again from HEAD
  once HEAD has moved past it; a save that would record nothing new records
  nothing; a conflicted index, which has no tree, is skipped; before the first
  commit nothing is saved. Without a configured identity the private commits
  are made as "Helix wip".
- When: after a file in a repository is written (only that file, off the
  editor's thread; git's own files such as the commit message are not work);
  before any command that asks for confirmation because it can lose work; and
  before `x` discards. If that save fails, the command or the discard does not
  run, and says why.
- `w` and `W` in the log menu show the working tree's and the index's chains
  (or say that there are none, and whether wip saves are off). A save is a
  commit: `RET` shows what it changed since the save before, `a` / `v` apply or
  reverse a hunk of it, and the reset menu's new `w` puts the whole working tree
  as the save has it (`git restore --worktree --source=…`), leaving HEAD and
  the index alone.

Checked in the editor with wip on: a file written (the save appears on the
worktree chain), a hard reset that threw away uncommitted work (saved just
before it), the chain listed with both saves, and the work put back from the
save before the reset; and with wip off, a write that saves nothing and a
wip log that says they are off.

### Task 2.17: Repository List, Buffer Freshness and the Margin

Three separate modules that all concern how the fork's Git buffers behave
rather than what they run.

- [x] A repository list: the repositories the user works in, with their branch
      and how far ahead or behind each is.
- [x] Refresh open buffers when git changes files underneath them. A checkout
      or a reset currently leaves every open document stale, which is a
      correctness problem, not a convenience one.
- [x] A margin alongside log and status lines showing the author and the age of
      each commit, and a way to toggle it.

*Done.* The list is `R` in the Magit menu (and in the menu shown outside a
repository): `helix_magit::repos` finds the repositories under
`[editor.magit] repository-directories` (default: the working directory),
`repository-depth` levels down (default 2), stopping at a repository and
skipping hidden directories, and summarizes each with one
`git status --porcelain=v2 --branch`: branch, upstream, `↑`/`↓` counts, `*`
when dirty. `RET` opens a repository's status over the list, a menu key opens
that menu on the repository under the cursor, and the list refreshes after a
command like the other views. Every git command the fork runs, and a discard,
apply or reverse from the status, is followed by a pass over the open
documents: an unmodified one whose file changed is reloaded (as `:reload`
would, keeping its views in place); one with unsaved changes is left alone and
named in a "Not reloaded from disk" message, as is an unmodified one whose file
git removed. The comparison is of contents, not modification times, so a
command that rewrites a file identically reloads nothing. The margin is the
author and the age (`3 days`) or the date, flush right and aligned among the
visible lines, in the log and on the status buffer's commit lines; `Z` cycles
age → date → hidden, and the choice holds for the session, separately for the
log (shown by default, as in Magit) and the status (hidden by default). Stash
lines have no margin: `git stash list` is read without author or date.

### Task 2.18: Sparse Checkout and Bundles

Two self-contained areas with no equivalent in the fork. Neither is needed for
the daily loop; both are the kind of thing whose absence is only discovered at
the moment it is wanted.

- [x] Sparse checkout: enable it, list and edit the directories included.
- [x] Bundles: create a bundle from a range of commits, and unbundle one.

*Done.* Sparse checkout is `>` (Magit's key), in the main menu and from the
status and the log: enable (`git sparse-checkout set --cone`: the top-level
files only), set the directories, add some, reapply, disable. Cone mode only
is offered, as git recommends; a repository already in the older pattern mode
is shown and edited as its patterns. The status header gains a `Sparse:` line
listing the directories while it is on. Setting starts from the current
directories, filled in to edit. Every word of an answer of several words is
now checked for a leading dash, not only the first. Bundles are `n` (Magit
binds none; `n` is free in every view): create (`--all`, or revisions such as
`v1.0..main`; from the log, the commit under the cursor), verify, list heads,
and unbundle. Unbundling fetches the bundle's branches into
`refs/remotes/<name>/…`, since `git bundle unbundle` only stores objects and
names nothing. A bundle's file is never taken from the file under the cursor,
which writing the bundle would overwrite. What verify and list-heads print in
full is in the process output (`$`).

### Task 2.19: The Long Tail of Everyday Commands

Drawn from Magit's own grab-bag module. Small individually, and several are
used more often than most of the transients above.

- [x] Copy the revision or the section value at point, so a commit hash can be
      pasted somewhere without retyping it.
- [x] Abort whatever operation is in progress, without the user having to know
      whether it is a merge, a rebase, a cherry-pick, a revert or a bisect.
- [x] `git clean`: remove untracked files, with ignored files as a separate
      choice and a confirmation naming what goes.
- [x] Edit the commit that last touched the line at point, which is blame and
      interactive rebase used together.
- [x] Rewrite the author and committer dates of a range of commits.
- [x] Resolve a conflict with the user's configured mergetool, as an escape
      hatch from whatever Task 2.10 builds.
- [x] A revision stack: revisions recently looked at, insertable into a buffer
      — useful when writing a commit message that refers to another commit.

*Done.* `C-w` copies the value under the cursor in the status, log, blame and
commit views (a commit, stash, branch, tag or file) and `A-w` the view's own
revision (the commit shown, a range's newer end, or HEAD), into the default
yank register and the clipboard. Abort is `a` in the main menu and in the
resolve menu: it reads what is in progress and runs its `--abort`, or
`bisect reset`, after a confirmation naming the operation. `git clean` is the
`K` menu (untracked, ignored, both, always with `-d`): git is first asked with
`-n`, the confirmation names what would go, and nothing is asked when that is
nothing. Editing the commit of a line is `e` in the file dispatch and in the
blame view: the line is blamed, and an interactive rebase starts at the
commit's parent with a sequence editor that turns its `pick` into `edit`. A
pushed commit is refused (Task 2.4's rule), and so is a merge, which the rebase
would drop. Reshelving is `d` in the rebase menu: the commits after a revision
get new author and committer dates, the first at the date given (parsed by git
itself) and each next one a minute later, in the committer's zone, through a
`rebase --exec` that amends each commit; also refused once pushed. The
mergetool is `m` in the resolve menu and runs `git mergetool` on the file in
the integrated terminal, since the tool usually needs one; whatever the
terminal was running receives the line. The revision stack keeps the last 30
commits opened or copied, newest first; `:magit-insert-revision`, or `r` in
the file dispatch, inserts one at the cursors as `abc1234 ("Subject")`, RET
alone taking the newest.

*What is left after Task 2.19 is Emacs, not Git: Dired and bookmark
integration, `project.el` entry points, shift-selection variants of the
cursor-motion commands, and wrappers that launch `gitk` and `git gui`. These
have no meaning in Helix and are deliberately not listed as gaps.*

### Task 2.20: Commit Construction and Attribution

Three commit-building tools upstream offers that the fork has no equivalent of.
The first is small and used constantly; the other two are the reason people
describe Magit as letting them commit the way they think.

- [x] Trailers in the message buffer: insert `Signed-off-by`, `Co-authored-by`,
      `Reported-by` and the rest, with completion over people already in the
      history rather than retyping an address.
- [x] Absorb: take the staged changes and fold each hunk into whichever earlier
      commit introduced the lines it touches, instead of one catch-all fixup.
      This needs blame per hunk and then an autosquash, so it depends on
      Tasks 2.11 and 2.7.
- [x] Autofixup: the same idea driven from the diff rather than from blame.
- [x] Both rewrite history, so they inherit Task 2.4's rule about not amending
      what is already pushed, and need the same confirmation.

*Done.* `:magit-trailer [kind] [person]` adds a trailer to the message
buffer, asking for what is missing: the kind from the usual ten (a part of the
name is enough, `signed`; an ambiguous one says what it could be), the person
from the authors and committers of the last 2000 commits, most frequent first —
the user first for a sign-off, and never for `Co-authored-by`. It goes where
`git interpret-trailers` would put it: into the last paragraph when that is
already trailers, into a paragraph of its own otherwise, above git's comment
lines and the `--verbose` diff, and not twice. The message template mentions
it. Absorb and autofixup are `x` and `X` in the commit menu (Magit's keys).
Both split the staged diff into hunks and look each one up with a blame of
HEAD, among the commits on no remote only: absorb takes the commit that last
changed every line the hunk removes (the lines either side, for a hunk that
only adds), on `-U0` hunks, and then squashes the `fixup!` commits in with an
autosquash rebase that sets uncommitted changes aside and stages again what
was left staged; autofixup takes the one unpushed commit among those that last
changed the hunk's lines, context included (git-autofixup's default), and stops
at the `fixup!` commits for review. A hunk with no such commit, or several,
stays staged, as do new and binary files. Each fixup commit is built in the
index from the original HEAD, so the working tree is untouched until the
rebase, and a failure puts HEAD and the index back. Pushed commits are never
candidates, a merge after the oldest target is refused, and both are behind
the usual confirmation.

---

## Phase 3: Integrated Terminal (`crates/helix-pty`)

*Tasks 3.3 to 3.7 were derived from the embedded emulator itself: the 71
operations of `vte`'s handler trait, the 13 event variants
`alacritty_terminal` reports, and the defaults of the `Config` the fork passes
it — checked against what the integration does with each, rather than from a
list of things terminals generally have.*

### Task 3.1: PTY Engine & VT100 Emulator
- [x] Create `crates/helix-pty` in the workspace.
- [x] Add `portable-pty` and `alacritty_terminal` dependencies.
- [x] Implement `PtyTerminal` wrapper managing shell spawning (`SHELL`), PTY I/O, and grid state thread-safety (`Arc<Mutex<Term>>`).
- [x] Set up background reader thread forwarding PTY ANSI output to Alacritty's processor.

### Task 3.2: `TerminalView` UI Component
- [x] Implement `TerminalView` component in `helix-term`.
- [x] Translate Alacritty grid cells to Helix `Surface` rendering calls.
- [x] Implement full key pass-through from `crossterm` to the PTY writer.
- [x] Implement escape key sequence (e.g., `Ctrl-a Esc`) to toggle focus back to Helix normal mode.
- [x] Handle dynamic terminal resizing (`Pty::resize`).

### Task 3.3: The Emulator Events the Integration Drops

`alacritty_terminal` reports thirteen kinds of event. The integration handles
three — the write-back that keeps a program from hanging on a query, the
wakeup, and the bell, which only triggers a redraw — and discards the rest in a
catch-all arm. Shell exit is covered separately, through the reader's
end-of-file. The emulation itself is complete, because `Term` implements all 71
operations of the handler; everything below is the embedder's half.

- [x] Title and reset-title: the shell and the programs in it say what they are
      doing, and the view shows nothing. This is how a user tells one terminal
      from another and is the cheapest item here.
- [x] `ClipboardStore` (OSC 52): a program asking for text to be put on the
      system clipboard, which is how copying works over ssh.
- [x] `ClipboardLoad`: the same in reverse, and a decision rather than a task —
      it lets a program *read* the user's clipboard. The crate has a policy
      setting for this; the fork should choose deliberately rather than inherit
      a default.
- [x] `ColorRequest`: programs that ask the terminal for its palette in order
      to pick readable colours. Unanswered, they guess, which is why some
      programs are unreadable on some themes.
- [x] `TextAreaSizeRequest`: reports the area in pixels. The view only knows
      cells, so this needs the cell size in pixels from the frontend, or a
      documented refusal.
- [x] `CursorBlinkingChange` and `MouseCursorDirty`, both presentation.
- [x] The bell currently only redraws. Decide what it should do — a visible
      flash, a status message, nothing — and make it a choice rather than an
      accident.

*Done.* The view has a one-line title bar showing the title the running
program set (OSC 0/2; reset falls back to "Terminal") beside the way back,
`Ctrl-\ Ctrl-n`; the terminal itself is a line shorter for it. Text copied
with OSC 52 goes to the `+` register (the `*` one for the primary selection)
with a status message, so it can be pasted into a document. The emulator is
configured with `Osc52::OnlyCopy` explicitly rather than by default: a program
can copy, never read the clipboard, so `ClipboardLoad` never arrives, and
Task 3.7 is where a setting for it would go. Colour requests are answered
where the answer is known: the 240 fixed palette entries (the xterm cube and
grey ramp, which every terminal shares) and the default foreground and
background when the theme gives them as RGB — the view tells the emulator
each frame. The sixteen named colours are drawn with the host terminal's own
palette, which cannot be read from here, so those requests stay unanswered and
the program keeps its guess rather than getting a wrong answer. The text area
size is reported in cells, with zero for the pixel size, which is how a
terminal says it does not know. Cursor blinking and the mouse pointer's shape
are deliberately ignored: the view draws a steady block cursor, and a text
interface has no pointer to shape. The bell flashes the title bar for 200 ms
(in the theme's warning colour) instead of only redrawing; the flash itself
was not caught on screen by the test harness, whose snapshots come after it
ends.

### Task 3.4: Scrollback, Selection and Search

`Term` is built with `Config::default()`, which keeps **10,000 lines** of
scrollback. Nothing in the view can reach them: there is no display offset, no
scroll command, no selection. That is both a missing feature and a memory cost
per terminal that nobody chose.

The crate already ships `selection.rs`, `search.rs` and `vi_mode.rs`. These are
not to be written, only wired.

- [x] Scroll through the scrollback, and choose how much of it to keep.
- [x] Select text with the keyboard, and copy it into a Helix register so it
      can be pasted into a document.
- [x] Search the scrollback, which is the crate's `search` module.
- [x] Vi-mode navigation of the grid, which the crate implements and which
      would fit this fork's keymap better than it fits Alacritty's.

*Done.* The scrollback is `[editor.integrated-terminal] scrollback` (10,000
lines by default, now a stated choice rather than Alacritty's inherited one,
documented as a per-terminal memory cost). `Shift-PageUp` and `Shift-PageDown`
scroll without leaving the shell, and typing returns to the bottom. Copy mode
is `Ctrl-\ [` (tmux's prefix-and-bracket) and is Alacritty's own vi mode,
wired rather than written: `helix_pty::copy` drives its cursor motions,
selections (characters, lines, block, following the cursor), regex search and
selection text, and has tests against an emulator fed real output. The keys
are vi's, with Helix's `x` accepted for a line selection, and counts; `y`
copies into the default yank register, so `p` pastes it into a document. The
view draws the scrolled-back lines, the selection (`ui.selection`) and the
current match (`ui.selection.primary`), shows the copy-mode cursor, and says
`[copy]` and the search in its title bar. Searching skips the match the cursor
is in and wraps at both ends of the scrollback; Alacritty's regex search does
not anchor `^` and `$` to lines, which the book says.

### Task 3.5: Input Beyond xterm Keys

`keys.rs` encodes keys the xterm way, including DECCKM. That is the floor, not
the ceiling.

- [ ] The Kitty keyboard protocol: the handler has push, pop, set and report
      operations for it and the crate has a config flag. Without it a program
      cannot tell `Ctrl-I` from `Tab`, or see a key release — which Helix
      itself asks for when run inside a terminal.
- [ ] Mouse reporting: forward clicks, drags and wheel to a program that asked
      for them. Until then `htop`, `tmux` and an inner editor are keyboard-only.
- [ ] Bracketed paste: paste a Helix register into the shell as a paste rather
      than as typing, so a shell does not run half of it on the first newline.

### Task 3.6: More Than One Terminal

`Editor::terminal` is a single `Option`, so the fork has exactly one shell.
Reopening `:terminal` returns to it, which was a deliberate improvement over
killing it, but it is still one.

- [ ] Several terminals, listed and switchable.
- [ ] Name them, or show what each is running, which needs Task 3.3's title.
- [ ] Close one explicitly, rather than only by exiting its shell.
- [ ] Decide what a terminal's working directory follows: the document at the
      time it was opened, as now, or the current one.

### Task 3.7: Terminal Configuration

`TerminalConfig` holds a command and its arguments. The emulator is built with
`Config::default()`, so everything it can be told is currently left unsaid.

- [ ] Scrollback size, cursor style, and the word characters used when
      selecting by word.
- [ ] The OSC 52 clipboard policy from Task 3.3, and whether the Kitty
      keyboard protocol from Task 3.5 is offered.
- [ ] Environment variables for the shell, and whether `TERM` should claim
      something other than what the crate advertises.
- [ ] These belong in Helix's own configuration idiom rather than as a copy of
      Alacritty's, and the mapping is the work.

---

## Phase 4: Commands & Keybindings

### Task 4.1: Command Registration & Shortcuts
- [x] Register commands in `helix-term/src/commands.rs`:
  - `:roam-node-find`, `:roam-backlinks`
  - `:magit`
  - `:terminal`
- [x] Add default space-leader keybindings in `helix-term/src/keymap/default.rs`:
  - `space + n + f` -> Roam Find Node
  - `space + n + b` -> Roam Toggle Backlinks
  - `space + m`     -> Open Magit Status
  - `space + t`     -> Open Terminal
  - `z a` / `z f` / `z o` / `z M` / `z R` -> folding (Task 1.4)
  - `z tab` / `z S-tab` -> Org's visibility cycling (Task 1.4)

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
