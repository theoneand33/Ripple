//! AI Assistant: one question to an OpenAI-compatible service, its answer streamed in as Markdown.

use rune::{El, TextEdit, Transition, col, el, row, text, text_input};
use rune_kit::Markdown;

use super::{Scrolls, button, tab_arrows};
use crate::island::Island;

#[derive(Default)]
pub struct Assistant {
    pub ask: TextEdit,
    pub asked: bool,
    pub answer: Option<String>,
    /// Bumped on every question, so a stream for an earlier one is dropped.
    generation: u64,
}

impl Island {
    pub fn ask_ai(&mut self) {
        self.ai.asked = true;
        self.ai.generation += 1;
        let generation = self.ai.generation;
        let provider = self.cfg.provider();
        // A keyless (local) provider never gets the key saved for another one.
        let key = if provider.key_hint.is_empty() { String::new() } else { self.cfg.api_key.trim().to_string() };
        let url = self.cfg.ai_url();
        if key.is_empty() && !provider.key_hint.is_empty() {
            self.ai.answer = Some("Enter your API key in settings".into());
            return;
        }
        if !key.is_empty() && !url.starts_with("https://") {
            self.ai.answer = Some("Use an https:// Base URL with an API key".into());
            return;
        }
        self.ai.answer = None;
        let Some(me) = self.me.clone() else { return };
        let model = if self.cfg.ai_model.trim().is_empty() { provider.model.to_string() } else { self.cfg.ai_model.clone() };
        let question = self.ai.ask.text.clone();
        std::thread::spawn(move || {
            let stream = me.clone();
            let result = crate::net::ask_ai(&url, &model, &key, &question, |delta| {
                let delta = delta.to_string();
                stream.update(move |s, _| if s.ai.generation == generation { s.ai.answer.get_or_insert_default().push_str(&delta) });
            });
            let ending = match result {
                Ok(true) => return,
                Ok(false) => "No response received. Check your settings.".to_string(),
                Err(e) => format!("Error: {e}"),
            };
            me.update(move |s, _| if s.ai.generation == generation { s.ai.answer = Some(ending) });
        });
    }

    fn ask_another(&mut self) {
        self.ai.generation += 1;
        self.ai.asked = false;
        self.ai.answer = None;
        self.ai.ask.clear();
    }
}

pub fn view(s: &Island) -> El {
    let (bg, fg) = (s.cfg.bg().alpha(1.0), s.cfg.text());
    if !s.ai.asked {
        let input = text_input(&s.ai.ask, s.cfg.font(20.0, 400.0).lh(26.0)).key("userinput").label("Ask").w_full().grow(1.0)
            .color(fg).placeholder("Ask", fg.alpha(0.7 * fg.a))
            .on_edit(|s: &mut Island| &mut s.ai.ask)
            .on_submit(|s: &mut Island, _| s.ask_ai())
            .on_input_key(tab_arrows);
        return col().key("ask").w_full().h_full().pt(16.0).px(24.0).pb(20.0).gap(8.0)
            .transition(Transition::blur_replace())
            .child(col().w_full().grow(1.0).min_h(0.0).padding(8.0).scrolls().child(input))
            .child(row().w_full().justify_end().child(button(s, "chatsubmit", "Ask", true).on_press(|s: &mut Island| s.ask_ai())));
    }
    let answer = match &s.ai.answer {
        Some(answer) if !answer.is_empty() => Markdown::new(answer.as_str()).selectable(true).streaming(true).build(&s.cfg.kit()),
        _ => text("Thinking…").font(s.cfg.font(14.0, 400.0).italic()).color(fg.alpha(0.5 * fg.a)),
    };
    el().key("result").w_full().h_full()
        .transition(Transition::blur_replace())
        .child(
            col().key("answer-scroll").w_full().h_full().scrolls().scroll_edge_fade(16.0)
                .child(col().w_full().px(24.0).pt(28.0).pb(56.0).child(answer)),
        )
        .child(button(s, "ask-another", "Ask another", true).pin(None, None, Some(16.0), Some(15.0))
            .shadow(2.0, 10.0, bg.alpha(0.5))
            .on_press(Island::ask_another))
}
