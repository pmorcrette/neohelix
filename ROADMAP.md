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
- [ ] Jump from an agenda line to its headline, and act on it in place
      (change state, reschedule) without losing the agenda.

  Jumping works. Acting *in place* does not, and the reason is structural
  rather than unfinished: Helix's `Picker` consumes its own keys and offers no
  hook for an action that leaves it open, so this needs either a patch to an
  upstream file or an agenda component of the fork's own — which is the
  agenda buffer Org has, and what Phase 5's docked pane would host.
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
- [ ] Dailies come in two forms upstream, and the difference matters: *goto*
      opens the day's note, *capture* adds an entry to it through a template
      without leaving the current buffer. Both, plus opening the dailies
      directory itself.

  Only *goto* is built. `daily_directory` resolves the path but no command
  opens it, and the capture form — the one that does not move the cursor — is
  the half that is missing.
- [x] Unlinked references: occurrences of a node's title or alias in other
      files that are not yet links, and a way to turn one into a link.
- [x] Renaming a node's title, updating the link descriptions that named it.

### Task 1.9: Export, Babel and Clocking

The far horizon: large, self-contained, and none of it needed for the notes
workflow the fork is built around. Listed so the gap is explicit rather than
forgotten.

- [ ] Export to HTML, Markdown and LaTeX.
- [ ] Source block execution (Babel), which is an arbitrary-code-execution
      surface and needs a trust decision before a single line is written.
- [ ] Clocking: clock in and out, `:LOGBOOK:` drawers, and time reports.
- [ ] Attachments and column view.

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
- [ ] Insert a drawer, and fold drawers by default the way Org does.
      Inserting is built. Folding is no longer blocked — Task 1.4 landed, and
      `folds.scm` already marks `(property_drawer)` and `(drawer)` — but
      *by default* means folding them when a file opens, which needs the
      `#+STARTUP:` options of Task 1.21 to say so.
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
- [ ] Copy only the visible text of a region, so a folded outline can be shared
      as an outline. Unblocked by Task 1.4: there is now such a thing as
      visible text, and `Folds::hidden` says which characters are not.
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

- [ ] Edit a source block with the full language tooling — LSP, completion,
      diagnostics, formatting — rather than only its highlighting.
- [ ] Decide the mechanism: a scratch buffer bound to the block and written
      back, or making the language server see the block in place. The first is
      how Emacs does it; the second is better and harder.
- [ ] Navigate between blocks, and between a block and its result.
- [ ] Tangling: write the blocks out to their target files, which is the half of
      literate programming that needs no code execution and no trust decision.

### Task 1.16: The Optional Modules Worth Having

Org ships a long list of optional modules. Most are links into Emacs
applications and mean nothing here; these are the ones that do.

- [ ] Habits: a repeating task with a consistency graph in the agenda.
- [ ] TODO dependencies: an entry that cannot be done before its children or a
      named other entry.
- [ ] Inline tasks: a task that does not break the outline it sits in.
- [ ] Encrypted subtrees.
- [ ] A protocol handler, so a browser or another program can capture into the
      notes directory. Org-Roam users lean on this heavily.

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

- [x] Diagnose the node at point: what the index believes about it, which is
      the only way to tell a parser bug from a malformed drawer.

  It reports the title twice, from the buffer and from the index, and says
  whether the buffer has changed since the index last read it — which is the
  answer most of the time, and the one that stops the hunt for a parser bug
  that is not there.

### Task 1.20: Graph Visualisation and Export

Two extensions upstream ships that the fork has no equivalent of.

- [ ] Render the graph — the whole one, or a neighbourhood around a node at a
      chosen depth — and open it. Upstream shells out to Graphviz; doing the
      same avoids a layout engine in-process, at the cost of a dependency the
      fork can detect and report rather than require.
- [ ] Export: resolve `id:` links to something meaningful in the exported
      output rather than leaving a raw UUID. This is a prerequisite for
      Task 1.9's export being useful on a notes directory at all.

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
- [ ] `#+SETUPFILE:`, which pulls settings in from another file. This one is
      structural: it means a file's meaning depends on a second file, and the
      indexer currently reparses exactly one file per save. A setupfile change
      has to invalidate every file that includes it.
