use std::{cell::Cell, path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Result;
use gpui_kit::assets::IconName;
use gpui_kit::{
    Anchor, App, AssetSource, Bounds, Context, Entity, ListAlignment, ListState, Render,
    SharedString, Subscription, Task, Window, WindowBounds, WindowOptions,
    component::{
        ActiveTheme, Disableable, Selectable, StyledExt, Theme, ThemeMode,
        button::{Button, ButtonVariants},
        input::{Input, InputEvent, InputState},
        popover::Popover,
        select::{Select, SelectEvent, SelectItem, SelectState},
    },
    div, list,
    prelude::*,
    px, rgb, size,
};
use talk_core::{
    application::{Action, AppState, Application, Effect, Navigation},
    backend::Backend,
    chat::{ChatMessage, ChatRoom, Event, Start},
    runtime::BackgroundRuntime,
    voice::{self, Phase},
};

// The default component bundle omits these two application navigation icons.
gpui_kit::assets::icon_assets!(
    ChatIcons,
    [
        Hash,
        MessageCircle,
        Volume2,
        MicOff,
        Mic,
        Headphones,
        PhoneOff,
        Settings
    ]
);
struct TalkAssets;
impl AssetSource for TalkAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ChatIcons.load(path)? {
            Some(asset) => Ok(Some(asset)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut assets = gpui_kit::assets::Assets.list(path)?;
        assets.extend(ChatIcons.list(path)?);
        Ok(assets)
    }
}

pub fn run(runtime: BackgroundRuntime, root: PathBuf, preview: bool) -> Result<()> {
    let runtime = Arc::new(runtime);
    let window_failed = Rc::new(Cell::new(false));
    let failed = window_failed.clone();
    gpui_kit::application()
        .with_assets(TalkAssets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);
            cx.set_app_identity("chat.talk.Talk", "Talk");
            Theme::change(ThemeMode::Dark, None, cx);
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let options = WindowOptions {
                app_id: Some("chat.talk.Talk".into()),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1120.), px(760.)),
                    cx,
                ))),
                window_min_size: Some(size(px(800.), px(560.))),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("Talk".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            match gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TalkApp::new(runtime, root, preview, window, cx))
            }) {
                Ok(_) => {
                    cx.activate(true);
                    tracing::info!("desktop window opened");
                }
                Err(_) => {
                    failed.set(true);
                    tracing::error!("could not open desktop window");
                    cx.quit();
                }
            }
        });
    if window_failed.get() {
        anyhow::bail!(
            "could not open the desktop window; check your display session and graphics driver"
        );
    }
    Ok(())
}

#[derive(Clone)]
struct AudioDevice {
    id: String,
    name: SharedString,
}
impl SelectItem for AudioDevice {
    type Value = String;
    fn title(&self) -> SharedString {
        self.name.clone()
    }
    fn value(&self) -> &String {
        &self.id
    }
}

