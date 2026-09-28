//! The agenda's views: what is due by day, the TODO list, matches,
//! searches and one's own views, shown in a picker whose Alt keys act on
//! the entries as Org's agenda buffer does.

use super::*;

use crate::roam::{AgendaFilter, AgendaLine, ViewOptions};

/// Shows what is due, over `days` days from today. A day's view shows the
/// hours between its timed entries, as Org's does.
pub fn org_agenda_picker(editor: &mut Editor, days: i64) -> Option<Box<dyn Component>> {
    let options = ViewOptions {
        grid: days == 1,
        ..ViewOptions::default()
    };
    let rebuild: AgendaLines =
        Box::new(move |editor, options| crate::roam::agenda_lines(editor, days, options));
    let lines = rebuild(editor, &options);
    Some(agenda_view(editor, lines, "when", rebuild, Some(options)))
}

/// Shows everything unfinished, dated or not.
pub fn org_todo_list_picker(editor: &mut Editor) -> Option<Box<dyn Component>> {
    org_filtered_todo_picker(editor, &helix_roam::agenda::TodoFilter::default())
}

/// Shows the unfinished nodes matching a filter.
pub fn org_filtered_todo_picker(
    editor: &mut Editor,
    filter: &helix_roam::agenda::TodoFilter,
) -> Option<Box<dyn Component>> {
    let lines = crate::roam::filtered_todo_lines(editor, filter);
    if lines.is_empty() {
        editor.set_status("Nothing to do");
        return None;
    }
    let filter = filter.clone();
    Some(agenda_view(
        editor,
        lines,
        "category",
        Box::new(move |editor, _| crate::roam::filtered_todo_lines(editor, &filter)),
        None,
    ))
}

/// Shows the entries a match finds: Org's tags view, or with `todo_only`
/// its tags-todo view.
pub fn org_match_picker(
    editor: &mut Editor,
    query: &str,
    todo_only: bool,
) -> Option<Box<dyn Component>> {
    let query = if todo_only && !query.contains('/') {
        format!("{query}/!")
    } else {
        query.to_string()
    };
    let matcher = match helix_roam::search::Match::parse(&query, helix_roam::Date::today()) {
        Ok(matcher) => matcher,
        Err(err) => {
            editor.set_error(err);
            return None;
        }
    };
    let lines = crate::roam::match_lines(editor, &matcher);
    if lines.is_empty() {
        editor.set_status(format!("Nothing matches {query}"));
        return None;
    }
    Some(agenda_view(
        editor,
        lines,
        "category",
        Box::new(move |editor, _| crate::roam::match_lines(editor, &matcher)),
        None,
    ))
}

/// Shows the entries whose text has the words: Org's search view.
pub fn org_search_picker(editor: &mut Editor, query: &str) -> Option<Box<dyn Component>> {
    let search = match helix_roam::search::Search::parse(query) {
        Ok(search) => search,
        Err(err) => {
            editor.set_error(err);
            return None;
        }
    };
    let lines = crate::roam::search_lines(editor, &search);
    if lines.is_empty() {
        editor.set_status(format!("Nothing has {query}"));
        return None;
    }
    Some(agenda_view(
        editor,
        lines,
        "category",
        Box::new(move |editor, _| crate::roam::search_lines(editor, &search)),
        None,
    ))
}

/// Shows the projects with nothing to do next.
pub fn org_stuck_picker(editor: &mut Editor) -> Option<Box<dyn Component>> {
    let lines = match crate::roam::stuck_lines(editor) {
        Ok(lines) => lines,
        Err(err) => {
            editor.set_error(format!("stuck-projects: {err}"));
            return None;
        }
    };
    if lines.is_empty() {
        editor.set_status("No project is stuck");
        return None;
    }
    Some(agenda_view(
        editor,
        lines,
        "category",
        Box::new(|editor, _| crate::roam::stuck_lines(editor).unwrap_or_default()),
        None,
    ))
}

/// Shows a custom agenda view, its blocks one after the other.
pub fn org_custom_view_picker(
    editor: &mut Editor,
    view: helix_view::editor::AgendaView,
) -> Option<Box<dyn Component>> {
    let lines = match crate::roam::custom_view_lines(editor, &view) {
        Ok(lines) => lines,
        Err(err) => {
            editor.set_error(format!("{}: {err}", view.name));
            return None;
        }
    };
    if lines.iter().all(crate::roam::AgendaLine::is_heading) {
        editor.set_status(format!("{}: nothing to show", view.name));
        return None;
    }
    Some(agenda_view(
        editor,
        lines,
        "when",
        Box::new(move |editor, _| {
            crate::roam::custom_view_lines(editor, &view).unwrap_or_default()
        }),
        None,
    ))
}

