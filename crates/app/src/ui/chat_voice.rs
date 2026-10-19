//! Dictation into a conversation's field.
//!
//! The button at the right of the field (and the command `Dictate`) opens
//! the microphone; pressing it again, or Enter, closes it and what was said
//! is written where the cursor is, to be read and changed before it is
//! sent. Escape closes the microphone and writes nothing.
//!
//! Nothing of this is on until the person asks. Dictation needs a speech
//! model, which is a file they choose to download: until there is one the
//! button opens the list of the models offered for their language
//! (`/voice` opens it at any time), and choosing one downloads it. The
//! microphone is only ever opened by that button, and what it hears stays
//! on this computer.
//!
//! One microphone, so one dictation at a time for the whole window
//! ([`Dictating`]): the field it was started in ([`Field`]: a conversation's,
//! or one of King's) is the one that gets the text.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::prelude::*;
use gpui_kit::{div, AnyElement, ClickEvent, Context, SharedString, Task, Window};
use leon_voice::catalog::{self, Model};

use super::chat_menu::MenuKind;
use super::live::LiveId;
use super::shell::Shell;
use super::widgets::{led, mono};
use crate::chat::complete::{self, Voices};
use crate::schema;
use crate::settings;
use crate::theme::{metrics, px, Palette};
use crate::voice::Listening;

/// How often the button is drawn again while something moves in it: how
/// loud the person is, how far a download got.
const TICK: Duration = Duration::from_millis(120);

/// The field dictation writes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// A conversation's.
    Chat(LiveId),
    /// King's, or an open worker's in King's view.
    King(leon_king::coordinator::Target),
}

/// What dictation is doing.
#[derive(Default)]
pub enum Now {
    /// Nothing.
    #[default]
    Idle,
    /// A model is being downloaded.
    Downloading {
        /// Which.
        model: &'static Model,
        /// The bytes there are of it.
        bytes: Arc<AtomicU64>,
        _task: Task<()>,
    },
    /// The microphone is open.
    Listening {
        /// The field that gets the text.
        into: Field,
        /// The microphone.
        recording: Box<dyn Listening>,
        _tick: Task<()>,
    },
    /// What was heard is being made into text.
    Writing {
        /// The field that gets the text.
        into: Field,
        _task: Task<()>,
    },
}

/// Dictation, for the whole window.
#[derive(Default)]
pub struct Dictating {
    /// What it is doing.
    pub now: Now,
    /// The ids of the models on this computer, as last looked at.
    pub installed: Vec<String>,
    /// What went wrong last, until something else is asked of it.
    pub problem: Option<String>,
}