struct TalkApp {
    runtime: Arc<BackgroundRuntime>,
    root: PathBuf,
    model: Application,
    homeserver: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
    recovery: Entity<InputState>,
    filter: Entity<InputState>,
    composer: Entity<InputState>,
    audio_settings: bool,
    microphone: Entity<SelectState<Vec<AudioDevice>>>,
    speaker: Entity<SelectState<Vec<AudioDevice>>>,
    list: ListState,
    backend: Option<Backend>,
    events: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

// Snapshot only values needed to update widgets and preserve the reading position.
struct PresentationState {
    navigation: Navigation,
    selection_version: u64,
    reset_version: u64,
    draft: String,
    messages: Arc<Vec<ChatMessage>>,
    voice: voice::State,
}

impl PresentationState {
    fn capture(state: &AppState) -> Self {
        Self {
            navigation: state.navigation.clone(),
            selection_version: state.selection_version,
            reset_version: state.reset_version,
            draft: state.draft().to_owned(),
            messages: state.messages.clone(),
            voice: state.voice.clone(),
        }
    }
}

impl TalkApp {
    fn new(
        runtime: Arc<BackgroundRuntime>,
        root: PathBuf,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let homeserver =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://matrix.org"));
        homeserver.update(cx, |input, cx| {
            input.set_value("https://matrix.org", window, cx)
        });
        let username =
            cx.new(|cx| InputState::new(window, cx).placeholder("@you:matrix.org"));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Password")
        });
        let recovery = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Recovery key")
        });
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Find a DM"));
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Write a message"));
        let microphone = cx.new(|cx| SelectState::new(Vec::<AudioDevice>::new(), None, window, cx));
        let speaker = cx.new(|cx| SelectState::new(Vec::<AudioDevice>::new(), None, window, cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &microphone,
                window,
                |this, _, event: &SelectEvent<Vec<AudioDevice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(id)) = event {
                        this.voice_command(voice::Command::Microphone(id.clone()), window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &speaker,
                window,
                |this, _, event: &SelectEvent<Vec<AudioDevice>>, window, cx| {
                    if let SelectEvent::Confirm(Some(id)) = event {
                        this.voice_command(voice::Command::Speaker(id.clone()), window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &composer,
                window,
                |this, _, event, window, cx| match event {
                    InputEvent::PressEnter { .. } => this.send(window, cx),
                    InputEvent::Change => {
                        let draft = this.composer.read(cx).value().to_string();
                        this.model.dispatch(Action::SetDraft(draft));
                        cx.notify();
                    }
                    _ => {}
                },
            ),
            cx.subscribe_in(&password, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.login(window, cx);
                }
            }),
            cx.subscribe_in(&recovery, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.recover(window, cx);
                }
            }),
            cx.subscribe_in(&filter, window, |_, _, _: &InputEvent, _, cx| cx.notify()),
        ];
        let mut app = Self {
            runtime,
            root,
            homeserver,
            username,
            password,
            recovery,
            filter,
            composer,
            model: Application::default(),
            audio_settings: false,
            microphone,
            speaker,
            list: ListState::new(0, ListAlignment::Bottom, px(320.)),
            backend: None,
            events: None,
            _subscriptions: subscriptions,
        };
        if preview {
            app.start_preview(window, cx);
        } else if app.root.join("active.json").exists() {
            app.start(Start::Restore, window, cx);
        } else {
            app.username.update(cx, |input, cx| input.focus(window, cx));
        }
        app
    }

    fn execute(&mut self, effects: Vec<Effect>, cx: &mut Context<Self>) {
        for effect in effects {
            match effect {
                Effect::Start(start) => {
                    let (backend, mut events) =
                        Backend::start(self.runtime.clone(), self.root.clone(), start);
                    self.backend = Some(backend);
                    self.events = Some(cx.spawn(async move |this, cx| {
                        while let Some(event) = events.recv().await {
                            if this
                                .update_in(cx, |this, window, cx| {
                                    this.event(event, window, cx);
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        let _ = this.update(cx, |this, cx| {
                            this.model.stopped();
                            this.backend = None;
                            cx.notify();
                        });
                    }));
                }
                Effect::Backend(command) => {
                    let result = match &self.backend {
                        Some(backend) => backend.send(command),
                        None => Err(command),
                    };
                    if let Err(command) = result {
                        self.model.backend_failed(&command);
                    }
                }
            }
        }
    }

    fn action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let before = PresentationState::capture(&self.model.state);
        let effects = self.model.dispatch(action);
        self.execute(effects, cx);
        self.update_presentation(before, window, cx);
        cx.notify();
    }

    fn event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let before = PresentationState::capture(&self.model.state);
        let effects = self.model.apply(event);
        self.execute(effects, cx);
        self.update_presentation(before, window, cx);
        cx.notify();
    }

    fn update_presentation(
        &mut self,
        before: PresentationState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = &self.model.state;
        let reset = before.reset_version != state.reset_version;
        let selection_changed = before.selection_version != state.selection_version;
        if reset || before.navigation != state.navigation {
            let placeholder = if state.navigation == Navigation::DirectMessages {
                "Find a DM"
            } else {
                "Find a channel"
            };
            self.filter.update(cx, |input, cx| {
                input.set_value("", window, cx);
                input.set_placeholder(placeholder, window, cx);
            });
        }
        if reset || selection_changed {
            self.list.reset(state.messages.len());
            self.list.scroll_to_end();
        } else if !Arc::ptr_eq(&before.messages, &state.messages) {
            let at_end = self.list.is_scrolled_to_end().unwrap_or(true);
            let prefix = before
                .messages
                .iter()
                .zip(state.messages.iter())
                .take_while(|(a, b)| a.id == b.id && a.body == b.body && a.author == b.author)
                .count();
            self.list
                .splice(prefix..before.messages.len(), state.messages.len() - prefix);
            if at_end {
                self.list.scroll_to_end();
            }
        }
        if reset || selection_changed || before.draft != state.draft() {
            let draft = state.draft().to_owned();
            let focus = selection_changed && state.selected_room().is_some_and(|room| !room.voice);
            self.composer.update(cx, |input, cx| {
                input.set_value(draft, window, cx);
                if focus {
                    input.focus(window, cx);
                }
            });
        }
        if before.voice.microphones != state.voice.microphones
            || before.voice.microphone != state.voice.microphone
        {
            self.microphone.update(cx, |select, cx| {
                select.set_items(
                    state
                        .voice
                        .microphones
                        .iter()
                        .map(|device| AudioDevice {
                            id: device.id.clone(),
                            name: device.name.clone().into(),
                        })
                        .collect(),
                    window,
                    cx,
                );
                if let Some(id) = &state.voice.microphone {
                    select.set_selected_value(id, window, cx);
                }
            });
        }
        if before.voice.speakers != state.voice.speakers
            || before.voice.speaker != state.voice.speaker
        {
            self.speaker.update(cx, |select, cx| {
                select.set_items(
                    state
                        .voice
                        .speakers
                        .iter()
                        .map(|device| AudioDevice {
                            id: device.id.clone(),
                            name: device.name.clone().into(),
                        })
                        .collect(),
                    window,
                    cx,
                );
                if let Some(id) = &state.voice.speaker {
                    select.set_selected_value(id, window, cx);
                }
            });
        }
        if reset {
            self.audio_settings = false;
            self.recovery
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.username
                .update(cx, |input, cx| input.focus(window, cx));
        }
    }

    fn start(&mut self, start: Start, window: &mut Window, cx: &mut Context<Self>) {
        match start {
            Start::Restore => self.action(Action::Restore, window, cx),
            Start::Login {
                homeserver,
                username,
                password,
            } => self.action(
                Action::Login {
                    homeserver,
                    username,
                    password,
                },
                window,
                cx,
            ),
        }
    }

    fn login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = Action::Login {
            homeserver: self.homeserver.read(cx).value().to_string(),
            username: self.username.read(cx).value().to_string(),
            password: self.password.read(cx).value().to_string(),
        };
        let was_busy = self.model.state.busy;
        self.action(action, window, cx);
        if !was_busy && self.model.state.busy {
            self.password
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn navigate(&mut self, navigation: Navigation, window: &mut Window, cx: &mut Context<Self>) {
        self.action(Action::Navigate(navigation), window, cx);
    }

    fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.action(Action::SelectRoom(id), window, cx);
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.model.state.messages.len();
        self.action(Action::Send, window, cx);
        if self.model.state.preview && self.model.state.messages.len() > count {
            self.list.scroll_to_end();
        }
    }

    fn recover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.recovery.read(cx).value().to_string();
        let was_busy = self.model.state.busy;
        self.action(Action::Recover(key), window, cx);
        if !was_busy && self.model.state.busy {
            self.recovery
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn start_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.action(Action::Preview, window, cx);
    }

    fn sign_in_view(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("sign-in-scroll")
            .flex_1()
            .h_full()
            .overflow_y_scroll()
            .child(self.sign_in_body(cx))
    }

    fn sign_in_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div().w_full().min_h_full().flex_shrink_0().v_flex().items_center().justify_center().p_6().gap_6()
            .child(div().w(px(380.)).v_flex().gap_2()
                .child(div().text_3xl().font_semibold().child("Talk"))
                .child(div().text_color(cx.theme().muted_foreground).child("Your people. Your Matrix account.")))
            .child(div().w(px(380.)).v_flex().gap_4()
                .child(field("Homeserver", Input::new(&self.homeserver).aria_label("Homeserver").disabled(self.model.state.busy)))
                .child(field("Matrix ID", Input::new(&self.username).aria_label("Matrix ID").disabled(self.model.state.busy)))
                .child(field("Password", Input::new(&self.password).aria_label("Password").mask_toggle().disabled(self.model.state.busy)))
                .child(Button::new("sign-in").primary().label(if self.model.state.busy { "Connecting..." } else { "Sign in" })
                    .disabled(self.model.state.busy).on_click(cx.listener(|this, _, window, cx| this.login(window, cx))))
                .when_some(self.model.state.notice.clone().map(SharedString::from), |view, notice| view.child(div().text_sm().child(notice)))
                .when(self.root.join("active.json").exists() && !self.model.state.busy, |view| view.child(Button::new("restore").label("Retry saved session").on_click(cx.listener(|this, _, window, cx| this.start(Start::Restore, window, cx)))))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Use an existing account. Password sign-in is supported in this build.")))
            .child(div().w(px(380.)).v_flex().gap_2().pt_4().border_t_1().border_color(cx.theme().border)
                .child(Button::new("preview").ghost().label("Explore local preview").disabled(self.model.state.busy)
                    .on_click(cx.listener(|this, _, window, cx| this.start_preview(window, cx))))
                .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Try the chat layout without an account. Nothing is sent.")))
    }

    fn voice_command(
        &mut self,
        command: voice::Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.action(Action::Voice(command), window, cx);
    }

    fn join_voice(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.action(Action::RequestJoinVoice(id), window, cx);
    }

    fn audio_devices_popover(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let microphone = self.microphone.clone();
        let speaker = self.speaker.clone();
        let preview = self.model.state.preview;
        let has_microphones = !self.model.state.voice.microphones.is_empty();
        let has_speakers = !self.model.state.voice.speakers.is_empty();
        let weak = cx.entity().downgrade();
        Popover::new("voice-device-popover")
            .anchor(Anchor::BottomLeft)
            .open(self.audio_settings)
            .on_open_change(cx.listener(|this, open, window, cx| {
                this.audio_settings = *open;
                if *open {
                    this.voice_command(voice::Command::Devices, window, cx);
                }
                cx.notify();
            }))
            .trigger(
                Button::new("voice-audio-settings")
                    .ghost()
                    .size(px(36.))
                    .icon(IconName::Settings)
                    .accessibility_label("Audio devices")
                    .tooltip("Audio devices"),
            )
            .content(move |_, _, cx| {
                let weak = weak.clone();
                div()
                    .w(px(264.))
                    .v_flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Audio devices"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Microphone"),
                    )
                    .child(
                        Select::new(&microphone)
                            .placeholder("System default")
                            .accessibility_label("Microphone device")
                            .disabled(preview || !has_microphones)
                            .w_full(),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Speakers"),
                    )
                    .child(
                        Select::new(&speaker)
                            .placeholder("System default")
                            .accessibility_label("Speaker device")
                            .disabled(preview || !has_speakers)
                            .w_full(),
                    )
                    .when(preview, |view| {
                        view.child(
                            div()
                                .text_xs()
                                .child("Device selection is available with a real account."),
                        )
                    })
                    .child(
                        Button::new("refresh-audio")
                            .ghost()
                            .label("Refresh devices")
                            .disabled(preview)
                            .on_click(cx.listener(move |_, _, window, cx| {
                                let _ = weak.update(cx, |this, cx| {
                                    this.voice_command(voice::Command::Devices, window, cx)
                                });
                            })),
                    )
            })
    }

    fn voice_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = matches!(
            self.model.state.voice.phase,
            Phase::Connected | Phase::Reconnecting
        );
        let muted = cx.theme().muted_foreground;
        let status = match self.model.state.voice.phase {
            Phase::Disconnected => "Voice disconnected",
            Phase::Joining => "Joining voice...",
            Phase::Connected if self.model.state.preview => "Preview voice",
            Phase::Connected => "Encrypted voice",
            Phase::Reconnecting => "Reconnecting voice...",
            Phase::Leaving => "Leaving voice...",
        };
        let mic_label = if self.model.state.voice.muted {
            "Unmute microphone"
        } else {
            "Mute microphone"
        };
        div()
            .flex_shrink_0()
            .p_3()
            .v_flex()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(div().text_sm().font_semibold().child(status))
            .when(self.model.state.voice.active(), |view| {
                view.child(
                    div()
                        .text_sm()
                        .truncate()
                        .child(SharedString::from(self.model.state.voice.name.clone())),
                )
                .child(div().text_xs().text_color(muted).child(
                    if self.model.state.voice.deafened {
                        "Deafened and muted"
                    } else if self.model.state.voice.muted {
                        "Microphone muted"
                    } else {
                        "Microphone live"
                    },
                ))
                .child(
                    div()
                        .h_flex()
                        .gap_1()
                        .child(
                            Button::new("voice-mute")
                                .ghost()
                                .size(px(36.))
                                .icon(if self.model.state.voice.muted {
                                    IconName::MicOff
                                } else {
                                    IconName::Mic
                                })
                                .accessibility_label(mic_label)
                                .tooltip(mic_label)
                                .selected(self.model.state.voice.muted)
                                .disabled(!ready || self.model.state.voice.deafened)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.voice_command(
                                        voice::Command::Mute(!this.model.state.voice.muted),
                                        window,
                                        cx,
                                    )
                                })),
                        )
                        .child(
                            Button::new("voice-deafen")
                                .ghost()
                                .size(px(36.))
                                .icon(IconName::Headphones)
                                .accessibility_label(if self.model.state.voice.deafened {
                                    "Undeafen"
                                } else {
                                    "Deafen"
                                })
                                .tooltip(if self.model.state.voice.deafened {
                                    "Undeafen"
                                } else {
                                    "Deafen"
                                })
                                .selected(self.model.state.voice.deafened)
                                .disabled(!ready)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.voice_command(
                                        voice::Command::Deafen(!this.model.state.voice.deafened),
                                        window,
                                        cx,
                                    )
                                })),
                        )
                        .child(self.audio_devices_popover(cx))
                        .child(
                            Button::new("voice-leave")
                                .ghost()
                                .label("Leave")
                                .disabled(self.model.state.voice.phase == Phase::Leaving)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.voice_command(voice::Command::Leave, window, cx);
                                })),
                        ),
                )
            })
            .when_some(self.model.state.voice.error.clone(), |view, error| {
                view.child(
                    div()
                        .id("voice-error")
                        .max_h(px(48.))
                        .overflow_y_scroll()
                        .text_sm()
                        .child(SharedString::from(error)),
                )
            })
            .when(!self.model.state.voice.active(), |view| {
                view.child(
                    Button::new("dismiss-voice")
                        .ghost()
                        .label("Dismiss")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.audio_settings = false;
                            this.action(Action::DismissVoiceError, window, cx);
                        })),
                )
            })
    }

    fn voice_view(&self, room: &ChatRoom, cx: &mut Context<Self>) -> impl IntoElement {
        let here = self.model.state.voice.room.as_ref() == Some(&room.id)
            && self.model.state.voice.active();
        let id = room.id.clone();
        let muted = cx.theme().muted_foreground;
        div()
            .id("voice-channel-scroll")
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .p_6()
            .v_flex()
            .gap_4()
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .child(SharedString::from(room.name.clone())),
            )
            .child(div().text_sm().text_color(muted).child(if here {
                if self.model.state.voice.participants.len() == 1 {
                    "You're here alone. Stay as long as you like; others can join later."
                } else {
                    "Voice stays connected while you browse other conversations."
                }
            } else {
                "Join muted and hang out. The channel stays here when empty."
            }))
            .when(!self.model.state.preview && !room.encrypted, |view| {
                view.child(div().text_sm().child(
                    "This channel is unencrypted. Talk only joins encrypted voice channels.",
                ))
            })
            .when(!here, |view| {
                view.child(
                    div().h_flex().child(
                        Button::new("join-voice")
                            .primary()
                            .label(if self.model.state.voice.active() {
                                "Switch voice channel"
                            } else {
                                "Join voice"
                            })
                            .disabled(!self.model.state.can_join_voice(&room.id))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.join_voice(id.clone(), window, cx)
                            })),
                    ),
                )
            })
            .when(here, |view| {
                view.child(
                    div()
                        .v_flex()
                        .gap_2()
                        .child(div().font_semibold().child("In voice"))
                        .children(self.model.state.voice.participants.iter().map(|person| {
                            div()
                                .h_flex()
                                .items_center()
                                .gap_3()
                                .py_2()
                                .child(if person.muted {
                                    IconName::MicOff
                                } else {
                                    IconName::Mic
                                })
                                .child(div().flex_1().min_w_0().truncate().child(
                                    SharedString::from(if person.local {
                                        "You".into()
                                    } else {
                                        person.user.clone()
                                    }),
                                ))
                                .child(div().text_sm().text_color(muted).child(if person.muted {
                                    "Muted"
                                } else if person.speaking {
                                    "Speaking"
                                } else {
                                    "Listening"
                                }))
                        })),
                )
            })
            .when(
                self.model.state.switch_voice.as_ref() == Some(&room.id),
                |view| {
                    view.child(
                        div()
                            .v_flex()
                            .gap_2()
                            .child(div().text_sm().child(SharedString::from(format!(
                                "Leave {} and join {} muted?",
                                self.model.state.voice.name, room.name
                            ))))
                            .child(
                                div()
                                    .h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("confirm-switch-voice")
                                            .primary()
                                            .label("Switch channel")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.action(Action::ConfirmVoiceSwitch, window, cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("cancel-switch-voice")
                                            .ghost()
                                            .label("Cancel")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.action(Action::CancelVoiceSwitch, window, cx);
                                            })),
                                    ),
                            ),
                    )
                },
            )
    }

    fn workspace(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.filter.read(cx).value().to_lowercase();
        let room = self
            .model
            .state
            .rooms
            .iter()
            .find(|room| Some(&room.id) == self.model.state.selected.as_ref())
            .cloned();
        let sidebar_title = match &self.model.state.navigation {
            Navigation::DirectMessages => "Direct messages".to_owned(),
            Navigation::Server(id) => self
                .model
                .state
                .servers
                .iter()
                .find(|server| &server.id == id)
                .map(|server| server.name.clone())
                .unwrap_or_else(|| "Server".into()),
        };
        let has_rooms = self
            .model
            .state
            .rooms
            .iter()
            .any(|room| self.model.state.in_scope(room));
        let read_only =
            !self.model.state.preview && room.as_ref().is_some_and(|room| !room.encrypted);
        let messages = self.model.state.messages.clone();
        let muted = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;
        let mut listed_voice = false;
        let mut listed_text = false;
        let room_buttons: Vec<_> = self
            .model
            .state
            .rooms
            .iter()
            .filter(|room| {
                self.model.state.in_scope(room) && room.name.to_lowercase().contains(&query)
            })
            .map(|room| {
                let id = room.id.clone();
                let heading = if self.model.state.navigation == Navigation::DirectMessages {
                    None
                } else if room.voice && !listed_voice {
                    listed_voice = true;
                    Some("Voice channels")
                } else if !room.voice && !listed_text {
                    listed_text = true;
                    Some("Text channels")
                } else {
                    None
                };
                div()
                    .v_flex()
                    .gap_1()
                    .when_some(heading, |view, label| {
                        view.child(
                            div()
                                .px_2()
                                .pt_3()
                                .pb_1()
                                .text_xs()
                                .text_color(muted)
                                .child(label),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("room-{id}")))
                            .ghost()
                            .w_full()
                            .accessibility_label(SharedString::from(room.name.clone()))
                            .child(
                                div()
                                    .w_full()
                                    .h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(if room.voice {
                                        IconName::Volume2
                                    } else if room.direct {
                                        IconName::MessageCircle
                                    } else {
                                        IconName::Hash
                                    })
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(SharedString::from(room.name.clone())),
                                    ),
                            )
                            .selected(Some(&room.id) == self.model.state.selected.as_ref())
                            .tooltip(SharedString::from(room.name.clone()))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(id.clone(), window, cx)
                            })),
                    )
            })
            .collect();
        div().h_flex().flex_1().min_w_0().h_full()
            .child(div().w(px(240.)).flex_shrink_0().h_full().v_flex().bg(rgb(0x141414)).border_r_1().border_color(border)
                .child(div().h(px(64.)).flex_shrink_0().px_4().h_flex().items_center().font_semibold().child(div().min_w_0().truncate().child(SharedString::from(sidebar_title))))
                .child(div().flex_shrink_0().px_3().pb_3().child(Input::new(&self.filter).aria_label(if self.model.state.navigation == Navigation::DirectMessages { "Find a DM" } else { "Find a channel" })))
                .child(div().id("rooms-scroll").flex_1().min_h(px(96.)).overflow_y_scroll().px_2().v_flex().gap_1()
                    .children(room_buttons)
                    .when(!has_rooms, |view| view.child(div().p_3().text_sm().text_color(muted).child(if self.model.state.navigation == Navigation::DirectMessages { "No direct messages yet. Existing DMs appear here after syncing." } else { "No joined channels in this server. Join a channel in another Matrix client to see it here." })))
                    .when(has_rooms && !self.model.state.rooms.iter().any(|room| self.model.state.in_scope(room) && room.name.to_lowercase().contains(&query)), |view| view.child(div().p_3().text_sm().text_color(muted).child("No matching conversations."))))
                .when(self.model.state.voice.active() || self.model.state.voice.error.is_some() || self.audio_settings, |view| view.child(self.voice_footer(cx)))
                .child(div().flex_shrink_0().p_3().v_flex().gap_2().border_t_1().border_color(border)
                    .child(div().text_sm().truncate().child(SharedString::from(self.model.state.account.clone().unwrap_or_else(|| "Local preview".into()))))
                    .when_some(self.model.state.storage.filter(|_| !self.model.state.voice.active()), |view, storage| view.child(div().text_xs().text_color(muted).child(storage)))
                    .child(div().text_xs().text_color(muted).child(if self.model.state.preview { "Messages stay here" } else if self.model.state.online { "Connected to Matrix" } else { "Sync paused or connecting" }))
                    .child(div().h_flex().gap_1().child(Button::new("exit").ghost().label(if self.model.state.preview { "Back to sign in" } else { "Sign out" }).disabled(self.model.state.busy || self.model.state.sending.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.action(Action::Logout, window, cx);
                        })))
                    .when(!self.model.state.preview && !self.model.state.online, |view| view.child(Button::new("retry-sync").ghost().label("Reconnect").disabled(self.model.state.busy).on_click(cx.listener(|this, _, window, cx| { this.action(Action::RetrySync, window, cx); })))))))
            .child(div().flex_1().min_w_0().h_full().v_flex()
                .child(div().h(px(64.)).flex_shrink_0().px_6().h_flex().items_center().gap_3().border_b_1().border_color(border)
                    .child(if room.as_ref().is_some_and(|r| r.voice) { IconName::Volume2 } else if room.as_ref().is_some_and(|r| r.direct) { IconName::MessageCircle } else { IconName::Hash })
                    .child(div().flex_1().min_w_0().font_semibold().truncate().child(SharedString::from(room.as_ref().map(|r| r.name.clone()).unwrap_or_else(|| "Choose a conversation".into()))))
                    .when_some(room.clone(), |view, room| view.child(div().text_sm().text_color(muted).child(format!("{} {}", room.members, if room.members == 1 { "member" } else { "members" })))))
                .when(self.model.state.preview, |view| view.child(div().px_6().py_2().text_sm().bg(rgb(0x202020)).child("Local preview. Messages and voice controls are samples; no audio devices are opened.")))
                .when(!self.model.state.preview && !self.model.state.trusted, |view| view.child(div().px_6().py_3().v_flex().gap_2().bg(rgb(0x202020))
                    .child(div().font_semibold().child("Verify this device to send messages"))
                    .child(div().text_sm().text_color(muted).child("Use your Matrix recovery key, or verify Talk in another client and check again."))
                    .child(div().h_flex().gap_2()
                        .child(div().flex_1().min_w_0().child(Input::new(&self.recovery).aria_label("Matrix recovery key").mask_toggle().disabled(self.model.state.busy)))
                        .child(Button::new("recover").label("Recover").disabled(self.model.state.busy || self.model.state.voice.active()).on_click(cx.listener(|this, _, window, cx| this.recover(window, cx))))
                        .child(Button::new("check-trust").ghost().label("Check again").disabled(self.model.state.busy).on_click(cx.listener(|this, _, window, cx| { this.action(Action::RefreshTrust, window, cx); }))))))
                .when_some(self.model.state.notice.clone().map(SharedString::from), |view, notice| view.child(div().px_6().py_2().text_sm().bg(rgb(0x262626)).child(notice)))
                .when(self.model.state.selected.is_some() && !self.model.state.preview && !room.as_ref().is_some_and(|r| r.voice), |view| view.child(div().px_6().py_2().h_flex()
                    .child(Button::new("earlier").ghost().label(if self.model.state.loading_history { "Loading history..." } else { "Load earlier messages" }).disabled(self.model.state.busy || !self.model.state.room_ready || self.model.state.loading_history)
                        .on_click(cx.listener(|this, _, window, cx| { this.action(Action::Earlier, window, cx); })))))
                .when(room.as_ref().is_some_and(|r| r.voice), |view| view.child(self.voice_view(room.as_ref().unwrap(), cx)))
                .when(!room.as_ref().is_some_and(|r| r.voice), |view| view
                .child(div().flex_1().min_h_0().min_w_0().v_flex()
                    .when(self.model.state.messages.is_empty(), |view| view.child(div().flex_1().v_flex().justify_center().items_center().p_6().gap_2()
                        .child(div().text_lg().font_semibold().child(if !self.model.state.room_ready && self.model.state.selected.is_some() { "Loading conversation..." } else if read_only { "No messages loaded" } else { "Room to talk" }))
                        .child(div().text_sm().text_color(muted).child(if self.model.state.selected.is_none() { "Select a conversation on the left." } else if read_only { "Load earlier history to read this room." } else { "No messages loaded. Start a conversation or load earlier history." }))))
                    .when(!self.model.state.messages.is_empty(), |view| view.child(list(self.list.clone(), move |index, _, _| render_message(&messages[index], foreground, muted).into_any_element()).flex_1().size_full())))
                .child(div().px_6().pt_3().pb_4().v_flex().gap_2()
                    .child(div().h_flex().gap_2()
                        .child(div().flex_1().min_w_0().child(Input::new(&self.composer).aria_label("Message").disabled(!self.model.state.room_ready || self.model.state.busy || read_only)))
                        .child(Button::new("send").primary().label(if self.model.state.sending.is_some() { "Sending..." } else { "Send" }).disabled(!self.model.state.can_send())
                            .on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))))
                    .child(div().text_xs().text_color(muted).child(if self.model.state.selected.is_none() { "Choose a DM or server channel to write a message." } else if self.model.state.preview { "Enter to send locally." } else if !self.model.state.trusted { "Sending is locked until this device is verified." } else if room.as_ref().is_some_and(|r| r.encrypted) { "End-to-end encrypted. Enter to send." } else { "Unencrypted room. Reading only in this build." })))))
    }
}