/// What the agenda dispatcher offers: a built-in view or one's own.
#[derive(Clone)]
enum AgendaChoice {
    Days(i64),
    Todo,
    Match { todo_only: bool },
    Search,
    Stuck,
    Custom(helix_view::editor::AgendaView),
}

/// A line of the agenda dispatcher.
#[derive(Clone)]
struct AgendaEntry {
    key: String,
    name: String,
    choice: AgendaChoice,
}

/// The agenda dispatcher, Org's `C-c a`: the built-in views and the custom
/// ones from `[[editor.roam.agenda-views]]`, by key. With `key`, the view
/// opens without the picker.
pub fn org_agenda_dispatch_picker(
    editor: &mut Editor,
    key: Option<&str>,
) -> Option<Box<dyn Component>> {
    let builtin = [
        ("a", "Agenda for the week", AgendaChoice::Days(7)),
        ("d", "Agenda for today", AgendaChoice::Days(1)),
        ("t", "Every unfinished task", AgendaChoice::Todo),
        (
            "m",
            "Match tags, properties and states",
            AgendaChoice::Match { todo_only: false },
        ),
        (
            "M",
            "Match, unfinished tasks only",
            AgendaChoice::Match { todo_only: true },
        ),
        ("s", "Search for words in the text", AgendaChoice::Search),
        ("#", "Stuck projects", AgendaChoice::Stuck),
    ];
    let custom = editor.config().roam.agenda_views.clone();
    // A custom view takes a built-in one's key over, as in Org.
    let mut entries: Vec<AgendaEntry> = custom
        .into_iter()
        .map(|view| AgendaEntry {
            key: view.key.clone(),
            name: view.name.clone(),
            choice: AgendaChoice::Custom(view),
        })
        .collect();
    for (key, name, choice) in builtin {
        if !entries.iter().any(|entry| entry.key == key) {
            entries.push(AgendaEntry {
                key: key.to_string(),
                name: name.to_string(),
                choice,
            });
        }
    }

    if let Some(key) = key {
        return match entries.into_iter().find(|entry| entry.key == key) {
            Some(entry) => open_agenda_choice(editor, entry.choice),
            None => {
                editor.set_error(format!("No agenda view with the key {key}"));
                None
            }
        };
    }

    let columns = [
        ui::PickerColumn::new("key", |item: &AgendaEntry, _: &()| item.key.as_str().into()),
        ui::PickerColumn::new("view", |item: &AgendaEntry, _: &()| {
            item.name.as_str().into()
        }),
    ];
    let picker = Picker::new(columns, 1, entries, (), |cx, entry, _action| {
        let choice = entry.choice.clone();
        cx.jobs.callback(async move {
            let call: job::Callback = job::Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    if let Some(view) = open_agenda_choice(editor, choice) {
                        compositor.push(view);
                    }
                },
            ));
            Ok(call)
        });
    });
    Some(Box::new(overlaid(picker)))
}

/// Opens what the dispatcher chose, asking first for a match or words.
fn open_agenda_choice(editor: &mut Editor, choice: AgendaChoice) -> Option<Box<dyn Component>> {
    match choice {
        AgendaChoice::Days(days) => org_agenda_picker(editor, days),
        AgendaChoice::Todo => org_todo_list_picker(editor),
        AgendaChoice::Custom(view) => org_custom_view_picker(editor, view),
        AgendaChoice::Match { todo_only } => Some(org_agenda_query_prompt(Some(todo_only))),
        AgendaChoice::Search => Some(org_agenda_query_prompt(None)),
        AgendaChoice::Stuck => org_stuck_picker(editor),
    }
}

