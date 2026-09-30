//! Org's fast tag selection: the tags of `#+TAGS:` on one key each, in one
//! popup, toggled on the entry at once.
//!
//! A key toggles its tag, and turning on a tag of an exclusive group
//! (`{ @office @remote }`) turns the others off. `Space` clears every tag,
//! `Enter` writes them, `Esc` leaves the entry as it was.

use helix_roam::tags::TagSetup;
use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Context, Event, EventResult};

type Apply = Box<dyn FnOnce(&mut Context, Vec<String>) + Send>;

/// The popup, and the tags as they stand.
pub struct TagSelect {
    /// Key, tag and row, in the order declared.
    choices: Vec<(char, String, usize)>,
    setup: TagSetup,
    /// The entry's tags as edited, in order.
    tags: Vec<String>,
    on_apply: Option<Apply>,
}

impl TagSelect {
    pub const ID: &'static str = "org-tag-select";

    pub fn new(
        setup: TagSetup,
        tags: Vec<String>,
        on_apply: impl FnOnce(&mut Context, Vec<String>) + Send + 'static,
    ) -> Self {
        let choices = setup
            .keyed()
            .into_iter()
            .map(|(key, choice)| (key, choice.name.clone(), choice.row))
            .collect();
        Self {
            choices,
            setup,
            tags,
            on_apply: Some(Box::new(on_apply)),
        }
    }

    fn toggle(&mut self, tag: &str) {
        if let Some(at) = self.tags.iter().position(|own| own == tag) {
            self.tags.remove(at);
            return;
        }
        let excluded = self.setup.excluded_by(tag);
        self.tags.retain(|own| !excluded.contains(&own.as_str()));
        self.tags.push(tag.to_string());
    }

    /// The choices as laid out: one line per row, wrapped at `width`.
    fn lines(&self, width: usize) -> Vec<Vec<(char, &str)>> {
        let mut lines: Vec<Vec<(char, &str)>> = Vec::new();
        let mut row = None;
        let mut used = 0;
        for (key, name, choice_row) in &self.choices {
            let cell = name.chars().count() + 6;
            let new_row = row != Some(*choice_row);
            if new_row || used + cell > width || lines.is_empty() {
                lines.push(Vec::new());
                used = 0;
            }
            row = Some(*choice_row);
            used += cell;
            if let Some(line) = lines.last_mut() {
                line.push((*key, name.as_str()));
            }
        }
        lines
    }
}

impl Component for TagSelect {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let width = viewport.width.min(80);
        let inner_width = width.saturating_sub(2) as usize;
        let lines = self.lines(inner_width);
        let height = (lines.len() as u16 + 5).min(viewport.height);
        let area = viewport.intersection(Rect::new(
            viewport.width.saturating_sub(width) / 2,
            viewport.height.saturating_sub(height + 2),
            width,
            height,
        ));
        if area.width < 20 || area.height < 4 {
            return;
        }

        let theme = &cx.editor.theme;
        let popup = theme.get("ui.popup");
        let text = theme.get("ui.text");
        let key_style = theme.get("special");
        let on = theme.get("ui.selection").patch(theme.get("ui.text.focus"));
        let dim = theme.get("ui.text.inactive");

        surface.clear_with(area, popup);
        let block = Block::bordered().title("Tags");
        let inner = block.inner(area);
        block.render(area, surface);

        let current = if self.tags.is_empty() {
            "(none)".to_string()
        } else {
            format!(":{}:", self.tags.join(":"))
        };
        surface.set_string_truncated(
            inner.x,
            inner.y,
            &format!("Current: {current}"),
            inner.width as usize,
            |_| text,
            true,
            false,
        );

        for (index, line) in lines.iter().enumerate() {
            let y = inner.y + 2 + index as u16;
            if y >= inner.y + inner.height.saturating_sub(1) {
                break;
            }
            let mut x = inner.x;
            for (key, name) in line {
                let selected = self.tags.iter().any(|own| own == name);
                surface.set_string(x, y, &format!("[{key}]"), key_style);
                x += 4;
                let style = if selected { on } else { text };
                surface.set_string(x, y, name, style);
                x += name.chars().count() as u16 + 2;
            }
        }

        let help = "key: toggle · Space: clear · Enter: apply · Esc: cancel";
        surface.set_string_truncated(
            inner.x,
            inner.y + inner.height.saturating_sub(1),
            help,
            inner.width as usize,
            |_| dim,
            true,
            false,
        );
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Consumed(None);
        };
        let close = |apply: Option<(Apply, Vec<String>)>| -> EventResult {
            EventResult::Consumed(Some(Box::new(move |compositor, cx| {
                compositor.remove(TagSelect::ID);
                if let Some((apply, tags)) = apply {
                    apply(cx, tags);
                }
            })))
        };
        match key.code {
            KeyCode::Esc => close(None),
            KeyCode::Enter => {
                let apply = self.on_apply.take().map(|apply| (apply, self.tags.clone()));
                close(apply)
            }
            KeyCode::Char(' ') => {
                self.tags.clear();
                EventResult::Consumed(None)
            }
            KeyCode::Char(c)
                if key.modifiers == KeyModifiers::NONE || key.modifiers == KeyModifiers::SHIFT =>
            {
                let tag = self
                    .choices
                    .iter()
                    .find(|(key, _, _)| *key == c)
                    .map(|(_, name, _)| name.clone());
                if let Some(tag) = tag {
                    self.toggle(&tag);
                }
                EventResult::Consumed(None)
            }
            _ => EventResult::Consumed(None),
        }
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggling_respects_exclusive_groups() {
        let setup = TagSetup::parse(["@work(w) @home(h)", "{ @office(o) @remote(r) }"]);
        let mut select = TagSelect::new(setup, vec!["keep".into()], |_, _| {});
        select.toggle("@office");
        select.toggle("@work");
        assert_eq!(select.tags, ["keep", "@office", "@work"]);
        select.toggle("@remote");
        assert_eq!(select.tags, ["keep", "@work", "@remote"]);
        select.toggle("@work");
        assert_eq!(select.tags, ["keep", "@remote"]);
        // One line per `#+TAGS:` row.
        assert_eq!(select.lines(80).len(), 2);
    }
}