impl Render for TalkApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.model.state.preview || self.model.state.account.is_some();
        div()
            .size_full()
            .h_flex()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(
                div()
                    .w(px(64.))
                    .h_full()
                    .flex_shrink_0()
                    .v_flex()
                    .items_center()
                    .py_4()
                    .gap_3()
                    .bg(rgb(0x0e0e0e))
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .when(!workspace, |view| {
                        view.child(div().text_xl().font_semibold().child("T"))
                    })
                    .when(workspace, |view| {
                        view.child(
                            Button::new("dm-chats")
                                .ghost()
                                .size(px(40.))
                                .icon(IconName::MessageCircle)
                                .accessibility_label("Direct messages")
                                .tooltip("Direct messages")
                                .selected(self.model.state.navigation == Navigation::DirectMessages)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.navigate(Navigation::DirectMessages, window, cx)
                                })),
                        )
                        .child(div().w(px(32.)).h(px(1.)).bg(cx.theme().border))
                        .child(
                            div()
                                .id("servers-scroll")
                                .flex_1()
                                .min_h_0()
                                .w_full()
                                .overflow_y_scroll()
                                .v_flex()
                                .items_center()
                                .gap_3()
                                .children(self.model.state.servers.iter().map(|server| {
                                    let id = server.id.clone();
                                    Button::new(SharedString::from(format!("server-{id}")))
                                        .ghost()
                                        .size(px(40.))
                                        .label(
                                            server
                                                .name
                                                .chars()
                                                .next()
                                                .unwrap_or('?')
                                                .to_uppercase()
                                                .to_string(),
                                        )
                                        .accessibility_label(SharedString::from(format!(
                                            "Server: {}",
                                            server.name
                                        )))
                                        .tooltip(SharedString::from(server.name.clone()))
                                        .selected(
                                            self.model.state.navigation
                                                == Navigation::Server(id.clone()),
                                        )
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.navigate(
                                                Navigation::Server(id.clone()),
                                                window,
                                                cx,
                                            )
                                        }))
                                })),
                        )
                    }),
            )
            .child(if workspace {
                self.workspace(cx).into_any_element()
            } else {
                self.sign_in_view(cx).into_any_element()
            })
    }
}