/// Asks for a match (`Some(todo_only)`) or the words of a search (`None`),
/// then shows what it finds.
pub fn org_agenda_query_prompt(todo_only: Option<bool>) -> Box<dyn Component> {
    let label = match todo_only {
        Some(_) => "Match (+work-boss|urgent/TODO): ",
        None => "Search (words -not \"a phrase\" {regexp}): ",
    };
    Box::new(ui::Prompt::new(
        label.into(),
        None,
        |_editor, _input| Vec::new(),
        move |cx, input, event| {
            if event != PromptEvent::Validate {
                return;
            }
            let input = input.to_string();
            cx.jobs.callback(async move {
                let call: job::Callback = job::Callback::EditorCompositor(Box::new(
                    move |editor: &mut Editor, compositor: &mut Compositor| {
                        let view = match todo_only {
                            Some(todo_only) => org_match_picker(editor, &input, todo_only),
                            None => org_search_picker(editor, &input),
                        };
                        if let Some(view) = view {
                            compositor.push(view);
                        }
                    },
                ));
                Ok(call)
            });
        },
    ))
}

/// Rebuilds an agenda's lines from the index, after an action changed it
/// or with other options.
type AgendaLines = Box<dyn Fn(&Editor, &ViewOptions) -> Vec<AgendaLine>>;

/// The keys of an agenda view, for `Alt-?`.
const AGENDA_KEYS: &str = "Alt-t/T state · Alt-s/d schedule/deadline · Alt-+/- priority · \
     Alt-i clock in · Alt-/ tags · Alt-< category · Alt-_ effort · Alt-= regexp · \
     Alt-| unfilter · Alt-m mark · Alt-* mark all · Alt-u unmark · Alt-B bulk · \
     Alt-f/b/. later/earlier/today · Alt-l log · Alt-r report · Alt-g grid · \
     Alt-c columns · Alt-w write to a file (.txt, .html, .ics)";

/// An agenda view: the picker, and keys that act on the selected entry
/// without leaving it, as Org's agenda buffer does.
///
/// The picker is upstream's and consumes its own keys, so the actions are
/// taken before it sees the event, on Alt keys it does not use. After each,
/// the view is rebuilt from the index, on the same entry; what is shown is
/// then narrowed by the view's filter, and marked entries are flagged.
pub struct AgendaView {
    picker: ui::overlay::Overlay<Picker<AgendaLine, PathStyleConfig>>,
    lines: AgendaLines,
    first: &'static str,
    /// For a view by day, how it is shown; `None` for the others.
    options: Option<ViewOptions>,
    filter: AgendaFilter,
    /// Entries marked for a bulk action, by file and line.
    /// with each entry's title, which finds it again when an edit moved it.
    marks: std::collections::BTreeMap<EntryKey, String>,
    /// Marks the last rebuild could not find again.
    lost_marks: usize,
    /// The lines the picker was given, in order.
    shown: Vec<AgendaLine>,
    /// Whether the entries show `agenda-columns` after them.
    columns: bool,
}

/// A file and a line: which entry a line of the agenda is.
type EntryKey = (PathBuf, usize);

impl AgendaView {
    fn new(
        editor: &Editor,
        lines: Vec<AgendaLine>,
        first: &'static str,
        rebuild: AgendaLines,
        options: Option<ViewOptions>,
    ) -> Self {
        let mut view = Self {
            picker: overlaid(agenda_picker(editor, Vec::new(), first)),
            lines: rebuild,
            first,
            options,
            filter: AgendaFilter::default(),
            marks: Default::default(),
            lost_marks: 0,
            shown: Vec::new(),
            columns: false,
        };
        view.show(editor, lines, None);
        view
    }

    fn selected(&self) -> Option<EntryKey> {
        self.picker
            .content
            .selection()
            .filter(|line| !line.is_heading())
            .map(|line| (line.path.clone(), line.line))
    }

    /// The entry after the selected one, for moving on after marking.
    fn next_after(&self, key: &EntryKey) -> Option<EntryKey> {
        let at = self
            .shown
            .iter()
            .position(|line| (&line.path, line.line) == (&key.0, key.1))?;
        self.shown[at + 1..]
            .iter()
            .find(|line| !line.is_heading())
            .map(|line| (line.path.clone(), line.line))
    }

    /// Rebuilds the view, keeping the cursor on `keep` if it is still there.
    fn refresh(&mut self, editor: &Editor, keep: Option<EntryKey>) {
        let options = self.options.unwrap_or_default();
        let lines = (self.lines)(editor, &options);
        self.show(editor, lines, keep);
    }