impl Shell {
    /// The language the person dictates in, from the setting: the system's
    /// own, one they named, or `None` for whichever is heard.
    fn dictation_language(&self, cx: &gpui_kit::App) -> Option<&'static str> {
        match settings::text(cx, "dictation_language").as_str() {
            "auto" => None,
            "system" | "" => self.options.voice.language(),
            code => catalog::LANGUAGES
                .iter()
                .find(|(have, _)| *have == code)
                .map(|(have, _)| *have),
        }
    }

    /// The model dictation uses: the one chosen when it is on this computer
    /// and serves the language, else the most accurate that does.
    fn dictation_model(&self, cx: &gpui_kit::App) -> Option<&'static Model> {
        let chosen = settings::text(cx, "dictation_model");
        catalog::in_use(
            self.dictation_language(cx),
            Some(chosen.as_str()).filter(|id| !id.is_empty()),
            &self.dictation.installed,
        )
    }

    /// What is known of the speech models, for their list.
    pub(super) fn voices(&self, cx: &gpui_kit::App) -> Voices<'_> {
        Voices {
            language: self.dictation_language(cx),
            installed: &self.dictation.installed,
            in_use: self.dictation_model(cx).map(|model| model.id),
            downloading: match &self.dictation.now {
                Now::Downloading { model, bytes, .. } => {
                    Some((model.id, bytes.load(Ordering::Relaxed)))
                }
                _ => None,
            },
        }
    }

    /// The title of the list of speech models: whose language they are for.
    pub(super) fn voices_title(&self, cx: &gpui_kit::App) -> String {
        match self.dictation_language(cx).and_then(catalog::language_name) {
            Some(name) => format!("DICTATION \u{b7} {}", name.to_uppercase()),
            None => "DICTATION \u{b7} ANY LANGUAGE".to_owned(),
        }
    }

    /// The conversation whose microphone is open, if one is.
    pub(super) fn dictating_in(&self) -> Option<LiveId> {
        match &self.dictation.now {
            Now::Listening {
                into: Field::Chat(chat),
                ..
            } => Some(*chat),
            _ => None,
        }
    }

    /// The field the microphone is open for, if it is.
    pub(super) fn dictating_into(&self) -> Option<Field> {
        match &self.dictation.now {
            Now::Listening { into, .. } => Some(*into),
            _ => None,
        }
    }

    /// The button of dictation, or the command: opens the microphone for a
    /// conversation, closes the one that is open, or, with no model to
    /// dictate with, opens the list to get one.
    pub(super) fn chat_dictate(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        if !self.chats.contains_key(&id) {
            return;
        }
        self.dictate(Field::Chat(id), window, cx);
        // A click on the button took the keyboard from the field: what is
        // said lands there, and Enter and Escape are read there.
        let listing = self.chats.get(&id).is_some_and(|chat| chat.menu.is_some());
        if let Some(chat) = self.chats.get(&id).filter(|_| !listing) {
            chat.input.update(cx, |input, cx| input.focus(window, cx));
        }
        cx.notify();
    }

    /// Opens the microphone for a field, closes the one that is open, or,
    /// with no model to dictate with, offers to get one: the list of models
    /// over a conversation's field, a word under King's (whose field has no
    /// list; any conversation's `/voice` and the settings have it).
    pub(super) fn dictate(&mut self, into: Field, window: &mut Window, cx: &mut Context<Self>) {
        self.dictation.problem = None;
        match (&self.dictation.now, into) {
            (Now::Listening { .. }, _) => self.dictation_finish(window, cx),
            (Now::Writing { .. }, _) => {}
            (Now::Downloading { .. }, Field::Chat(id)) => self.chat_show_voices(id, window, cx),
            (Now::Downloading { .. }, Field::King(_)) => {}
            (Now::Idle, _) => {
                self.dictation.installed = self.options.voice.installed();
                match into {
                    _ if self.dictation_model(cx).is_some() => self.dictation_listen(into, cx),
                    Field::Chat(id) => self.chat_show_voices(id, window, cx),
                    Field::King(_) => {
                        self.dictation.problem = Some(
                            "Choose a speech model first: /voice in a conversation, or \
                             Settings, Dictation."
                                .to_owned(),
                        );
                    }
                }
            }
        }
        cx.notify();
    }

    /// Opens the list of speech models over a conversation's field, unless
    /// it is the list already open.
    fn chat_show_voices(&mut self, id: LiveId, window: &mut Window, cx: &mut Context<Self>) {
        let open = self
            .chats
            .get(&id)
            .and_then(|chat| chat.menu.as_ref())
            .is_some_and(|menu| menu.kind == MenuKind::Voices);
        if !open {
            self.chat_open_menu(id, MenuKind::Voices, window, cx);
        }
    }

    /// Opens the microphone for a field.
    fn dictation_listen(&mut self, into: Field, cx: &mut Context<Self>) {
        match self.options.voice.listen() {
            Ok(recording) => {
                let tick = cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(TICK).await;
                    let listening = this.update(cx, |this, cx| {
                        cx.notify();
                        matches!(this.dictation.now, Now::Listening { .. })
                    });
                    if !listening.unwrap_or(false) {
                        return;
                    }
                });
                self.dictation.now = Now::Listening {
                    into,
                    recording,
                    _tick: tick,
                };
            }
            Err(error) => self.dictation.problem = Some(sentence(&error.to_string())),
        }
    }

    /// Closes the microphone and has what it heard made into text, which is
    /// written in the conversation it was opened for.
    pub(super) fn dictation_finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Now::Listening {
            into, recording, ..
        } = std::mem::take(&mut self.dictation.now)
        else {
            return;
        };
        cx.notify();
        let samples = recording.finish();
        let Some(model) = self.dictation_model(cx) else {
            return;
        };
        if !leon_voice::audio::worth_hearing(&samples) {
            self.dictation.problem = Some("Nothing was heard.".to_owned());
            return;
        }
        let language = self.dictation_language(cx);
        let voice = self.options.voice.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let said = cx
                .background_spawn(async move { voice.transcribe(model, &samples, language) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.dictation.now = Now::Idle;
                match said {
                    Ok(said) if said.trim().is_empty() => {
                        this.dictation.problem = Some("Nothing was heard.".to_owned());
                    }
                    Ok(said) => match into {
                        Field::Chat(chat) => this.chat_write_said(chat, &said, window, cx),
                        Field::King(target) => this.king_write(target, &said, window, cx),
                    },
                    Err(error) => this.dictation.problem = Some(sentence(&error.to_string())),
                }
                cx.notify();
            })
            .ok();
        });
        self.dictation.now = Now::Writing { into, _task: task };
    }

    /// Closes the microphone and writes nothing.
    pub(super) fn dictation_cancel(&mut self, cx: &mut Context<Self>) {
        if matches!(self.dictation.now, Now::Listening { .. }) {
            self.dictation.now = Now::Idle;
            cx.notify();
        }
    }

    /// Writes what was said where the cursor is in a conversation's field.
    fn chat_write_said(
        &mut self,
        id: LiveId,
        said: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(chat) = self.chats.get(&id) else {
            return;
        };
        chat.input.update(cx, |input, cx| {
            let (text, cursor) = complete::dictated(&input.value(), input.cursor(), said);
            input.set_value(text, window, cx);
            input.set_selected_range(cursor..cursor, cx);
        });
    }

    /// A speech model was chosen in the list: dictation uses it, after
    /// downloading it when it is not on this computer.
    pub(super) fn dictation_choose(
        &mut self,
        chat: LiveId,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = catalog::find(id) else {
            return;
        };
        self.dictation.problem = None;
        if self.dictation.installed.iter().any(|have| have == model.id) {
            use_model(model, cx);
            self.chat_close_menu(chat, window, cx);
            return;
        }
        if !matches!(self.dictation.now, Now::Idle) {
            // One thing at a time: the list shows what is going on.
            return;
        }
        let bytes = Arc::new(AtomicU64::new(0));
        let seen = bytes.clone();
        let download = self.options.voice.download(
            model,
            Arc::new(move |have| seen.store(have, Ordering::Relaxed)),
        );
        let task = cx.spawn(async move |this, cx| {
            let mut download = Box::pin(download);
            let done = loop {
                // Drawn again while it goes, so the list shows how far.
                let tick = cx.background_executor().timer(TICK);
                match first_of(&mut download, tick).await {
                    Some(done) => break done,
                    None => {
                        if this.update(cx, |_, cx| cx.notify()).is_err() {
                            return;
                        }
                    }
                }
            };
            this.update(cx, |this, cx| {
                this.dictation.now = Now::Idle;
                this.dictation.installed = this.options.voice.installed();
                match done {
                    Ok(()) => use_model(model, cx),
                    Err(error) => this.dictation.problem = Some(sentence(&error.to_string())),
                }
                cx.notify();
            })
            .ok();
        });
        self.dictation.now = Now::Downloading {
            model,
            bytes,
            _task: task,
        };
        cx.notify();
    }

    /// The button of dictation at the right of a conversation's field, with
    /// what went wrong last in front of it.
    pub(super) fn chat_dictate_button(
        &self,
        id: LiveId,
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.dictate_button(
            Field::Chat(id),
            (
                format!("chat-dictate-{}", id.0),
                format!("chat-dictate-problem-{}", id.0),
            ),
            colours,
            cx,
        )
    }

    /// The button of dictation of a field, with what went wrong last in
    /// front of it; `names` are the selectors of the two.
    pub(super) fn dictate_button(
        &self,
        field: Field,
        names: (String, String),
        colours: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (name, problem_name) = names;
        let hover = colours.surface_2;
        let (words, light): (String, Option<gpui_kit::Hsla>) = match &self.dictation.now {
            Now::Listening {
                into, recording, ..
            } if *into == field => {
                // The light breathes with the voice.
                let loud = recording.level().clamp(0.0, 1.0);
                (
                    "LISTENING".to_owned(),
                    Some(colours.signal.opacity(0.45 + 0.55 * loud)),
                )
            }
            Now::Writing { into, .. } if *into == field => ("WRITING\u{2026}".to_owned(), None),
            Now::Downloading { model, bytes, .. } => (
                format!(
                    "DOWNLOADING {}%",
                    bytes.load(Ordering::Relaxed).min(model.bytes) * 100 / model.bytes
                ),
                None,
            ),
            _ => ("DICTATE".to_owned(), None),
        };
        let listening = light.is_some();
        let button = div()
            .id(SharedString::from(name.clone()))
            .debug_selector(move || name.clone())
            .flex_none()
            .h(px(26.))
            .px(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(metrics::RADIUS())
            .border_1()
            .border_color(if listening {
                colours.signal
            } else {
                colours.elevated_border
            })
            .text_color(if listening {
                colours.text
            } else {
                colours.text_muted
            })
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .children(light.map(led))
            .child(mono(words))
            .on_click(
                cx.listener(move |this, _: &ClickEvent, window, cx| match field {
                    Field::Chat(id) => this.chat_dictate(id, window, cx),
                    Field::King(_) => this.dictate(field, window, cx),
                }),
            );
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(self.dictation.problem.clone().map(|problem| {
                div()
                    .debug_selector(move || problem_name.clone())
                    .max_w(px(320.))
                    .truncate()
                    .text_size(metrics::TEXT_SMALL())
                    .text_color(colours.warning)
                    .child(problem)
            }))
            .child(button)
            .into_any_element()
    }
}

/// Dictation uses `model` from now on.
fn use_model(model: &'static Model, cx: &mut gpui_kit::App) {
    if let Some(def) = schema::find("dictation_model") {
        settings::set_value(cx, def, schema::Value::Text(model.id.to_owned()));
    }
}

/// An error as a sentence a person reads: a capital, a full stop.
fn sentence(error: &str) -> String {
    let mut letters = error.trim().trim_end_matches('.').chars();
    match letters.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), letters.as_str()),
        None => String::new(),
    }
}

/// `first` when it ends before `tick` does, `None` when the tick came
/// first. `first` is polled again by the next call.
async fn first_of<F, T>(first: &mut std::pin::Pin<Box<F>>, tick: Task<()>) -> Option<T>
where
    F: Future<Output = T> + ?Sized,
{
    let mut tick = Box::pin(tick);
    std::future::poll_fn(|context| {
        if let std::task::Poll::Ready(done) = first.as_mut().poll(context) {
            return std::task::Poll::Ready(Some(done));
        }
        tick.as_mut().poll(context).map(|()| None)
    })
    .await
}
