| Name | Description | Default keybinds |
| --- | --- | --- |
| `no_op` | Do nothing |  |
| `move_char_left` | Move left | normal: `` h ``, `` <left> ``, insert: `` <left> `` |
| `move_char_right` | Move right | normal: `` l ``, `` <right> ``, insert: `` <right> `` |
| `move_line_up` | Move up | normal: `` gk `` |
| `move_line_down` | Move down | normal: `` gj `` |
| `move_visual_line_up` | Move up | normal: `` k ``, `` <up> ``, insert: `` <up> `` |
| `move_visual_line_down` | Move down | normal: `` j ``, `` <down> ``, insert: `` <down> `` |
| `extend_char_left` | Extend left | select: `` h ``, `` <left> `` |
| `extend_char_right` | Extend right | select: `` l ``, `` <right> `` |
| `extend_line_up` | Extend up | select: `` gk `` |
| `extend_line_down` | Extend down | select: `` gj `` |
| `extend_visual_line_up` | Extend up | select: `` k ``, `` <up> `` |
| `extend_visual_line_down` | Extend down | select: `` j ``, `` <down> `` |
| `copy_selection_on_next_line` | Copy selection on next line | normal: `` C ``, select: `` C `` |
| `copy_selection_on_prev_line` | Copy selection on previous line | normal: `` <A-C> ``, select: `` <A-C> `` |
| `move_next_word_start` | Move to start of next word | normal: `` w `` |
| `move_prev_word_start` | Move to start of previous word | normal: `` b `` |
| `move_next_word_end` | Move to end of next word | normal: `` e `` |
| `move_prev_word_end` | Move to end of previous word |  |
| `move_next_long_word_start` | Move to start of next long word | normal: `` W `` |
| `move_prev_long_word_start` | Move to start of previous long word | normal: `` B `` |
| `move_next_long_word_end` | Move to end of next long word | normal: `` E `` |
| `move_prev_long_word_end` | Move to end of previous long word |  |
| `move_next_sub_word_start` | Move to start of next sub word |  |
| `move_prev_sub_word_start` | Move to start of previous sub word |  |
| `move_next_sub_word_end` | Move to end of next sub word |  |
| `move_prev_sub_word_end` | Move to end of previous sub word |  |
| `move_parent_node_end` | Move to end of the parent node | normal: `` <A-e> `` |
| `move_parent_node_start` | Move to beginning of the parent node | normal: `` <A-b> `` |
| `extend_next_word_start` | Extend to start of next word | select: `` w `` |
| `extend_prev_word_start` | Extend to start of previous word | select: `` b `` |
| `extend_next_word_end` | Extend to end of next word | select: `` e `` |
| `extend_prev_word_end` | Extend to end of previous word |  |
| `extend_next_long_word_start` | Extend to start of next long word | select: `` W `` |
| `extend_prev_long_word_start` | Extend to start of previous long word | select: `` B `` |
| `extend_next_long_word_end` | Extend to end of next long word | select: `` E `` |
| `extend_prev_long_word_end` | Extend to end of prev long word |  |
| `extend_next_sub_word_start` | Extend to start of next sub word |  |
| `extend_prev_sub_word_start` | Extend to start of previous sub word |  |
| `extend_next_sub_word_end` | Extend to end of next sub word |  |
| `extend_prev_sub_word_end` | Extend to end of prev sub word |  |
| `extend_parent_node_end` | Extend to end of the parent node | select: `` <A-e> `` |
| `extend_parent_node_start` | Extend to beginning of the parent node | select: `` <A-b> `` |
| `find_till_char` | Move till next occurrence of char | normal: `` t `` |
| `find_next_char` | Move to next occurrence of char | normal: `` f `` |
| `extend_till_char` | Extend till next occurrence of char | select: `` t `` |
| `extend_next_char` | Extend to next occurrence of char | select: `` f `` |
| `till_prev_char` | Move till previous occurrence of char | normal: `` T `` |
| `find_prev_char` | Move to previous occurrence of char | normal: `` F `` |
| `extend_till_prev_char` | Extend till previous occurrence of char | select: `` T `` |
| `extend_prev_char` | Extend to previous occurrence of char | select: `` F `` |
| `repeat_last_motion` | Repeat last motion | normal: `` <A-.> ``, select: `` <A-.> `` |
| `replace` | Replace with new char | normal: `` r ``, select: `` r `` |
| `switch_case` | Switch (toggle) case | normal: `` ~ ``, select: `` ~ `` |
| `switch_to_uppercase` | Switch to uppercase | normal: `` <A-`> ``, select: `` <A-`> `` |
| `switch_to_lowercase` | Switch to lowercase | normal: `` ` ``, select: `` ` `` |
| `page_up` | Move page up | normal: `` <C-b> ``, `` Z<C-b> ``, `` z<C-b> ``, `` <pageup> ``, `` Z<pageup> ``, `` z<pageup> ``, select: `` <C-b> ``, `` Z<C-b> ``, `` z<C-b> ``, `` <pageup> ``, `` Z<pageup> ``, `` z<pageup> ``, insert: `` <pageup> `` |
| `page_down` | Move page down | normal: `` <C-f> ``, `` Z<C-f> ``, `` z<C-f> ``, `` <pagedown> ``, `` Z<pagedown> ``, `` z<pagedown> ``, select: `` <C-f> ``, `` Z<C-f> ``, `` z<C-f> ``, `` <pagedown> ``, `` Z<pagedown> ``, `` z<pagedown> ``, insert: `` <pagedown> `` |
| `half_page_up` | Move half page up |  |
| `half_page_down` | Move half page down |  |
| `page_cursor_up` | Move page and cursor up |  |
| `page_cursor_down` | Move page and cursor down |  |
| `page_cursor_half_up` | Move page and cursor half up | normal: `` <C-u> ``, `` Z<C-u> ``, `` z<C-u> ``, `` Z<backspace> ``, `` z<backspace> ``, select: `` <C-u> ``, `` Z<C-u> ``, `` z<C-u> ``, `` Z<backspace> ``, `` z<backspace> `` |
| `page_cursor_half_down` | Move page and cursor half down | normal: `` <C-d> ``, `` Z<C-d> ``, `` z<C-d> ``, `` Z<space> ``, `` z<space> ``, select: `` <C-d> ``, `` Z<C-d> ``, `` z<C-d> ``, `` Z<space> ``, `` z<space> `` |
| `select_all` | Select whole document | normal: `` % ``, select: `` % `` |
| `select_regex` | Select all regex matches inside selections | normal: `` s ``, select: `` s `` |
| `split_selection` | Split selections on regex matches | normal: `` S ``, select: `` S `` |
| `split_selection_on_newline` | Split selection on newlines | normal: `` <A-s> ``, select: `` <A-s> `` |
| `merge_selections` | Merge selections | normal: `` <A-minus> ``, select: `` <A-minus> `` |
| `merge_consecutive_selections` | Merge consecutive selections | normal: `` <A-_> ``, select: `` <A-_> `` |
| `search` | Search for regex pattern | normal: `` / ``, `` Z/ ``, `` z/ ``, select: `` / ``, `` Z/ ``, `` z/ `` |
| `rsearch` | Reverse search for regex pattern | normal: `` ? ``, `` Z? ``, `` z? ``, select: `` ? ``, `` Z? ``, `` z? `` |
| `search_next` | Select next search match | normal: `` n ``, `` Zn ``, `` zn ``, select: `` Zn ``, `` zn `` |
| `search_prev` | Select previous search match | normal: `` N ``, `` ZN ``, `` zN ``, select: `` ZN ``, `` zN `` |
| `extend_search_next` | Add next search match to selection | select: `` n `` |
| `extend_search_prev` | Add previous search match to selection | select: `` N `` |
| `search_selection` | Use current selection as search pattern | normal: `` <A-*> ``, select: `` <A-*> `` |
| `search_selection_detect_word_boundaries` | Use current selection as the search pattern, automatically wrapping with `\b` on word boundaries | normal: `` * ``, select: `` * `` |
| `make_search_word_bounded` | Modify current search to make it word bounded |  |
| `global_search` | Global search in workspace folder | normal: `` <space>/ ``, select: `` <space>/ `` |
| `extend_line` | Select current line, if already selected, extend to another line based on the anchor |  |
| `extend_line_below` | Select current line, if already selected, extend to next line | normal: `` x ``, select: `` x `` |
| `extend_line_above` | Select current line, if already selected, extend to previous line |  |
| `select_line_above` | Select current line, if already selected, extend or shrink line above based on the anchor |  |
| `select_line_below` | Select current line, if already selected, extend or shrink line below based on the anchor |  |
| `extend_to_line_bounds` | Extend selection to line bounds | normal: `` X ``, select: `` X `` |
| `shrink_to_line_bounds` | Shrink selection to line bounds | normal: `` <A-x> ``, select: `` <A-x> `` |
| `delete_selection` | Delete selection | normal: `` d ``, select: `` d `` |
| `delete_selection_noyank` | Delete selection without yanking | normal: `` <A-d> ``, select: `` <A-d> `` |
| `change_selection` | Change selection | normal: `` c ``, select: `` c `` |
| `change_selection_noyank` | Change selection without yanking | normal: `` <A-c> ``, select: `` <A-c> `` |
| `collapse_selection` | Collapse selection into single cursor | normal: `` ; ``, select: `` ; `` |
| `flip_selections` | Flip selection cursor and anchor | normal: `` <A-;> ``, select: `` <A-;> `` |
| `ensure_selections_forward` | Ensure all selections face forward | normal: `` <A-:> ``, select: `` <A-:> `` |
| `insert_mode` | Insert before selection | normal: `` i ``, select: `` i `` |
| `append_mode` | Append after selection | normal: `` a ``, select: `` a `` |
| `command_mode` | Enter command mode | normal: `` : ``, select: `` : `` |
| `file_picker` | Open file picker | normal: `` <space>f ``, select: `` <space>f `` |
| `file_picker_in_current_buffer_directory` | Open file picker at current buffer's directory |  |
| `file_picker_in_current_directory` | Open file picker at current working directory | normal: `` <space>F ``, select: `` <space>F `` |
| `file_explorer` | Open file explorer in workspace root | normal: `` <space>e ``, select: `` <space>e `` |
| `file_explorer_in_current_buffer_directory` | Open file explorer at current buffer's directory | normal: `` <space>. ``, select: `` <space>. `` |
| `file_explorer_in_current_directory` | Open file explorer at current working directory |  |
| `code_action` | Perform code action | normal: `` <space>a ``, select: `` <space>a `` |
| `buffer_picker` | Open buffer picker | normal: `` <space>b ``, select: `` <space>b `` |
| `jumplist_picker` | Open jumplist picker | normal: `` <space>j ``, select: `` <space>j `` |
| `roam_node_find` | Find Org-Roam node | normal: `` <space>nf ``, select: `` <space>nf `` |
| `roam_backlinks_toggle` | Toggle Org-Roam backlinks panel | normal: `` <space>nb ``, select: `` <space>nb `` |
| `roam_promote_buffer` | Promote the buffer to a single Org-Roam node |  |
| `roam_demote_buffer` | Demote the Org-Roam file node to a heading |  |
| `roam_extract_subtree` | Extract the subtree at the cursor into its own node |  |
| `roam_replace_links` | Replace legacy roam: links with id: links |  |
| `roam_refile` | Refile the subtree at the cursor into another Org-Roam node |  |
| `org_follow_link` | Follow the Org link under the cursor |  |
| `org_goto_next_link` | Move to the next Org link |  |
| `org_goto_previous_link` | Move to the previous Org link |  |
| `org_store_link` | Store a link to the cursor's location |  |
| `org_insert_link` | Insert the stored Org link |  |
| `org_create_id` | Give the entry at the cursor an :ID: |  |
| `roam_node_insert` | Insert a link to an Org-Roam node, creating it if needed |  |
| `roam_random_node` | Open a random Org-Roam node |  |
| `roam_ref_find` | Find an Org-Roam node by one of its refs |  |
| `roam_alias_add` | Add an alias to the node at the cursor |  |
| `roam_alias_remove` | Remove an alias from the node at the cursor |  |
| `roam_rename_node` | Rename the node at the cursor and the links naming it |  |
| `roam_unlinked_references` | List where this node is named without a link |  |
| `roam_capture` | Create an Org-Roam node from a template |  |
| `org_insert_heading` | Insert a heading after the current subtree |  |
| `org_promote` | Promote the headline at the cursor |  |
| `org_demote` | Demote the headline at the cursor |  |
| `org_promote_subtree` | Promote the subtree at the cursor |  |
| `org_demote_subtree` | Demote the subtree at the cursor |  |
| `org_move_subtree_up` | Move the subtree above its sibling |  |
| `org_move_subtree_down` | Move the subtree below its sibling |  |
| `org_todo` | Cycle the TODO state forward |  |
| `org_todo_previous` | Cycle the TODO state backward |  |
| `org_priority_up` | Raise the priority towards [#A] |  |
| `org_priority_down` | Lower the priority |  |
| `org_agenda_day` | Show today's agenda |  |
| `org_agenda_restrict` | Restrict the agenda to the current file |  |
| `org_agenda_unrestrict` | Lift the agenda restriction |  |
| `org_agenda_scope` | Say which files the agenda reads |  |
| `org_agenda_week` | Show the week's agenda |  |
| `org_todo_list` | List every unfinished task |  |
| `org_todo_filtered` | List unfinished tasks matching a keyword, tag or priority |  |
| `org_insert_item` | Insert a list item after the one at the cursor |  |
| `org_renumber_list` | Renumber the ordered list at the cursor |  |
| `org_demote_item` | Move the list item in a level |  |
| `org_promote_item` | Move the list item out a level |  |
| `org_toggle_checkbox` | Tick or untick the checkbox at the cursor |  |
| `org_update_cookies` | Bring every statistics cookie up to date |  |
| `org_table_align` | Realign the Org table at the cursor |  |
| `org_table_insert_row` | Insert a table row below the cursor |  |
| `org_table_insert_separator` | Insert a table separator below the cursor |  |
| `org_table_delete_row` | Remove the table row at the cursor |  |
| `org_table_insert_column` | Insert a table column at the cursor |  |
| `org_table_delete_column` | Remove the table column at the cursor |  |
| `org_table_next_cell` | Move to the next table cell |  |
| `org_table_previous_cell` | Move to the previous table cell |  |
| `fold` | Fold the innermost foldable range at the cursor | normal: `` Zf ``, `` zf ``, select: `` Zf ``, `` zf `` |
| `narrow_to_selection` | Hide every line outside the selection |  |
| `cycle_fold` | Step the range at the cursor through folded, children, open | normal: `` Z<tab> ``, `` z<tab> ``, select: `` Z<tab> ``, `` z<tab> `` |
| `cycle_fold_all` | Step the buffer through overview, contents, everything | normal: `` Z<S-tab> ``, `` z<S-tab> ``, select: `` Z<S-tab> ``, `` z<S-tab> `` |
| `unfold` | Open the fold at the cursor | normal: `` Zo ``, `` zo ``, select: `` Zo ``, `` zo `` |
| `toggle_fold` | Close the fold at the cursor, or open it | normal: `` Za ``, `` za ``, select: `` Za ``, `` za `` |
| `fold_all` | Fold everything the language marks as foldable | normal: `` ZM ``, `` zM ``, select: `` ZM ``, `` zM `` |
| `unfold_all` | Open every fold in the buffer | normal: `` ZR ``, `` zR ``, select: `` ZR ``, `` zR `` |
| `roam_index` | Look through everything the index holds |  |
| `roam_state` | Report the fork's Org-Roam state for a bug report |  |
| `roam_backlink_counts` | Show each headline's backlink count beside it |  |
| `roam_pin_node` | Pin the Roam panel to the node at the cursor |  |
| `roam_unpin_node` | Let the Roam panel follow the cursor again |  |
| `roam_diagnose` | Report what the index believes about the node at the cursor |  |
| `org_emphasis` | Toggle an emphasis marker on the selection |  |
| `org_insert_block` | Insert a structure block, wrapping the selection |  |
| `org_footnote_new` | Add a footnote and go to where its text goes |  |
| `org_footnote_goto` | Jump between a footnote's reference and definition |  |
| `org_footnote_renumber` | Renumber the numeric footnotes in reference order |  |
| `org_cite_insert` | Insert a citation, completing over the bibliography |  |
| `org_cite_follow` | Open the bibliography at the cited entry |  |
| `org_next_heading` | Move to the next heading |  |
| `org_previous_heading` | Move to the previous heading |  |
| `org_next_sibling_heading` | Move to the next heading at the same level |  |
| `org_previous_sibling_heading` | Move to the previous heading at the same level |  |
| `org_parent_heading` | Move to the parent heading |  |
| `org_goto_heading` | Jump to a heading in this buffer by name |  |
| `org_outline_path` | Show the outline path of the entry at the cursor |  |
| `org_sparse_tree` | Hide everything but the entries matching a filter |  |
| `org_narrow` | Hide everything outside the subtree at the cursor |  |
| `org_widen` | Bring back everything a narrowing or sparse tree hid |  |
| `org_startup_visibility` | Fold the buffer the way its #+STARTUP: says it opens |  |
| `org_src_next` | Move to the next source block |  |
| `org_src_previous` | Move to the previous source block |  |
| `org_src_result` | Jump between a source block and its #+RESULTS: |  |
| `org_edit_src` | Edit the source block at the cursor in a buffer of its own |  |
| `org_tangle` | Write every block with a :tangle target to its file |  |
| `org_clock_in` | Start a clock on the entry at the cursor, stopping any other |  |
| `org_clock_out` | Stop the running clock |  |
| `org_clock_cancel` | Discard the running clock |  |
| `org_clock_goto` | Jump to the entry with the running clock |  |
| `org_clock_report` | Insert or refresh a clock report table |  |
| `org_export_markdown` | Export the buffer to Markdown next to its file |  |
| `org_export_html` | Export the buffer to HTML next to its file |  |
| `org_export_latex` | Export the buffer to LaTeX next to its file |  |
| `org_babel_execute` | Run the source block at the cursor and write its results (needs workspace trust) |  |
| `org_columns` | Show or hide the column view of the buffer, from its #+COLUMNS: |  |
| `org_attach` | Copy a file into the attachment directory of the entry |  |
| `org_attach_open` | Pick one of the files attached to the entry |  |
| `org_inline_task` | Insert an inline task below the cursor |  |
| `org_copy_visible` | Yank only the visible text of the selections, or of the buffer |  |
| `roam_dailies_capture` | Add an entry to today's daily note without leaving this buffer |  |
| `roam_dailies_directory` | Pick a file in the dailies directory |  |
| `org_encrypt_entry` | Encrypt the body of the entry at the cursor with gpg |  |
| `org_encrypt_entries` | Encrypt every :crypt: entry of the buffer that is in clear |  |
| `org_decrypt_entry` | Decrypt the entry at the cursor |  |
| `roam_graph` | Draw the whole Org-Roam graph with Graphviz |  |
| `roam_graph_neighbourhood` | Draw the nodes within two links of the node at the cursor |  |
| `org_copy_subtree` | Copy the subtree at the cursor |  |
| `org_cut_subtree` | Cut the subtree at the cursor |  |
| `org_paste_subtree` | Paste the copied subtree at the cursor's level |  |
| `org_clone_subtree` | Clone the subtree at the cursor, shifting its dates |  |
| `org_sort_entries` | Sort the children of the entry at the cursor |  |
| `org_sort_list` | Sort the list items at the cursor |  |
| `org_sort_table` | Sort the table rows by the cursor's column |  |
| `org_dblock_update` | Regenerate the dynamic block at the cursor |  |
| `org_dblock_update_all` | Regenerate every dynamic block in the buffer |  |
| `org_archive_subtree` | Move the subtree at the cursor to the file's archive |  |
| `org_set_priority` | Set the priority on the headline at the cursor |  |
| `org_schedule` | Set SCHEDULED: on the entry at the cursor |  |
| `org_deadline` | Set DEADLINE: on the entry at the cursor |  |
| `org_set_property` | Set a property on the entry at the cursor |  |
| `org_remove_property` | Remove a property from the entry at the cursor |  |
| `org_set_effort` | Set the effort estimate on the entry at the cursor |  |
| `org_increment_effort` | Step the effort estimate to the next value |  |
| `org_insert_drawer` | Insert a drawer under the entry at the cursor |  |
| `org_add_note` | Record a dated note in the entry's logbook |  |
| `org_log_state` | Record a TODO state change in the entry's logbook |  |
| `roam_dailies_today` | Open today's daily note |  |
| `roam_dailies_date` | Open the daily note for a date |  |
| `roam_dailies_next` | Open the next daily note |  |
| `roam_dailies_previous` | Open the previous daily note |  |
| `roam_tag_add` | Add a tag to the node at the cursor |  |
| `roam_tag_remove` | Remove a tag from the node at the cursor |  |
| `roam_ref_add` | Add a ref to the node at the cursor |  |
| `roam_ref_remove` | Remove a ref from the node at the cursor |  |
| `magit` | Open the Magit transient menu | normal: `` <space>m ``, select: `` <space>m `` |
| `magit_file` | Open the Magit menu for the current file | normal: `` <space>M ``, select: `` <space>M `` |
| `terminal` | Open the integrated terminal | normal: `` <space>t ``, select: `` <space>t `` |
| `terminal_list` | List the integrated terminals | normal: `` <space>T ``, select: `` <space>T `` |
| `focus_dock` | Move the keys between the documents and the docked panes | normal: `` <C-w>p ``, `` <space>wp ``, `` <C-w><C-p> ``, `` <space>w<C-p> ``, select: `` <C-w>p ``, `` <space>wp ``, `` <C-w><C-p> ``, `` <space>w<C-p> `` |
| `symbol_picker` | Open symbol picker |  |
| `syntax_symbol_picker` | Open symbol picker from syntax information |  |
| `lsp_or_syntax_symbol_picker` | Open symbol picker from LSP or syntax information | normal: `` <space>s ``, select: `` <space>s `` |
| `changed_file_picker` | Open changed file picker | normal: `` <space>g ``, select: `` <space>g `` |
| `select_references_to_symbol_under_cursor` | Select symbol references | normal: `` <space>h ``, select: `` <space>h `` |
| `workspace_symbol_picker` | Open workspace symbol picker |  |
| `syntax_workspace_symbol_picker` | Open workspace symbol picker from syntax information |  |
| `lsp_or_syntax_workspace_symbol_picker` | Open workspace symbol picker from LSP or syntax information | normal: `` <space>S ``, select: `` <space>S `` |
| `diagnostics_picker` | Open diagnostic picker | normal: `` <space>d ``, select: `` <space>d `` |
| `workspace_diagnostics_picker` | Open workspace diagnostic picker | normal: `` <space>D ``, select: `` <space>D `` |
| `last_picker` | Open last picker | normal: `` <space>' ``, select: `` <space>' `` |
| `insert_at_line_start` | Insert at start of line | normal: `` I ``, select: `` I `` |
| `insert_at_line_end` | Insert at end of line | normal: `` A ``, select: `` A `` |
| `open_below` | Open new line below selection | normal: `` o ``, select: `` o `` |
| `open_above` | Open new line above selection | normal: `` O ``, select: `` O `` |
| `normal_mode` | Enter normal mode | normal: `` <esc> ``, select: `` v ``, insert: `` <esc> `` |
| `select_mode` | Enter selection extend mode | normal: `` v `` |
| `exit_select_mode` | Exit selection mode | select: `` <esc> `` |
| `goto_definition` | Goto definition | normal: `` gd ``, select: `` gd `` |
| `goto_declaration` | Goto declaration | normal: `` gD ``, select: `` gD `` |
| `add_newline_above` | Add newline above | normal: `` [<space> ``, select: `` [<space> `` |
| `add_newline_below` | Add newline below | normal: `` ]<space> ``, select: `` ]<space> `` |
| `goto_type_definition` | Goto type definition | normal: `` gy ``, select: `` gy `` |
| `goto_implementation` | Goto implementation | normal: `` gi ``, select: `` gi `` |
| `goto_file_start` | Goto line number `<n>` else file start | normal: `` gg `` |
| `goto_file_end` | Goto file end |  |
| `extend_to_file_start` | Extend to line number `<n>` else file start | select: `` gg `` |
| `extend_to_file_end` | Extend to file end |  |
| `goto_file` | Goto files/URLs in selections | normal: `` gf ``, select: `` gf `` |
| `goto_file_hsplit` | Goto files in selections (hsplit) | normal: `` <C-w>f ``, `` <space>wf ``, select: `` <C-w>f ``, `` <space>wf `` |
| `goto_file_vsplit` | Goto files in selections (vsplit) | normal: `` <C-w>F ``, `` <space>wF ``, select: `` <C-w>F ``, `` <space>wF `` |
| `goto_reference` | Goto references | normal: `` gr ``, select: `` gr `` |
| `goto_window_top` | Goto window top | normal: `` gt ``, select: `` gt `` |
| `goto_window_center` | Goto window center | normal: `` gc ``, select: `` gc `` |
| `goto_window_bottom` | Goto window bottom | normal: `` gb ``, select: `` gb `` |
| `goto_last_accessed_file` | Goto last accessed file | normal: `` ga ``, select: `` ga `` |
| `goto_last_modified_file` | Goto last modified file | normal: `` gm ``, select: `` gm `` |
| `goto_last_modification` | Goto last modification | normal: `` g. ``, select: `` g. `` |
| `goto_line` | Goto line | normal: `` G ``, select: `` G `` |
| `goto_last_line` | Goto last line | normal: `` ge `` |
| `extend_to_last_line` | Extend to last line | select: `` ge `` |
| `goto_first_diag` | Goto first diagnostic | normal: `` [D ``, select: `` [D `` |
| `goto_last_diag` | Goto last diagnostic | normal: `` ]D ``, select: `` ]D `` |
| `goto_next_diag` | Goto next diagnostic | normal: `` ]d ``, select: `` ]d `` |
| `goto_prev_diag` | Goto previous diagnostic | normal: `` [d ``, select: `` [d `` |
| `goto_next_change` | Goto next change | normal: `` ]g ``, select: `` ]g `` |
| `goto_prev_change` | Goto previous change | normal: `` [g ``, select: `` [g `` |
| `goto_first_change` | Goto first change | normal: `` [G ``, select: `` [G `` |
| `goto_last_change` | Goto last change | normal: `` ]G ``, select: `` ]G `` |
| `goto_line_start` | Goto line start | normal: `` gh ``, `` <home> ``, select: `` gh ``, insert: `` <home> `` |
| `goto_line_end` | Goto line end | normal: `` gl ``, `` <end> ``, select: `` gl `` |
| `goto_column` | Goto column | normal: `` g\| `` |
| `extend_to_column` | Extend to column | select: `` g\| `` |
| `goto_next_buffer` | Goto next buffer | normal: `` gn ``, select: `` gn `` |
| `goto_previous_buffer` | Goto previous buffer | normal: `` gp ``, select: `` gp `` |
| `goto_line_end_newline` | Goto newline at line end | insert: `` <end> `` |
| `goto_first_nonwhitespace` | Goto first non-blank in line | normal: `` gs ``, select: `` gs `` |
| `trim_selections` | Trim whitespace from selections | normal: `` _ ``, select: `` _ `` |
| `extend_to_line_start` | Extend to line start | select: `` <home> `` |
| `extend_to_first_nonwhitespace` | Extend to first non-blank in line |  |
| `extend_to_line_end` | Extend to line end | select: `` <end> `` |
| `extend_to_line_end_newline` | Extend to line end |  |
| `signature_help` | Show signature help |  |
| `smart_tab` | Insert tab if all cursors have all whitespace to their left; otherwise, run a separate command. | insert: `` <tab> `` |
| `insert_tab` | Insert tab char | insert: `` <S-tab> `` |
| `insert_newline` | Insert newline char | insert: `` <C-j> ``, `` <ret> `` |
| `insert_char_interactive` | Insert an interactively-chosen char |  |
| `append_char_interactive` | Append an interactively-chosen char |  |
| `delete_char_backward` | Delete previous char | insert: `` <C-h> ``, `` <backspace> ``, `` <S-backspace> `` |
| `delete_char_forward` | Delete next char | insert: `` <C-d> ``, `` <del> `` |
| `delete_word_backward` | Delete previous word | insert: `` <C-w> ``, `` <A-backspace> `` |
| `delete_word_forward` | Delete next word | insert: `` <A-d> ``, `` <A-del> `` |
| `kill_to_line_start` | Delete till start of line | insert: `` <C-u> `` |
| `kill_to_line_end` | Delete till end of line | insert: `` <C-k> `` |
| `undo` | Undo change | normal: `` u ``, select: `` u `` |
| `redo` | Redo change | normal: `` U ``, select: `` U `` |
| `earlier` | Move backward in history | normal: `` <A-u> ``, select: `` <A-u> `` |
| `later` | Move forward in history | normal: `` <A-U> ``, select: `` <A-U> `` |
| `commit_undo_checkpoint` | Commit changes to new checkpoint | insert: `` <C-s> `` |
| `yank` | Yank selection | normal: `` y ``, select: `` y `` |
| `yank_to_clipboard` | Yank selections to clipboard | normal: `` <space>y ``, select: `` <space>y `` |
| `yank_to_primary_clipboard` | Yank selections to primary clipboard |  |
| `yank_joined` | Join and yank selections |  |
| `yank_joined_to_clipboard` | Join and yank selections to clipboard |  |
| `yank_main_selection_to_clipboard` | Yank main selection to clipboard | normal: `` <space>Y ``, select: `` <space>Y `` |
| `yank_joined_to_primary_clipboard` | Join and yank selections to primary clipboard |  |
| `yank_main_selection_to_primary_clipboard` | Yank main selection to primary clipboard |  |
| `replace_with_yanked` | Replace with yanked text | normal: `` R ``, select: `` R `` |
| `replace_selections_with_clipboard` | Replace selections by clipboard content | normal: `` <space>R ``, select: `` <space>R `` |
| `replace_selections_with_primary_clipboard` | Replace selections by primary clipboard |  |
| `paste_after` | Paste after selection | normal: `` p ``, select: `` p `` |
| `paste_before` | Paste before selection | normal: `` P ``, select: `` P `` |
| `paste_clipboard_after` | Paste clipboard after selections | normal: `` <space>p ``, select: `` <space>p `` |
| `paste_clipboard_before` | Paste clipboard before selections | normal: `` <space>P ``, select: `` <space>P `` |
| `paste_primary_clipboard_after` | Paste primary clipboard after selections |  |
| `paste_primary_clipboard_before` | Paste primary clipboard before selections |  |
| `indent` | Indent selection | normal: `` <gt> ``, select: `` <gt> `` |
| `unindent` | Unindent selection | normal: `` <lt> ``, select: `` <lt> `` |
| `format_selections` | Format selection | normal: `` = ``, select: `` = `` |
| `join_selections` | Join lines inside selection | normal: `` J ``, select: `` J `` |
| `join_selections_space` | Join lines inside selection and select spaces | normal: `` <A-J> ``, select: `` <A-J> `` |
| `keep_selections` | Keep selections matching regex | normal: `` K ``, select: `` K `` |
| `remove_selections` | Remove selections matching regex | normal: `` <A-K> ``, select: `` <A-K> `` |
| `align_selections` | Align selections in column | normal: `` & ``, select: `` & `` |
| `keep_primary_selection` | Keep primary selection | normal: `` , ``, select: `` , `` |
| `remove_primary_selection` | Remove primary selection | normal: `` <A-,> ``, select: `` <A-,> `` |
| `completion` | Invoke completion popup | insert: `` <C-x> `` |
| `hover` | Show docs for item under cursor | normal: `` <space>k ``, select: `` <space>k `` |
| `toggle_comments` | Comment/uncomment selections | normal: `` <C-c> ``, `` <space>c ``, select: `` <C-c> ``, `` <space>c `` |
| `toggle_line_comments` | Line comment/uncomment selections | normal: `` <space><A-c> ``, select: `` <space><A-c> `` |
| `toggle_block_comments` | Block comment/uncomment selections | normal: `` <space>C ``, select: `` <space>C `` |
| `rotate_selections_forward` | Rotate selections forward | normal: `` ) ``, select: `` ) `` |
| `rotate_selections_backward` | Rotate selections backward | normal: `` ( ``, select: `` ( `` |
| `rotate_selection_contents_forward` | Rotate selection contents forward | normal: `` <A-)> ``, select: `` <A-)> `` |
| `rotate_selection_contents_backward` | Rotate selections contents backward | normal: `` <A-(> ``, select: `` <A-(> `` |
| `reverse_selection_contents` | Reverse selections contents |  |
| `expand_selection` | Expand selection to parent syntax node | normal: `` <A-o> ``, `` <A-up> ``, select: `` <A-o> ``, `` <A-up> `` |
| `shrink_selection` | Shrink selection to previously expanded syntax node | normal: `` <A-i> ``, `` <A-down> ``, select: `` <A-i> ``, `` <A-down> `` |
| `select_next_sibling` | Select next sibling in the syntax tree | normal: `` <A-n> ``, `` <A-right> ``, select: `` <A-n> ``, `` <A-right> `` |
| `select_prev_sibling` | Select previous sibling the in syntax tree | normal: `` <A-p> ``, `` <A-left> ``, select: `` <A-p> ``, `` <A-left> `` |
| `select_all_siblings` | Select all siblings of the current node | normal: `` <A-a> ``, select: `` <A-a> `` |
| `select_all_children` | Select all children of the current node | normal: `` <A-I> ``, `` <S-A-down> ``, select: `` <A-I> ``, `` <S-A-down> `` |
| `jump_forward` | Jump forward on jumplist | normal: `` <C-i> ``, `` <tab> ``, select: `` <C-i> ``, `` <tab> `` |
| `jump_backward` | Jump backward on jumplist | normal: `` <C-o> ``, select: `` <C-o> `` |
| `save_selection` | Save current selection to jumplist | normal: `` <C-s> ``, select: `` <C-s> `` |
| `jump_view_right` | Jump to right split | normal: `` <C-w>l ``, `` <space>wl ``, `` <C-w><C-l> ``, `` <C-w><right> ``, `` <space>w<C-l> ``, `` <space>w<right> ``, select: `` <C-w>l ``, `` <space>wl ``, `` <C-w><C-l> ``, `` <C-w><right> ``, `` <space>w<C-l> ``, `` <space>w<right> `` |
| `jump_view_left` | Jump to left split | normal: `` <C-w>h ``, `` <space>wh ``, `` <C-w><C-h> ``, `` <C-w><left> ``, `` <space>w<C-h> ``, `` <space>w<left> ``, select: `` <C-w>h ``, `` <space>wh ``, `` <C-w><C-h> ``, `` <C-w><left> ``, `` <space>w<C-h> ``, `` <space>w<left> `` |
| `jump_view_up` | Jump to split above | normal: `` <C-w>k ``, `` <C-w><up> ``, `` <space>wk ``, `` <C-w><C-k> ``, `` <space>w<up> ``, `` <space>w<C-k> ``, select: `` <C-w>k ``, `` <C-w><up> ``, `` <space>wk ``, `` <C-w><C-k> ``, `` <space>w<up> ``, `` <space>w<C-k> `` |
| `jump_view_down` | Jump to split below | normal: `` <C-w>j ``, `` <space>wj ``, `` <C-w><C-j> ``, `` <C-w><down> ``, `` <space>w<C-j> ``, `` <space>w<down> ``, select: `` <C-w>j ``, `` <space>wj ``, `` <C-w><C-j> ``, `` <C-w><down> ``, `` <space>w<C-j> ``, `` <space>w<down> `` |
| `swap_view_right` | Swap with right split | normal: `` <C-w>L ``, `` <space>wL ``, select: `` <C-w>L ``, `` <space>wL `` |
| `swap_view_left` | Swap with left split | normal: `` <C-w>H ``, `` <space>wH ``, select: `` <C-w>H ``, `` <space>wH `` |
| `swap_view_up` | Swap with split above | normal: `` <C-w>K ``, `` <space>wK ``, select: `` <C-w>K ``, `` <space>wK `` |
| `swap_view_down` | Swap with split below | normal: `` <C-w>J ``, `` <space>wJ ``, select: `` <C-w>J ``, `` <space>wJ `` |
| `transpose_view` | Transpose splits | normal: `` <C-w>t ``, `` <space>wt ``, `` <C-w><C-t> ``, `` <space>w<C-t> ``, select: `` <C-w>t ``, `` <space>wt ``, `` <C-w><C-t> ``, `` <space>w<C-t> `` |
| `rotate_view` | Goto next window | normal: `` <C-w>w ``, `` <space>ww ``, `` <C-w><C-w> ``, `` <space>w<C-w> ``, select: `` <C-w>w ``, `` <space>ww ``, `` <C-w><C-w> ``, `` <space>w<C-w> `` |
| `rotate_view_reverse` | Goto previous window |  |
| `hsplit` | Horizontal bottom split | normal: `` <C-w>s ``, `` <space>ws ``, `` <C-w><C-s> ``, `` <space>w<C-s> ``, select: `` <C-w>s ``, `` <space>ws ``, `` <C-w><C-s> ``, `` <space>w<C-s> `` |
| `hsplit_new` | Horizontal bottom split scratch buffer | normal: `` <C-w>ns ``, `` <space>wns ``, `` <C-w>n<C-s> ``, `` <space>wn<C-s> ``, select: `` <C-w>ns ``, `` <space>wns ``, `` <C-w>n<C-s> ``, `` <space>wn<C-s> `` |
| `vsplit` | Vertical right split | normal: `` <C-w>v ``, `` <space>wv ``, `` <C-w><C-v> ``, `` <space>w<C-v> ``, select: `` <C-w>v ``, `` <space>wv ``, `` <C-w><C-v> ``, `` <space>w<C-v> `` |
| `vsplit_new` | Vertical right split scratch buffer | normal: `` <C-w>nv ``, `` <space>wnv ``, `` <C-w>n<C-v> ``, `` <space>wn<C-v> ``, select: `` <C-w>nv ``, `` <space>wnv ``, `` <C-w>n<C-v> ``, `` <space>wn<C-v> `` |
| `wclose` | Close window | normal: `` <C-w>q ``, `` <space>wq ``, `` <C-w><C-q> ``, `` <space>w<C-q> ``, select: `` <C-w>q ``, `` <space>wq ``, `` <C-w><C-q> ``, `` <space>w<C-q> `` |
| `wonly` | Close windows except current | normal: `` <C-w>o ``, `` <space>wo ``, `` <C-w><C-o> ``, `` <space>w<C-o> ``, select: `` <C-w>o ``, `` <space>wo ``, `` <C-w><C-o> ``, `` <space>w<C-o> `` |
| `select_register` | Select register | normal: `` " ``, select: `` " `` |
| `insert_register` | Insert register | insert: `` <C-r> `` |
| `copy_between_registers` | Copy between two registers |  |
| `align_view_middle` | Align view middle | normal: `` Zm ``, `` zm ``, select: `` Zm ``, `` zm `` |
| `align_view_top` | Align view top | normal: `` Zt ``, `` zt ``, select: `` Zt ``, `` zt `` |
| `align_view_center` | Align view center | normal: `` Zc ``, `` Zz ``, `` zc ``, `` zz ``, select: `` Zc ``, `` Zz ``, `` zc ``, `` zz `` |
| `align_view_bottom` | Align view bottom | normal: `` Zb ``, `` zb ``, select: `` Zb ``, `` zb `` |
| `scroll_up` | Scroll view up | normal: `` Zk ``, `` zk ``, `` Z<up> ``, `` z<up> ``, select: `` Zk ``, `` zk ``, `` Z<up> ``, `` z<up> `` |
| `scroll_down` | Scroll view down | normal: `` Zj ``, `` zj ``, `` Z<down> ``, `` z<down> ``, select: `` Zj ``, `` zj ``, `` Z<down> ``, `` z<down> `` |
| `match_brackets` | Goto matching bracket | normal: `` mm ``, select: `` mm `` |
| `surround_add` | Surround add | normal: `` ms ``, select: `` ms `` |
| `surround_replace` | Surround replace | normal: `` mr ``, select: `` mr `` |
| `surround_delete` | Surround delete | normal: `` md ``, select: `` md `` |
| `select_textobject_around` | Select around object | normal: `` ma ``, select: `` ma `` |
| `select_textobject_inner` | Select inside object | normal: `` mi ``, select: `` mi `` |
| `goto_next_function` | Goto next function | normal: `` ]f ``, select: `` ]f `` |
| `goto_prev_function` | Goto previous function | normal: `` [f ``, select: `` [f `` |
| `goto_next_class` | Goto next type definition | normal: `` ]t ``, select: `` ]t `` |
| `goto_prev_class` | Goto previous type definition | normal: `` [t ``, select: `` [t `` |
| `goto_next_parameter` | Goto next parameter | normal: `` ]a ``, select: `` ]a `` |
| `goto_prev_parameter` | Goto previous parameter | normal: `` [a ``, select: `` [a `` |
| `goto_next_comment` | Goto next comment | normal: `` ]c ``, select: `` ]c `` |
| `goto_prev_comment` | Goto previous comment | normal: `` [c ``, select: `` [c `` |
| `goto_next_test` | Goto next test | normal: `` ]T ``, select: `` ]T `` |
| `goto_prev_test` | Goto previous test | normal: `` [T ``, select: `` [T `` |
| `goto_next_xml_element` | Goto next (X)HTML element | normal: `` ]x ``, select: `` ]x `` |
| `goto_prev_xml_element` | Goto previous (X)HTML element | normal: `` [x ``, select: `` [x `` |
| `goto_next_conflict` | Goto next merge conflict | normal: `` ]m ``, select: `` ]m `` |
| `goto_prev_conflict` | Goto previous merge conflict | normal: `` [m ``, select: `` [m `` |
| `goto_next_entry` | Goto next pairing | normal: `` ]e ``, select: `` ]e `` |
| `goto_prev_entry` | Goto previous pairing | normal: `` [e ``, select: `` [e `` |
| `goto_next_paragraph` | Goto next paragraph | normal: `` ]p ``, select: `` ]p `` |
| `goto_prev_paragraph` | Goto previous paragraph | normal: `` [p ``, select: `` [p `` |
| `dap_launch` | Launch debug target | normal: `` <space>Gl ``, select: `` <space>Gl `` |
| `dap_restart` | Restart debugging session | normal: `` <space>Gr ``, select: `` <space>Gr `` |
| `dap_toggle_breakpoint` | Toggle breakpoint | normal: `` <space>Gb ``, select: `` <space>Gb `` |
| `dap_continue` | Continue program execution | normal: `` <space>Gc ``, select: `` <space>Gc `` |
| `dap_pause` | Pause program execution | normal: `` <space>Gh ``, select: `` <space>Gh `` |
| `dap_step_in` | Step in | normal: `` <space>Gi ``, select: `` <space>Gi `` |
| `dap_step_out` | Step out | normal: `` <space>Go ``, select: `` <space>Go `` |
| `dap_next` | Step to next | normal: `` <space>Gn ``, select: `` <space>Gn `` |
| `dap_variables` | List variables | normal: `` <space>Gv ``, select: `` <space>Gv `` |
| `dap_terminate` | End debug session | normal: `` <space>Gt ``, select: `` <space>Gt `` |
| `dap_edit_condition` | Edit breakpoint condition on current line | normal: `` <space>G<C-c> ``, select: `` <space>G<C-c> `` |
| `dap_edit_log` | Edit breakpoint log message on current line | normal: `` <space>G<C-l> ``, select: `` <space>G<C-l> `` |
| `dap_switch_thread` | Switch current thread | normal: `` <space>Gst ``, select: `` <space>Gst `` |
| `dap_switch_stack_frame` | Switch stack frame | normal: `` <space>Gsf ``, select: `` <space>Gsf `` |
| `dap_enable_exceptions` | Enable exception breakpoints | normal: `` <space>Ge ``, select: `` <space>Ge `` |
| `dap_disable_exceptions` | Disable exception breakpoints | normal: `` <space>GE ``, select: `` <space>GE `` |
| `shell_pipe` | Pipe selections through shell command | normal: `` \| ``, select: `` \| `` |
| `shell_pipe_to` | Pipe selections into shell command ignoring output | normal: `` <A-\|> ``, select: `` <A-\|> `` |
| `shell_insert_output` | Insert shell command output before selections | normal: `` ! ``, select: `` ! `` |
| `shell_append_output` | Append shell command output after selections | normal: `` <A-!> ``, select: `` <A-!> `` |
| `shell_keep_pipe` | Filter selections with shell predicate | normal: `` $ ``, select: `` $ `` |
| `suspend` | Suspend and return to shell | normal: `` <C-z> ``, select: `` <C-z> `` |
| `rename_symbol` | Rename symbol | normal: `` <space>r ``, select: `` <space>r `` |
| `increment` | Increment item under cursor | normal: `` <C-a> ``, select: `` <C-a> `` |
| `decrement` | Decrement item under cursor | normal: `` <C-x> ``, select: `` <C-x> `` |
| `record_macro` | Record macro | normal: `` Q ``, select: `` Q `` |
| `replay_macro` | Replay macro | normal: `` q ``, select: `` q `` |
| `command_palette` | Open command palette | normal: `` <space>? ``, select: `` <space>? `` |
| `goto_word` | Jump to a two-character label | normal: `` gw `` |
| `extend_to_word` | Extend to a two-character label | select: `` gw `` |
| `goto_next_tabstop` | Goto next snippet placeholder |  |
| `goto_prev_tabstop` | Goto next snippet placeholder |  |
| `rotate_selections_first` | Make the first selection your primary one |  |
| `rotate_selections_last` | Make the last selection your primary one |  |