    /// Shows `lines` through the filter, with the marks.
    fn show(&mut self, editor: &Editor, lines: Vec<AgendaLine>, keep: Option<EntryKey>) {
        // An edit that adds lines (a planning line, a state's log) moves the
        // entries below it, so a mark follows its entry by title: to the
        // same line if the entry is still there, else to the nearest entry
        // of that title in the file. A mark whose entry is gone goes.
        let present: Vec<(EntryKey, &str)> = lines
            .iter()
            .filter(|line| !line.is_heading())
            .map(|line| ((line.path.clone(), line.line), line.title.as_str()))
            .collect();
        let mut kept = std::collections::BTreeMap::new();
        self.lost_marks = 0;
        for ((path, line), title) in std::mem::take(&mut self.marks) {
            let found = present
                .iter()
                .filter(|(key, own)| key.0 == path && *own == title)
                .min_by_key(|(key, _)| key.1.abs_diff(line))
                .map(|(key, _)| key.clone());
            match found {
                Some(key) if !kept.contains_key(&key) => {
                    kept.insert(key, title);
                }
                _ => self.lost_marks += 1,
            }
        }
        self.marks = kept;

        let mut shown: Vec<AgendaLine> = lines
            .into_iter()
            .filter(|line| self.filter.matches(line))
            .map(|mut line| {
                if self.marks.contains_key(&(line.path.clone(), line.line)) {
                    line.when = format!("» {}", line.when);
                }
                line
            })
            .collect();
        if self.columns {
            lay_out_columns(&mut shown, &editor.config().roam.agenda_columns);
        }
        let cursor = keep
            .and_then(|(path, line)| {
                shown
                    .iter()
                    .position(|item| item.path == path && item.line == line)
            })
            .unwrap_or(0);
        self.shown = shown.clone();
        self.picker =
            overlaid(agenda_picker(editor, shown, self.first).with_initial_cursor(cursor as u32));
    }

    fn act(&mut self, editor: &mut Editor, action: crate::roam::AgendaAction) {
        let Some((path, line)) = self.selected() else {
            return;
        };
        match crate::roam::agenda_act(editor, &path, line, action) {
            Ok(said) => editor.set_status(said),
            Err(err) => editor.set_error(err),
        }
        self.refresh(editor, Some((path, line)));
        self.say_lost_marks(editor);
    }

    /// Says when marks could not follow their entries after an edit.
    fn say_lost_marks(&self, editor: &mut Editor) {
        if self.lost_marks > 0 {
            editor.set_error(format!(
                "{} mark(s) lost: the entries are no longer found",
                self.lost_marks
            ));
        }
    }

    /// Says what the view is narrowed to and how many are marked.
    fn say_state(&self, editor: &mut Editor) {
        let mut parts = Vec::new();
        if !self.filter.is_empty() {
            parts.push(format!("Filter: {}", self.filter.describe()));
        }
        if !self.marks.is_empty() {
            parts.push(format!("{} marked", self.marks.len()));
        }
        if let Some(options) = self.options {
            let mut modes = Vec::new();
            if options.log {
                modes.push("log");
            }
            if options.report {
                modes.push("report");
            }
            if options.grid {
                modes.push("grid");
            }
            if !modes.is_empty() {
                parts.push(format!("Showing {}", modes.join(", ")));
            }
        }
        if parts.is_empty() {
            editor.set_status("No filter · Alt-? for the keys");
        } else {
            editor.set_status(parts.join(" · "));
        }
    }

    /// Changes the view's options, if it is a view by day.
    fn change_options(&mut self, editor: &mut Editor, change: impl FnOnce(&mut ViewOptions)) {
        let Some(options) = &mut self.options else {
            editor.set_error("Only a view by day has that");
            return;
        };
        change(options);
        let keep = self.selected();
        self.refresh(editor, keep);
        self.say_state(editor);
    }
}

/// Pushes a prompt whose answer changes the agenda view below it.
fn view_prompt(
    label: &'static str,
    apply: impl Fn(&mut AgendaView, &mut Editor, &str) + Send + Sync + 'static,
) -> compositor::EventResult {
    let apply = std::sync::Arc::new(apply);
    let prompt = Prompt::new(
        label.into(),
        None,
        ui::completers::none,
        move |_cx, input, event| {
            if event != PromptEvent::Validate {
                return;
            }
            let input = input.to_string();
            let apply = apply.clone();
            job::dispatch_blocking(move |editor, compositor| {
                if let Some(view) = compositor.find::<AgendaView>() {
                    apply(view, editor, &input);
                }
            });
        },
    );
    compositor::EventResult::Consumed(Some(Box::new(move |compositor: &mut Compositor, _| {
        compositor.push(Box::new(prompt))
    })))
}