- [x] `#+PROPERTY:` for file-level property defaults, which Task 1.11's
      inheritance builds on.
- [x] `#+TAGS:` and `#+PRIORITIES:`, which define the tag alist and the
      priority range a file uses. Priority cookies are currently stripped for
      any letter, without knowing the declared range.
- [x] `#+CATEGORY:` and `#+ARCHIVE:`, needed by Tasks 1.7 and 1.5 respectively.
- [x] `#+DRAWERS:` for custom drawer names, so a drawer the file declares is
      not parsed as content.
- [ ] `#+STARTUP:` folding and visibility options (`overview`, `content`,
      `showeverything`, `hidedrawers`, `hideblocks`, …), which is how a file
      says how it wants to open. Task 1.4 built what these drive; the parser
      already collects them, and nothing applies them yet.
- [ ] `#+STARTUP:` logging options (`logdone`, `logdrawer`, `logrepeat`, …),
      which Task 1.11 needs to know what to record.
- [ ] The export and citation keywords — `#+OPTIONS:`, `#+INCLUDE:`,
      `#+MACRO:`, `#+BIBLIOGRAPHY:`, `#+CITE_EXPORT:` — belong with Tasks 1.9
      and 1.14 rather than here.
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
The fork has one, `roam-reindex`, and no way to look at what the index
believes.

- [ ] Rebuild the whole index from scratch, distinct from reindexing one file,
      for when an incremental update has gone wrong.
- [ ] Resolve `id:` links whose target lives outside the notes directory.
      Upstream keeps a separate cache of id locations for exactly this; the
      fork's graph only knows the files it scanned, so such a link silently
      fails to resolve.
- [ ] Browse the index: which nodes, links and refs it holds, as a way of
      telling a parser bug from a malformed file.
- [ ] Report the fork's own state — versions, notes directory, file and node
      counts — so a bug report can carry it.

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
- [ ] `Rebase`: `--interactive` needs `GIT_SEQUENCE_EDITOR` pointed back at
      Helix, which means the integrated terminal or a spawned instance; decide
      which before starting.
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
- [ ] Apply (`a`) and reverse (`v`) the hunk or selection at point, the two
      operations that share discard's reverse-apply machinery.

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
- [ ] Subtree (`O`), which is separate from the submodule and worktree work
      above.
- [ ] Notes (`T`): add, edit and remove git notes.
- [ ] Show refs (`y`): every branch and tag with its relationship to `HEAD`.
- [ ] Cherries (`Y`): the commits one branch has that another does not.
- [ ] Gitignore helpers (`i`): ignore the file at point, in the repository's
      `.gitignore` or in the private exclude file.
- [ ] Branch management the Task 2.4 menu left out: rename, reset, `spinoff`
      and `spinout`.
- [ ] The configuration transients: a branch's own git config (upstream, rebase
      behaviour, description) and a remote's, both of which upstream gives a
      menu of their own rather than a single "set upstream" action.
- [ ] Shortlog: who contributed what over a range.
- [ ] Fetch submodules as an operation distinct from fetching the repository.

### Task 2.10: Conflict Resolution

Task 2.9 lists merge but explicitly excludes resolving the conflicts it
produces, because that is a buffer and a workflow rather than a command line.
Magit reaches for Ediff here (`e`, `E`); the fork has no equivalent and needs
its own answer.

- [ ] A conflicts section in the status buffer, listing unmerged paths and
      their stage.
- [ ] Open a conflicted file with the three sides available — ours, theirs and
      the base — and a way to take either side for a region.
- [ ] Mark a path resolved, and drive the surrounding operation to its end
      (`merge --continue`, `rebase --continue`, `cherry-pick --continue`).
- [ ] This applies to every operation that can stop on a conflict, not just
      merge, so it belongs with none of them in particular.

