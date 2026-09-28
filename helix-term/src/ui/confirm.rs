//! A modal yes/no question.
//!
//! Used before anything that can destroy work the user cannot get back. It is
//! deliberately modal and deliberately not a text prompt: the answer is one
//! key, and no other key can be mistaken for consent.

use helix_view::graphics::Rect;
use helix_view::input::{KeyCode, KeyModifiers};
use tui::buffer::Buffer as Surface;
use tui::widgets::{Block, Widget};

use crate::compositor::{Component, Context, Event, EventResult};

type Answer = Box<dyn FnOnce(&mut crate::compositor::Context) + Send>;

/// Asks a question that only `y` answers.
pub struct Confirm {
    question: String,
    detail: String,
    on_confirm: Option<Answer>,
}

impl Confirm {
    pub const ID: &'static str = "confirm";

    pub fn new(
        question: impl Into<String>,
        detail: impl Into<String>,
        on_confirm: impl FnOnce(&mut crate::compositor::Context) + Send + 'static,
    ) -> Self {
        Self {
            question: question.into(),
            detail: detail.into(),
            on_confirm: Some(Box::new(on_confirm)),
        }
    }
}

impl Component for Confirm {
    fn render(&mut self, viewport: Rect, surface: &mut Surface, cx: &mut Context) {
        let width =
            (self.detail.len().max(self.question.len()) + 6).min(viewport.width as usize) as u16;
        let height = 4;
        let area = viewport.intersection(Rect::new(
            viewport.width.saturating_sub(width) / 2,
            viewport.height.saturating_sub(height) / 2,
            width,
            height,
        ));
        if area.width < 8 || area.height < 3 {
            return;
        }

        let popup = cx.editor.theme.get("ui.popup");
        let warning = cx.editor.theme.get("warning");
        let text = cx.editor.theme.get("ui.text");

        surface.clear_with(area, popup);
        let block = Block::bordered().title("Confirm").border_style(warning);
        let inner = block.inner(area);
        block.render(area, surface);

        if inner.height == 0 {
            return;
        }
        surface.set_string_truncated(
            inner.x,
            inner.y,
            &self.question,
            inner.width as usize,
            |_| warning,
            true,
            false,
        );
        if inner.height > 1 {
            surface.set_string_truncated(
                inner.x,
                inner.y + 1,
                &self.detail,
                inner.width as usize,
                |_| text,
                true,
                false,
            );
        }
    }

    fn handle_event(&mut self, event: &Event, _cx: &mut Context) -> EventResult {
        let Event::Key(key) = event else {
            return EventResult::Consumed(None);
        };

        // Only an unmodified `y` is consent; everything else cancels, so a
        // stray keypress can never confirm.
        let confirmed = key.code == KeyCode::Char('y') && key.modifiers == KeyModifiers::NONE;
        let answer = confirmed.then(|| self.on_confirm.take()).flatten();

        EventResult::Consumed(Some(Box::new(move |compositor, cx| {
            compositor.remove(Confirm::ID);
            if let Some(answer) = answer {
                answer(cx);
            }
        })))
    }

    fn id(&self) -> Option<&'static str> {
        Some(Self::ID)
    }
}