impl Component for AgendaView {
    fn handle_event(
        &mut self,
        event: &compositor::Event,
        cx: &mut compositor::Context,
    ) -> compositor::EventResult {
        use crate::roam::AgendaAction;
        use compositor::EventResult::Consumed;

        let compositor::Event::Key(key) = event else {
            return self.picker.handle_event(event, cx);
        };
        // `Alt-B` arrives as Alt and Shift and `B`; the character already
        // says the Shift, as the editor's own keymap has it.
        let mut key = *key;
        if let helix_view::keyboard::KeyCode::Char(_) = key.code {
            key.modifiers
                .remove(helix_view::keyboard::KeyModifiers::SHIFT);
        }
        let key = &key;
        // A block's heading goes nowhere, and does not close the view.
        let on_heading = self
            .picker
            .content
            .selection()
            .is_some_and(AgendaLine::is_heading);
        if on_heading && *key == crate::key!(Enter) {
            return Consumed(None);
        }
        let action = match *key {
            crate::alt!('t') => Some(AgendaAction::State(true)),
            crate::alt!('T') => Some(AgendaAction::State(false)),
            crate::alt!('+') => Some(AgendaAction::Priority(true)),
            crate::alt!('-') => Some(AgendaAction::Priority(false)),
            crate::alt!('i') => Some(AgendaAction::ClockIn),
            _ => None,
        };
        if let Some(action) = action {
            self.act(cx.editor, action);
            return Consumed(None);
        }

        match *key {
            crate::alt!('?') => {
                cx.editor.set_status(AGENDA_KEYS);
                return Consumed(None);
            }
            crate::alt!('/') => {
                return view_prompt("Tags (+work -home): ", |view, editor, input| {
                    view.filter.tags = AgendaFilter::parse_tags(input);
                    let keep = view.selected();
                    view.refresh(editor, keep);
                    view.say_state(editor);
                });
            }
            crate::alt!('<') => {
                // Org's `<`: the selected entry's category, or none again.
                let category = self
                    .picker
                    .content
                    .selection()
                    .filter(|line| !line.is_heading())
                    .map(|line| line.category.clone());
                self.filter.category = match (&self.filter.category, category) {
                    (Some(_), _) | (None, None) => None,
                    (None, category) => category,
                };
                let keep = self.selected();
                self.refresh(cx.editor, keep);
                self.say_state(cx.editor);
                return Consumed(None);
            }
            crate::alt!('_') => {
                return view_prompt(
                    "Effort (<0:30, >1:00, =45; empty for none): ",
                    |view, editor, input| {
                        if input.trim().is_empty() {
                            view.filter.effort = None;
                        } else {
                            match AgendaFilter::parse_effort(input) {
                                Ok(effort) => view.filter.effort = Some(effort),
                                Err(err) => return editor.set_error(err),
                            }
                        }
                        let keep = view.selected();
                        view.refresh(editor, keep);
                        view.say_state(editor);
                    },
                );
            }
            crate::alt!('=') => {
                return view_prompt("Regexp (empty for none): ", |view, editor, input| {
                    if input.trim().is_empty() {
                        view.filter.regex = None;
                    } else {
                        match helix_core::regex::RegexBuilder::new(input)
                            .case_insensitive(true)
                            .build()
                        {
                            Ok(regex) => view.filter.regex = Some(regex),
                            Err(err) => return editor.set_error(format!("{err}")),
                        }
                    }
                    let keep = view.selected();
                    view.refresh(editor, keep);
                    view.say_state(editor);
                });
            }
            crate::alt!('|') => {
                self.filter = AgendaFilter::default();
                let keep = self.selected();
                self.refresh(cx.editor, keep);
                self.say_state(cx.editor);
                return Consumed(None);
            }
            crate::alt!('m') => {
                let title = self
                    .picker
                    .content
                    .selection()
                    .map(|line| line.title.clone());
                if let (Some(key), Some(title)) = (self.selected(), title) {
                    if self.marks.remove(&key).is_none() {
                        self.marks.insert(key.clone(), title);
                    }
                    let next = self.next_after(&key).or(Some(key));
                    self.refresh(cx.editor, next);
                    self.say_state(cx.editor);
                }
                return Consumed(None);
            }
            crate::alt!('*') => {
                self.marks = self
                    .shown
                    .iter()
                    .filter(|line| !line.is_heading())
                    .map(|line| ((line.path.clone(), line.line), line.title.clone()))
                    .collect();
                let keep = self.selected();
                self.refresh(cx.editor, keep);
                self.say_state(cx.editor);
                return Consumed(None);
            }
            crate::alt!('u') => {
                self.marks.clear();
                let keep = self.selected();
                self.refresh(cx.editor, keep);
                self.say_state(cx.editor);
                return Consumed(None);
            }
            crate::alt!('B') => {
                let marks: Vec<EntryKey> = if self.marks.is_empty() {
                    self.selected().into_iter().collect()
                } else {
                    self.marks.keys().cloned().collect()
                };
                if marks.is_empty() {
                    cx.editor.set_error("Nothing marked");
                    return Consumed(None);
                }
                let menu = bulk_menu(marks);
                return Consumed(Some(Box::new(move |compositor: &mut Compositor, _| {
                    compositor.push(menu)
                })));
            }
            crate::alt!('f') | crate::alt!('b') | crate::alt!('.') => {
                let step = match *key {
                    crate::alt!('f') => Some(1),
                    crate::alt!('b') => Some(-1),
                    _ => None,
                };
                self.change_options(cx.editor, |options| match step {
                    Some(step) => options.shift += step,
                    None => options.shift = 0,
                });
                return Consumed(None);
            }
            crate::alt!('l') => {
                self.change_options(cx.editor, |options| options.log = !options.log);
                return Consumed(None);
            }
            crate::alt!('r') => {
                self.change_options(cx.editor, |options| options.report = !options.report);
                return Consumed(None);
            }
            crate::alt!('g') => {
                self.change_options(cx.editor, |options| options.grid = !options.grid);
                return Consumed(None);
            }
            crate::alt!('c') => {
                self.columns = !self.columns;
                let keep = self.selected();
                self.refresh(cx.editor, keep);
                return Consumed(None);
            }
            crate::alt!('w') => {
                return view_prompt(
                    "Write the view to (.txt, .html, .ics): ",
                    |view, editor, input| {
                        let input = input.trim();
                        if input.is_empty() {
                            return;
                        }
                        let path = helix_stdx::path::expand_tilde(Path::new(input)).into_owned();
                        let format = crate::roam::AgendaExport::for_path(&path);
                        let title = format!("Agenda, {}", helix_roam::Date::today().to_iso());
                        let text = crate::roam::agenda_export(&view.shown, &title, format);
                        match std::fs::write(&path, text) {
                            Ok(()) => editor.set_status(format!("Wrote {}", path.display())),
                            Err(err) => editor
                                .set_error(format!("could not write {}: {err}", path.display())),
                        }
                    },
                );
            }
            _ => {}
        }

        let schedule = match *key {
            crate::alt!('s') => Some(true),
            crate::alt!('d') => Some(false),
            _ => None,
        };
        if let (Some(schedule), Some((path, line))) = (schedule, self.selected()) {
            let label = if schedule {
                "Scheduled (today, +3, 2026-09-18): "
            } else {
                "Deadline (today, +3, 2026-09-18): "
            };
            return view_prompt(label, move |view, editor, input| {
                let action = if schedule {
                    AgendaAction::Schedule(input.to_string())
                } else {
                    AgendaAction::Deadline(input.to_string())
                };
                match crate::roam::agenda_act(editor, &path, line, action) {
                    Ok(said) => editor.set_status(said),
                    Err(err) => editor.set_error(err),
                }
                view.refresh(editor, Some((path.clone(), line)));
                view.say_lost_marks(editor);
            });
        }
        self.picker.handle_event(event, cx)
    }