### Task 2.11: File-Scoped Commands and Blame

Magit has a second dispatch for the file you are editing, reachable without
opening the status buffer at all. The fork has nothing equivalent: every Git
operation currently starts from `<space>m`.

- [ ] A file dispatch: stage, unstage, diff, log and blame for the current
      buffer's file.
- [ ] Blame: annotate each line with its commit, author and date, and move
      between revisions of the same line.
- [ ] Log and diff restricted to the current file, which Task 2.8 should build
      on rather than duplicate.

### Task 2.12: Diff Presentation Controls

The `DiffView` renders with fixed settings. Magit puts these on a transient
(`d`, `D`) because the right answer changes with the diff you are reading.

- [ ] Context lines, adjustable while reading.
- [ ] Whitespace handling (`-w`, `--ignore-space-change`), which is the
      difference between a readable diff and an unreadable one after a
      reindent.
- [ ] Diff algorithm (`--histogram`, `--patience`).
- [ ] Word-level diff, and `--stat` as a summary view.
- [ ] Diff against an arbitrary revision or between two, rather than only
      worktree-against-index and index-against-`HEAD`.

### Task 2.13: Repository Entry Points and Arbitrary Commands

Small dispatch entries with nowhere else to sit; each is short on its own.

- [ ] Clone (`C`) and init (`I`).
- [ ] Jump to a section of the status buffer (`j`), and switch between the
      fork's Git buffers (`J`).
- [ ] Run an arbitrary git command in the repository (`Q`) and a shell command
      (`!`), both reporting into Task 2.9's process buffer.

### Task 2.14: The Transient Arguments Left Out

Task 2.4 shipped 19 arguments across five menus. These are the ones Magit
offers that it did not.

- [ ] Commit: `--gpg-sign=`, `--date=`, `--reset-author`.
- [ ] Push: tags and explicit refspecs.
- [ ] Rebase: `--onto` an arbitrary revision, rather than only the upstream.
- [ ] The commit menu offers `--verbose`, which asks git to put the diff in the
      message buffer. Task 2.4 supplies the message with `-F`, so no editor
      runs and the switch currently does nothing. Either the fork seeds the
      diff into the buffer itself, or the switch should go; leaving a flag that
      silently does nothing is the worst of the three.

### Task 2.15: The Reflog

Nothing in the fork exposes the reflog, and it is the one thing that gets a
user's work back after a bad reset, a dropped branch or a rebase that went
wrong. Every destructive action Tasks 2.4 and 2.9 put behind a confirmation is
recoverable through it, which is what makes its absence worth a task of its
own.

- [ ] A reflog buffer for `HEAD` and for a chosen ref.
- [ ] Act on the entry at point: check it out, reset to it, or create a branch
      there.
- [ ] Reach it from the status buffer, so it is findable at the moment it is
      needed rather than only by someone who knows it exists.

### Task 2.16: Work-in-Progress Refs

Magit can commit the working tree and the index to hidden refs on every save
and every operation, so that uncommitted work is recoverable even though it was
never committed. It is off by default there and would be here too.

- [ ] Write worktree and index wip refs on save and before destructive
      operations.
- [ ] A log over the wip refs, and a way to restore from one.
- [ ] Decide the default. This writes to the repository on every save, so it is
      opt-in or it is a surprise.

### Task 2.17: Repository List, Buffer Freshness and the Margin

Three separate modules that all concern how the fork's Git buffers behave
rather than what they run.

- [ ] A repository list: the repositories the user works in, with their branch
      and how far ahead or behind each is.
- [ ] Refresh open buffers when git changes files underneath them. A checkout
      or a reset currently leaves every open document stale, which is a
      correctness problem, not a convenience one.
- [ ] A margin alongside log and status lines showing the author and the age of
      each commit, and a way to toggle it.

### Task 2.18: Sparse Checkout and Bundles

Two self-contained areas with no equivalent in the fork. Neither is needed for
the daily loop; both are the kind of thing whose absence is only discovered at
the moment it is wanted.