fn field(label: &'static str, input: Input) -> impl IntoElement {
    div()
        .v_flex()
        .gap_2()
        .child(div().text_sm().child(label))
        .child(input)
}

fn render_message(
    message: &ChatMessage,
    foreground: gpui_kit::Hsla,
    muted: gpui_kit::Hsla,
) -> impl IntoElement {
    div()
        .w_full()
        .h_flex()
        .items_start()
        .gap_3()
        .px_6()
        .py_3()
        .min_w_0()
        .child(
            div()
                .size(px(32.))
                .flex_shrink_0()
                .rounded_md()
                .bg(rgb(if message.own { 0x363636 } else { 0x232323 }))
                .h_flex()
                .justify_center()
                .items_center()
                .text_sm()
                .font_semibold()
                .child(
                    message
                        .author
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .h_flex()
                        .gap_3()
                        .items_baseline()
                        .child(
                            div()
                                .font_semibold()
                                .truncate()
                                .text_color(foreground)
                                .child(SharedString::from(message.author.clone())),
                        )
                        .child(
                            div()
                                .text_xs()
                                .flex_shrink_0()
                                .text_color(muted)
                                .child(SharedString::from(message.time.clone())),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .text_color(foreground)
                        .child(SharedString::from(message.body.clone())),
                ),
        )
}