    fn render(
        &mut self,
        area: helix_view::graphics::Rect,
        surface: &mut tui::buffer::Buffer,
        cx: &mut compositor::Context,
    ) {
        self.picker.render(area, surface, cx);
    }

    fn cursor(
        &self,
        area: helix_view::graphics::Rect,
        editor: &Editor,
    ) -> (
        Option<helix_core::Position>,
        helix_view::graphics::CursorKind,
    ) {
        self.picker.cursor(area, editor)
    }
}

/// Puts `columns` after each entry's title, every column as wide as its
/// widest value, under a line naming them.
fn lay_out_columns(lines: &mut Vec<AgendaLine>, columns: &[String]) {
    let entries = || lines.iter().filter(|line| !line.is_heading());
    let title_width = entries()
        .map(|line| line.title.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(5, 40);
    let widths: Vec<usize> = columns
        .iter()
        .map(|name| {
            entries()
                .map(|line| line.column(name).chars().count())
                .chain(std::iter::once(name.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let row = |first: &str, values: Vec<String>| {
        let mut out = format!("{first:<title_width$.title_width$}");
        for (value, width) in values.iter().zip(&widths) {
            out.push_str(&format!(" │ {value:<width$}"));
        }
        out.trim_end().to_string()
    };
    for line in lines.iter_mut().filter(|line| !line.is_heading()) {
        line.what = row(
            &line.title,
            columns.iter().map(|name| line.column(name)).collect(),
        );
    }
    let header = AgendaLine {
        what: row("ITEM", columns.to_vec()),
        ..AgendaLine::default()
    };
    lines.insert(0, header);
}

/// What a bulk action does to each marked entry.
#[derive(Clone, Copy)]
enum Bulk {
    State,
    Schedule,
    Deadline,
    TagAdd,
    TagRemove,
    Refile,
}

/// The bulk actions, Org's `B`, over `marks`.
fn bulk_menu(marks: Vec<EntryKey>) -> Box<dyn Component> {
    let actions = vec![
        ("t", "Set the state", Bulk::State),
        ("s", "Schedule", Bulk::Schedule),
        ("d", "Set the deadline", Bulk::Deadline),
        ("+", "Add a tag", Bulk::TagAdd),
        ("-", "Remove a tag", Bulk::TagRemove),
        ("r", "Refile under a node", Bulk::Refile),
    ];
    let count = marks.len();
    let columns = [
        ui::PickerColumn::new("key", |item: &(&str, &str, Bulk), _: &()| item.0.into()),
        ui::PickerColumn::new(
            format!("on {count} entr{}", if count == 1 { "y" } else { "ies" }),
            |item: &(&str, &str, Bulk), _: &()| item.1.into(),
        ),
    ];
    let picker = Picker::new(columns, 1, actions, (), move |cx, item, _action| {
        let bulk = item.2;
        let marks = marks.clone();
        cx.jobs.callback(async move {
            let call: job::Callback = job::Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    if let Some(next) = bulk_ask(editor, bulk, marks) {
                        compositor.push(next);
                    }
                },
            ));
            Ok(call)
        });
    });
    Box::new(overlaid(picker))
}

/// Asks what the bulk action needs: a state, a date, a tag or a node.
fn bulk_ask(editor: &mut Editor, bulk: Bulk, marks: Vec<EntryKey>) -> Option<Box<dyn Component>> {
    use crate::roam::AgendaAction;

    if let Bulk::Refile = bulk {
        let targets = crate::roam::refile_targets(editor);
        let columns = [ui::PickerColumn::new(
            "node",
            |item: &crate::roam::RefileTarget, _: &()| item.title.as_str().into(),
        )];
        let picker = Picker::new(columns, 0, targets, (), move |cx, target, _action| {
            let target = target.clone();
            let marks = marks.clone();
            bulk_apply(cx.editor, marks, move |editor, path, line| {
                crate::roam::agenda_refile(editor, path, line, &target)
            });
        });
        return Some(Box::new(overlaid(picker)));
    }

    let label = match bulk {
        Bulk::State => "State (TODO, DONE…; empty for none): ",
        Bulk::Schedule => "Scheduled (today, +3, 2026-09-18): ",
        Bulk::Deadline => "Deadline (today, +3, 2026-09-18): ",
        Bulk::TagAdd => "Add the tag: ",
        Bulk::TagRemove => "Remove the tag: ",
        Bulk::Refile => unreachable!(),
    };
    let tags = matches!(bulk, Bulk::TagAdd | Bulk::TagRemove);
    let known = if tags {
        crate::roam::known_tags(editor)
    } else {
        Vec::new()
    };
    Some(Box::new(Prompt::new(
        label.into(),
        None,
        move |_editor, input| {
            known
                .iter()
                .filter(|tag| tag.contains(input))
                .map(|tag| (0.., tag.clone().into()))
                .collect()
        },
        move |cx, input, event| {
            if event != PromptEvent::Validate {
                return;
            }
            let input = input.trim().to_string();
            let action = move || match bulk {
                Bulk::State => {
                    AgendaAction::SetState((!input.is_empty()).then(|| input.to_uppercase()))
                }
                Bulk::Schedule => AgendaAction::Schedule(input.clone()),
                Bulk::Deadline => AgendaAction::Deadline(input.clone()),
                Bulk::TagAdd => AgendaAction::Tag(input.trim_matches(':').to_string(), true),
                Bulk::TagRemove => AgendaAction::Tag(input.trim_matches(':').to_string(), false),
                Bulk::Refile => unreachable!(),
            };
            bulk_apply(cx.editor, marks.clone(), move |editor, path, line| {
                crate::roam::agenda_act(editor, path, line, action())
            });
        },
    )))
}

/// Does `act` to every marked entry, last line of each file first so the
/// others stay where they were, then says how it went and rebuilds the
/// view without the marks.
fn bulk_apply(
    editor: &mut Editor,
    mut marks: Vec<EntryKey>,
    act: impl Fn(&mut Editor, &Path, usize) -> Result<String, String>,
) {
    marks.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut done = 0;
    let mut failed = Vec::new();
    for (path, line) in &marks {
        match act(editor, path, *line) {
            Ok(_) => done += 1,
            Err(err) => failed.push(err),
        }
    }
    if failed.is_empty() {
        editor.set_status(format!("Bulk: {done} done"));
    } else {
        editor.set_error(format!(
            "Bulk: {done} done, {} failed: {}",
            failed.len(),
            failed.join("; ")
        ));
    }
    job::dispatch_blocking(|editor, compositor| {
        if let Some(view) = compositor.find::<AgendaView>() {
            view.marks.clear();
            view.refresh(editor, None);
        }
    });
}

/// An agenda view over `lines`, rebuilt with `rebuild` after an action;
/// `options` for a view by day.
fn agenda_view(
    editor: &mut Editor,
    lines: Vec<AgendaLine>,
    first: &'static str,
    rebuild: AgendaLines,
    options: Option<ViewOptions>,
) -> Box<dyn Component> {
    editor.set_status(
        "Alt-t state · Alt-s schedule · Alt-/ tags · Alt-m mark · Alt-B bulk · Alt-? more",
    );
    Box::new(AgendaView::new(editor, lines, first, rebuild, options))
}

/// The picker both views share: a column of context, then the entry.
fn agenda_picker(
    editor: &Editor,
    lines: Vec<AgendaLine>,
    first: &'static str,
) -> Picker<AgendaLine, PathStyleConfig> {
    let columns = [
        ui::PickerColumn::new(
            first,
            |item: &crate::roam::AgendaLine, _: &PathStyleConfig| item.when.as_str().into(),
        ),
        ui::PickerColumn::new(
            "entry",
            |item: &crate::roam::AgendaLine, _: &PathStyleConfig| item.what.as_str().into(),
        ),
        ui::PickerColumn::new(
            "habit",
            |item: &crate::roam::AgendaLine, config: &PathStyleConfig| config.habit(&item.habit),
        ),
        ui::PickerColumn::new(
            "path",
            |item: &crate::roam::AgendaLine, config: &PathStyleConfig| {
                if item.is_heading() {
                    return "".into();
                }
                config.stylize(Some(item.path.as_path()), Some(item.line))
            },
        ),
    ];

    let picker = Picker::new(
        columns,
        1, // the entry itself is what a search is for
        lines,
        PathStyleConfig::new(&editor.theme),
        |cx, item, action| {
            if item.is_heading() {
                return;
            }
            if let Err(err) = cx.editor.open(&item.path, action) {
                cx.editor
                    .set_error(format!("Failed to open '{}': {}", item.path.display(), err));
                return;
            }
            let doc = doc!(cx.editor);
            if item.line < doc.text().len_lines() {
                let pos = doc.text().line_to_char(item.line);
                let view_id = view!(cx.editor).id;
                doc_mut!(cx.editor).set_selection(view_id, Selection::point(pos));
            }
        },
    );

    picker
}