- [ ] Sparse checkout: enable it, list and edit the directories included.
- [ ] Bundles: create a bundle from a range of commits, and unbundle one.

### Task 2.19: The Long Tail of Everyday Commands

Drawn from Magit's own grab-bag module. Small individually, and several are
used more often than most of the transients above.

- [ ] Copy the revision or the section value at point, so a commit hash can be
      pasted somewhere without retyping it.
- [ ] Abort whatever operation is in progress, without the user having to know
      whether it is a merge, a rebase, a cherry-pick, a revert or a bisect.
- [ ] `git clean`: remove untracked files, with ignored files as a separate
      choice and a confirmation naming what goes.
- [ ] Edit the commit that last touched the line at point, which is blame and
      interactive rebase used together.
- [ ] Rewrite the author and committer dates of a range of commits.
- [ ] Resolve a conflict with the user's configured mergetool, as an escape
      hatch from whatever Task 2.10 builds.
- [ ] A revision stack: revisions recently looked at, insertable into a buffer
      — useful when writing a commit message that refers to another commit.

*What is left after Task 2.19 is Emacs, not Git: Dired and bookmark
integration, `project.el` entry points, shift-selection variants of the
cursor-motion commands, and wrappers that launch `gitk` and `git gui`. These
have no meaning in Helix and are deliberately not listed as gaps.*

### Task 2.20: Commit Construction and Attribution

Three commit-building tools upstream offers that the fork has no equivalent of.
The first is small and used constantly; the other two are the reason people
describe Magit as letting them commit the way they think.

- [ ] Trailers in the message buffer: insert `Signed-off-by`, `Co-authored-by`,
      `Reported-by` and the rest, with completion over people already in the
      history rather than retyping an address.
- [ ] Absorb: take the staged changes and fold each hunk into whichever earlier
      commit introduced the lines it touches, instead of one catch-all fixup.
      This needs blame per hunk and then an autosquash, so it depends on
      Tasks 2.11 and 2.7.
- [ ] Autofixup: the same idea driven from the diff rather than from blame.
- [ ] Both rewrite history, so they inherit Task 2.4's rule about not amending
      what is already pushed, and need the same confirmation.

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

- [ ] Title and reset-title: the shell and the programs in it say what they are
      doing, and the view shows nothing. This is how a user tells one terminal
      from another and is the cheapest item here.
- [ ] `ClipboardStore` (OSC 52): a program asking for text to be put on the
      system clipboard, which is how copying works over ssh.
- [ ] `ClipboardLoad`: the same in reverse, and a decision rather than a task —
      it lets a program *read* the user's clipboard. The crate has a policy
      setting for this; the fork should choose deliberately rather than inherit
      a default.
- [ ] `ColorRequest`: programs that ask the terminal for its palette in order
      to pick readable colours. Unanswered, they guess, which is why some
      programs are unreadable on some themes.
- [ ] `TextAreaSizeRequest`: reports the area in pixels. The view only knows
      cells, so this needs the cell size in pixels from the frontend, or a
      documented refusal.
- [ ] `CursorBlinkingChange` and `MouseCursorDirty`, both presentation.
- [ ] The bell currently only redraws. Decide what it should do — a visible
      flash, a status message, nothing — and make it a choice rather than an
      accident.

### Task 3.4: Scrollback, Selection and Search

`Term` is built with `Config::default()`, which keeps **10,000 lines** of
scrollback. Nothing in the view can reach them: there is no display offset, no
scroll command, no selection. That is both a missing feature and a memory cost
per terminal that nobody chose.

The crate already ships `selection.rs`, `search.rs` and `vi_mode.rs`. These are
not to be written, only wired.

- [ ] Scroll through the scrollback, and choose how much of it to keep.
- [ ] Select text with the keyboard, and copy it into a Helix register so it
      can be pasted into a document.
- [ ] Search the scrollback, which is the crate's `search` module.
- [ ] Vi-mode navigation of the grid, which the crate implements and which
      would fit this fork's keymap better than it fits Alacritty's.

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
