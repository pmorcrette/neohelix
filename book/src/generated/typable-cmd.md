| Name | Description |
| --- | --- |
| `:exit`, `:x`, `:xit` | Write changes to disk if the buffer is modified and then quit. Accepts an optional path (:exit some/path.txt). |
| `:exit!`, `:x!`, `:xit!` | Force write changes to disk, creating necessary subdirectories, if the buffer is modified and then quit. Accepts an optional path (:exit! some/path.txt). |
| `:quit`, `:q` | Close the current view. |
| `:quit!`, `:q!` | Force close the current view, ignoring unsaved changes. |
| `:open`, `:o`, `:edit`, `:e` | Open a file from disk into the current view. |
| `:buffer-close`, `:bc`, `:bclose` | Close the current buffer. |
| `:buffer-close!`, `:bc!`, `:bclose!` | Close the current buffer forcefully, ignoring unsaved changes. |
| `:buffer-close-others`, `:bco`, `:bcloseother` | Close all buffers but the currently focused one. |
| `:buffer-close-others!`, `:bco!`, `:bcloseother!` | Force close all buffers but the currently focused one. |
| `:buffer-close-all`, `:bca`, `:bcloseall` | Close all buffers without quitting. |
| `:buffer-close-all!`, `:bca!`, `:bcloseall!` | Force close all buffers ignoring unsaved changes without quitting. |
| `:buffer-next`, `:bn`, `:bnext` | Goto next buffer. |
| `:buffer-previous`, `:bp`, `:bprev` | Goto previous buffer. |
| `:write`, `:w` | Write changes to disk. Accepts an optional path (:write some/path.txt) |
| `:write!`, `:w!` | Force write changes to disk creating necessary subdirectories. Accepts an optional path (:write! some/path.txt) |
| `:write-buffer-close`, `:wbc` | Write changes to disk and closes the buffer. Accepts an optional path (:write-buffer-close some/path.txt) |
| `:write-buffer-close!`, `:wbc!` | Force write changes to disk creating necessary subdirectories and closes the buffer. Accepts an optional path (:write-buffer-close! some/path.txt) |
| `:new`, `:n` | Create a new scratch buffer. |
| `:format`, `:fmt` | Format the file using an external formatter or language server. |
| `:indent-style` | Set the indentation style for editing. ('t' for tabs or 1-16 for number of spaces.) |
| `:line-ending` | Set the document's default line ending. Options: crlf, lf. |
| `:earlier`, `:ear` | Jump back to an earlier point in edit history. Accepts a number of steps or a time span. |
| `:later`, `:lat` | Jump to a later point in edit history. Accepts a number of steps or a time span. |
| `:write-quit`, `:wq` | Write changes to disk and close the current view. Accepts an optional path (:wq some/path.txt) |
| `:write-quit!`, `:wq!` | Write changes to disk and close the current view forcefully. Accepts an optional path (:wq! some/path.txt) |
| `:write-all`, `:wa` | Write changes from all buffers to disk. |
| `:write-all!`, `:wa!` | Forcefully write changes from all buffers to disk creating necessary subdirectories. |
| `:write-quit-all`, `:wqa`, `:xa` | Write changes from all buffers to disk and close all views. |
| `:write-quit-all!`, `:wqa!`, `:xa!` | Forcefully write changes from all buffers to disk, creating necessary subdirectories, and close all views (ignoring unsaved changes). |
| `:quit-all`, `:qa` | Close all views. |
| `:quit-all!`, `:qa!` | Force close all views ignoring unsaved changes. |
| `:cquit`, `:cq` | Quit with exit code (default 1). Accepts an optional integer exit code (:cq 2). |
| `:cquit!`, `:cq!` | Force quit with exit code (default 1) ignoring unsaved changes. Accepts an optional integer exit code (:cq! 2). |
| `:theme` | Change the editor theme (show current theme if no name specified). |
| `:yank-join` | Yank joined selections. A separator can be provided as first argument. Default value is newline. |
| `:clipboard-yank` | Yank main selection into system clipboard. |
| `:clipboard-yank-join` | Yank joined selections into system clipboard. A separator can be provided as first argument. Default value is newline. |
| `:primary-clipboard-yank` | Yank main selection into system primary clipboard. |
| `:primary-clipboard-yank-join` | Yank joined selections into system primary clipboard. A separator can be provided as first argument. Default value is newline. |
| `:clipboard-paste-after` | Paste system clipboard after selections. |
| `:clipboard-paste-before` | Paste system clipboard before selections. |
| `:clipboard-paste-replace` | Replace selections with content of system clipboard. |
| `:primary-clipboard-paste-after` | Paste primary clipboard after selections. |
| `:primary-clipboard-paste-before` | Paste primary clipboard before selections. |
| `:primary-clipboard-paste-replace` | Replace selections with content of system primary clipboard. |
| `:show-clipboard-provider` | Show clipboard provider name in status bar. |
| `:change-current-directory`, `:cd` | Change the current working directory. |
| `:show-directory-stack` | Show the directory stack as a <space> delimited string. |
| `:push-directory`, `:pushd` | Save and then change the current directory. |
| `:pop-directory`, `:popd` | Remove the top entry from the directory stack, and cd to the new top directory.. |
| `:show-directory`, `:pwd` | Show the current working directory. |
| `:encoding` | Set encoding. Based on `https://encoding.spec.whatwg.org`. |
| `:character-info`, `:char` | Get info about the character under the primary cursor. |
| `:terminal`, `:term` | Open the integrated terminal. |
| `:magit` | Open the Magit transient menu. |
| `:magit-file` | Open the Magit menu for the current file: stage, unstage, diff, log, blame. |
| `:magit-trailer` | Add a trailer (Signed-off-by, Co-authored-by, …) to the commit message, choosing from people in the history. |
| `:magit-insert-revision` | Insert a revision looked at recently (a commit opened or copied) as `hash ("subject")`. |
| `:conflict-take` | Resolve the merge conflict under the cursor with ours, theirs, base or both. |
| `:rebase-todo` | In a rebase todo-list, set the selected lines to pick, reword, edit, squash, fixup or drop, or move them up or down. |
| `:roam-node-find`, `:rnf` | Open the Org-Roam node picker. |
| `:roam-backlinks-toggle`, `:roam-backlinks` | Show or hide the Org-Roam backlinks panel. |
| `:roam-promote-buffer`, `:roam-promote` | Turn a buffer holding one heading into an Org-Roam file node. |
| `:roam-demote-buffer`, `:roam-demote` | Turn an Org-Roam file node into a single heading holding the file. |
| `:roam-extract-subtree`, `:roam-extract` | Extract the subtree at the cursor into a node of its own. |
| `:roam-replace-links` | Rewrite this buffer's legacy roam: links as id: links. |
| `:org-follow-link`, `:org-open` | Follow the Org link under the cursor. |
| `:org-store-link` | Store a link to the cursor's location, for inserting elsewhere. |
| `:org-insert-link` | Insert the stored Org link at the cursor. |
| `:org-create-id`, `:org-id-get-create` | Give the entry at the cursor an :ID: so it can be linked to. |
| `:roam-node-insert`, `:roam-insert` | Insert a link to an Org-Roam node, creating it if the title is new. |
| `:roam-ref-find` | Find an Org-Roam node by one of its :ROAM_REFS: keys. |
| `:roam-random-node`, `:roam-random` | Open a random Org-Roam node. |
| `:roam-alias-add` | Add an alias to the node at the cursor. |
| `:roam-alias-remove` | Remove an alias from the node at the cursor. |
| `:org-insert-heading` | Insert a heading after the current subtree. |
| `:org-promote` | Promote the headline at the cursor. |
| `:org-demote` | Demote the headline at the cursor. |
| `:org-promote-subtree` | Promote the subtree at the cursor. |
| `:org-demote-subtree` | Demote the subtree at the cursor. |
| `:org-move-subtree-up` | Move the subtree above its sibling. |
| `:org-move-subtree-down` | Move the subtree below its sibling. |
| `:org-todo` | Cycle the TODO state forward, using the file's keywords. |
| `:org-todo-previous` | Cycle the TODO state backward. |
| `:org-priority-up` | Raise the priority towards [#A]. |
| `:org-priority-down` | Lower the priority. |
| `:org-todo-filtered`, `:org-todo-filter` | List unfinished tasks matching a keyword, tag or priority. |
| `:org-agenda-restrict` | Restrict the agenda to the file in this buffer. |
| `:org-agenda-unrestrict` | Lift the agenda restriction. |
| `:org-agenda-scope` | Say which files the agenda currently reads. |
| `:org-agenda`, `:org-agenda-day` | Show what is due today. |
| `:org-agenda-week` | Show what is due over the next seven days. |
| `:org-todo-list`, `:org-todos` | List every unfinished task, whatever its dates. |
| `:org-insert-item` | Insert a list item after the one at the cursor. |
| `:org-renumber-list` | Renumber the ordered list at the cursor. |
| `:org-demote-item` | Move the list item in a level, with its children. |
| `:org-promote-item` | Move the list item out a level, with its children. |
| `:org-toggle-checkbox`, `:org-toggle` | Tick or untick the checkbox at the cursor. |
| `:org-update-cookies` | Bring every [n/m] and [p%] cookie up to date. |
| `:narrow-to-selection`, `:narrow` | Hide every line outside the selection. |
| `:cycle-fold` | Step the range at the cursor through folded, children, open. |
| `:cycle-fold-all` | Step the buffer through overview, contents, everything. |
| `:fold` | Fold the innermost foldable range at the cursor. |
| `:unfold` | Open the fold at the cursor. |
| `:toggle-fold` | Close the fold at the cursor, or open it. |
| `:fold-all` | Fold everything the language marks as foldable. |
| `:unfold-all` | Open every fold in the buffer. |
| `:roam-index`, `:roam-browse` | Look through everything the index holds. |
| `:roam-state` | Report the fork's Org-Roam state for a bug report. |
| `:roam-backlink-counts`, `:roam-counts` | Show each headline's backlink count beside it. |
| `:roam-pin` | Pin the Roam panel to the node at the cursor. |
| `:roam-unpin` | Let the Roam panel follow the cursor again. |
| `:roam-diagnose`, `:roam-doctor` | Report what the index believes about the node at the cursor. |
| `:org-emphasis` | Toggle an emphasis marker on the selection. |
| `:org-insert-block`, `:org-block` | Insert a structure block, wrapping the selection. |
| `:org-footnote-new` | Add a footnote and go to where its text goes. |
| `:org-footnote-goto` | Jump between a footnote's reference and definition. |
| `:org-footnote-renumber` | Renumber the numeric footnotes in reference order. |
| `:org-cite-insert`, `:org-cite` | Insert a citation, completing over the bibliography. |
| `:org-cite-follow` | Open the bibliography at the cited entry. |
| `:org-next-heading` | Move to the next heading. |
| `:org-previous-heading` | Move to the previous heading. |
| `:org-next-sibling-heading` | Move to the next heading at the same level. |
| `:org-previous-sibling-heading` | Move to the previous heading at the same level. |
| `:org-parent-heading`, `:org-up-heading` | Move to the parent heading. |
| `:org-goto-heading`, `:org-goto` | Jump to a heading in this buffer by name. |
| `:org-outline-path` | Show the outline path of the entry at the cursor. |
| `:org-sparse-tree`, `:org-match` | Hide everything but the entries matching a filter. |
| `:org-narrow` | Hide everything outside the subtree at the cursor. |
| `:org-widen` | Bring back everything a narrowing or sparse tree hid. |
| `:org-startup-visibility` | Fold the buffer the way its #+STARTUP: says it opens. |
| `:org-src-next` | Move to the next source block. |
| `:org-src-previous` | Move to the previous source block. |
| `:org-src-result` | Jump between a source block and its #+RESULTS:. |
| `:org-edit-src` | Edit the source block at the cursor in a buffer of its own. |
| `:org-tangle` | Write every block with a :tangle target to its file. |
| `:org-clock-in` | Start a clock on the entry at the cursor, stopping any other. |
| `:org-clock-out` | Stop the running clock. |
| `:org-clock-cancel` | Discard the running clock. |
| `:org-clock-goto` | Jump to the entry with the running clock. |
| `:org-clock-report` | Insert or refresh a clock report table. |
| `:org-export` | Export the buffer next to its file: md, html (the default) or latex. |
| `:org-babel-execute` | Run the source block at the cursor and write its results (needs workspace trust). |
| `:org-columns` | Show or hide the column view of the buffer, from its #+COLUMNS:. |
| `:roam-dailies-capture` | Add an entry to today's daily note without leaving this buffer. |
| `:roam-dailies-directory` | Pick a file in the dailies directory. |
| `:org-copy-visible` | Yank only the visible text of the selections (or of the buffer) to the default register. |
| `:org-encrypt-entry` | Encrypt the body of the entry at the cursor with gpg. |
| `:org-encrypt-entries` | Encrypt every :crypt: entry of the buffer that is in clear. |
| `:org-decrypt-entry` | Decrypt the entry at the cursor. |
| `:org-inline-task` | Insert an inline task, with its END line, below the cursor. |
| `:roam-graph` | Draw the Org-Roam graph with Graphviz; with a depth, only the nodes that many links from the one at the cursor. |
| `:org-attach` | Copy a file into the attachment directory of the entry at the cursor. |
| `:org-attach-open` | Pick one of the files attached to the entry at the cursor. |
| `:org-copy-subtree` | Copy the subtree at the cursor. |
| `:org-cut-subtree` | Cut the subtree at the cursor. |
| `:org-paste-subtree` | Paste the copied subtree at the cursor's level. |
| `:org-clone-subtree` | Clone the subtree at the cursor, shifting its dates. |
| `:org-sort-entries`, `:org-sort` | Sort the children of the entry at the cursor. |
| `:org-sort-list` | Sort the list items at the cursor. |
| `:org-sort-table` | Sort the table rows by the cursor's column. |
| `:org-dblock-update`, `:org-update-block` | Regenerate the dynamic block at the cursor. |
| `:org-dblock-update-all` | Regenerate every dynamic block in the buffer. |
| `:org-table-align`, `:org-align` | Realign the Org table at the cursor. |
| `:org-table-insert-row` | Insert a table row below the cursor's. |
| `:org-table-insert-separator` | Insert a table separator below the cursor's row. |
| `:org-table-delete-row` | Remove the table row at the cursor. |
| `:org-table-insert-column` | Insert a table column at the cursor's. |
| `:org-table-delete-column` | Remove the table column at the cursor. |
| `:org-table-next-cell` | Realign, then move to the next table cell. |
| `:org-table-previous-cell` | Realign, then move to the previous table cell. |
| `:org-archive-subtree`, `:org-archive` | Move the subtree at the cursor to the file's archive. |
| `:org-set-priority` | Set the priority on the headline at the cursor. |
| `:org-schedule` | Set SCHEDULED: on the entry at the cursor. |
| `:org-deadline` | Set DEADLINE: on the entry at the cursor. |
| `:org-set-property` | Set a property on the entry at the cursor. |
| `:org-remove-property` | Remove a property from the entry at the cursor. |
| `:org-set-effort` | Set the effort estimate on the entry at the cursor. |
| `:org-increment-effort`, `:org-inc-effort` | Step the effort estimate to the next value in the file's list. |
| `:org-insert-drawer` | Insert an empty drawer under the entry at the cursor. |
| `:org-add-note` | Record a dated note in the entry's :LOGBOOK:. |
| `:org-log-state` | Record a TODO state change in the entry's :LOGBOOK:. |
| `:roam-capture` | Create an Org-Roam node from a template. |
| `:roam-unlinked-references`, `:roam-unlinked` | List the places this node is named without being linked. |
| `:roam-rename-node`, `:roam-rename` | Rename the node at the cursor, and the link descriptions naming it. |
| `:roam-dailies-today`, `:roam-today` | Open today's daily note, creating it if needed. |
| `:roam-dailies-date` | Open the daily note for a date, creating it if needed. |
| `:roam-dailies-next` | Open the next daily note that exists. |
| `:roam-dailies-previous` | Open the previous daily note that exists. |
| `:roam-tag-add` | Add a tag to the node at the cursor. |
| `:roam-tag-remove` | Remove a tag from the node at the cursor. |
| `:roam-ref-add` | Add a ref to the node at the cursor. |
| `:roam-ref-remove` | Remove a ref from the node at the cursor. |
| `:roam-refile` | Refile the subtree at the cursor into another Org-Roam node. |
| `:roam-reindex` | Re-index the Org-Roam directory from scratch. |
| `:reload`, `:rl` | Discard changes and reload from the source file. |
| `:reload-all`, `:rla` | Discard changes and reload all documents from the source files. |
| `:update`, `:u` | Write changes only if the file has been modified. |
| `:lsp-workspace-command` | Open workspace command picker |
| `:lsp-restart` | Restarts the given language servers, or all language servers that are used by the current file if no arguments are supplied |
| `:lsp-stop` | Stops the given language servers, or all language servers that are used by the current file if no arguments are supplied |
| `:tree-sitter-scopes` | Display tree sitter scopes, primarily for theming and development. |
| `:tree-sitter-highlight-name` | Display name of tree-sitter highlight scope under the cursor. |
| `:tree-sitter-layers` | Display language names of tree-sitter injection layers under the cursor. |
| `:debug-start`, `:dbg` | Start a debug session from a given template with given parameters. |
| `:debug-remote`, `:dbg-tcp` | Connect to a debug adapter by TCP address and start a debugging session from a given template with given parameters. |
| `:debug-eval` | Evaluate expression in current debug context. |
| `:vsplit`, `:vs` | Open the file in a vertical split. |
| `:vsplit-new`, `:vnew` | Open a scratch buffer in a vertical split. |
| `:hsplit`, `:hs`, `:sp` | Open the file in a horizontal split. |
| `:hsplit-new`, `:hnew` | Open a scratch buffer in a horizontal split. |
| `:tutor` | Open the tutorial. |
| `:goto`, `:g` | Goto line number. |
| `:set-language`, `:lang` | Set the language of current buffer (show current language if no value specified). |
| `:set-option`, `:set` | Set a config option at runtime.<br>For example to disable smart case search, use `:set search.smart-case false`. |
| `:toggle-option`, `:toggle` | Toggle a config option at runtime.<br>For example to toggle smart case search, use `:toggle search.smart-case`. |
| `:get-option`, `:get` | Get the current value of a config option. |
| `:sort` | Sort ranges in selection. |
| `:reflow` | Hard-wrap the current selection of lines to a given width. |
| `:tree-sitter-subtree`, `:ts-subtree` | Display the smallest tree-sitter subtree that spans the primary selection, primarily for debugging queries. |
| `:config-reload` | Refresh user config. |
| `:config-open` | Open the user config.toml file. |
| `:config-open-workspace` | Open the workspace config.toml file. |
| `:log-open` | Open the helix log file. |
| `:insert-output` | Run shell command, inserting output before each selection. |
| `:append-output` | Run shell command, appending output after each selection. |
| `:pipe`, `:\|` | Pipe each selection to the shell command. |
| `:pipe-to` | Pipe each selection to the shell command, ignoring output. |
| `:run-shell-command`, `:sh`, `:!` | Run a shell command |
| `:reset-diff-change`, `:diffget`, `:diffg` | Reset the diff change at the cursor position. |
| `:clear-register` | Clear given register. If no argument is provided, clear all registers. |
| `:set-register` | Set contents of the given register. |
| `:redraw` | Clear and re-render the whole UI |
| `:move`, `:mv` | Move the current buffer and its corresponding file to a different path |
| `:move!`, `:mv!` | Move the current buffer and its corresponding file to a different path creating necessary subdirectories |
| `:yank-diagnostic` | Yank diagnostic(s) under primary cursor to register, or clipboard by default |
| `:read`, `:r` | Load a file into buffer |
| `:echo` | Prints the given arguments to the statusline. |
| `:noop` | Does nothing. |
| `:workspace-trust` | Allow language servers and local config for the current workspace. |
| `:workspace-untrust` | Revoke the current workspace's trust grant or exclusion. |
| `:workspace-exclude` | Mark the current workspace as never-prompt. Never prompts for trust again. |
