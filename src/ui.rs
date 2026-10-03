//! Slint presentation and event-loop adapter for the shared application.
use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use talk_core::{
    application::{Action, Application, Effect, Navigation},
    backend::Backend,
    chat::{ChatMessage, Event},
    runtime::BackgroundRuntime,
    voice::{self, Phase},
};

slint::include_modules!();

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Renderer {
    #[default]
    Gpu,
    Software,
}

impl Renderer {
    fn name(self) -> &'static str {
        match self {
            Self::Gpu => "skia",
            Self::Software => "skia-software",
        }
    }
}

pub fn run(
    runtime: BackgroundRuntime,
    root: PathBuf,
    preview: bool,
    renderer: Renderer,
) -> Result<()> {
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.name().into())
        .select()
        .context("could not select the Slint desktop backend")?;
    let window = TalkWindow::new().context("could not create the Talk window")?;
    window
        .show()
        .context("could not show Talk; try --renderer software")?;
    // Only the private test compositor can request a different initial size.
    if std::env::var("TALK_FRAMEWORK_ISOLATED_DISPLAY").as_deref() == Ok("1")
        && let Ok(size) = std::env::var("TALK_UI_TEST_SIZE")
        && let Some((width, height)) = size.split_once('x')
        && let (Ok(width), Ok(height)) = (width.parse::<u32>(), height.parse::<u32>())
        && width >= 800
        && height >= 560
    {
        window
            .window()
            .set_size(slint::PhysicalSize::new(width, height));
    }
    let adapter = Adapter::new(Arc::new(runtime), root, &window)?;
    bind_callbacks(&window, &adapter);
    adapter.borrow_mut().refresh(&window);
    if preview {
        adapter.borrow_mut().action(&window, Action::Preview);
    } else if adapter.borrow().root.join("active.json").exists() {
        adapter.borrow_mut().action(&window, Action::Restore);
    } else {
        window.invoke_focus_sign_in();
    }
    tracing::info!(renderer = renderer.name(), "Slint desktop window opened");
    let result = window.run().context("Slint's desktop event loop failed");
    adapter.borrow_mut().stop_backend();
    result
}

#[derive(Default)]
struct Models {
    servers: Rc<VecModel<ServerRow>>,
    rooms: Rc<VecModel<RoomRow>>,
    messages: Rc<VecModel<MessageRow>>,
    participants: Rc<VecModel<PersonRow>>,
    microphones: Rc<VecModel<SharedString>>,
    speakers: Rc<VecModel<SharedString>>,
}

struct Icons {
    dm: slint::Image,
    text: slint::Image,
    voice: slint::Image,
}

struct Adapter {
    // Cancel the UI event receiver before releasing the actor and runtime.
    events: Option<slint::JoinHandle<()>>,
    backend: Option<Backend>,
    runtime: Arc<BackgroundRuntime>,
    root: PathBuf,
    app: Application,
    models: Models,
    icons: Icons,
    weak: std::rc::Weak<RefCell<Self>>,
    window: slint::Weak<TalkWindow>,
    navigation: Navigation,
    selection_version: u64,
    reset_version: u64,
    messages: Arc<Vec<ChatMessage>>,
}

impl Adapter {
    fn new(
        runtime: Arc<BackgroundRuntime>,
        root: PathBuf,
        window: &TalkWindow,
    ) -> Result<Rc<RefCell<Self>>> {
        let icons = Icons {
            dm: slint::Image::load_from_svg_data(include_bytes!("../ui/icons/message-circle.svg"))?,
            text: slint::Image::load_from_svg_data(include_bytes!("../ui/icons/hash.svg"))?,
            voice: slint::Image::load_from_svg_data(include_bytes!("../ui/icons/volume-2.svg"))?,
        };
        let models = Models::default();
        window.set_servers(ModelRc::from(models.servers.clone()));
        window.set_rooms(ModelRc::from(models.rooms.clone()));
        window.set_messages(ModelRc::from(models.messages.clone()));
        window.set_participants(ModelRc::from(models.participants.clone()));
        window.set_microphones(ModelRc::from(models.microphones.clone()));
        window.set_speakers(ModelRc::from(models.speakers.clone()));
        Ok(Rc::new_cyclic(|weak| {
            RefCell::new(Self {
                events: None,
                backend: None,
                runtime,
                root,
                app: Application::default(),
                models,
                icons,
                weak: weak.clone(),
                window: window.as_weak(),
                navigation: Navigation::default(),
                selection_version: 0,
                reset_version: 0,
                messages: Arc::new(Vec::new()),
            })
        }))
    }

    fn stop_backend(&mut self) {
        if let Some(events) = self.events.take() {
            events.abort();
        }
        self.backend = None;
    }

    fn execute(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Start(start) => {
                    self.stop_backend();
                    let (backend, mut events) =
                        Backend::start(self.runtime.clone(), self.root.clone(), start);
                    self.backend = Some(backend);
                    let weak = self.weak.clone();
                    let window = self.window.clone();
                    // Network work stays on Tokio. Only channel delivery runs on
                    // Slint's executor, which wakes on an event and never polls idle.
                    match slint::spawn_local(async move {
                        while let Some(event) = events.recv().await {
                            let (Some(adapter), Some(window)) = (weak.upgrade(), window.upgrade())
                            else {
                                return;
                            };
                            adapter.borrow_mut().event(&window, event);
                        }
                        if let (Some(adapter), Some(window)) = (weak.upgrade(), window.upgrade()) {
                            let mut adapter = adapter.borrow_mut();
                            adapter.app.stopped();
                            adapter.backend = None;
                            adapter.refresh(&window);
                        }
                    }) {
                        Ok(task) => self.events = Some(task),
                        Err(error) => {
                            tracing::error!(%error, "could not connect backend events to Slint");
                            self.backend = None;
                            self.app.stopped();
                        }
                    }
                }
                Effect::Backend(command) => {
                    let result = match &self.backend {
                        Some(backend) => backend.send(command),
                        None => Err(command),
                    };
                    if let Err(command) = result {
                        self.app.backend_failed(&command);
                    }
                }
            }
        }
    }

    fn action(&mut self, window: &TalkWindow, action: Action) {
        let effects = self.app.dispatch(action);
        self.execute(effects);
        self.refresh(window);
    }

    fn event(&mut self, window: &TalkWindow, event: Event) {
        let effects = self.app.apply(event);
        self.execute(effects);
        self.refresh(window);
    }

    fn refresh(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let reset = self.reset_version != state.reset_version;
        let selection_changed = self.selection_version != state.selection_version;
        let navigation_changed = self.navigation != state.navigation;
        if reset || navigation_changed {
            window.set_filter(SharedString::default());
        }
        if reset {
            window.set_password(SharedString::default());
            window.set_recovery(SharedString::default());
            window.set_audio_settings(false);
        }
        let messages_changed = !Arc::ptr_eq(&self.messages, &state.messages);
        let at_end = window.invoke_timeline_at_end();
        let old_extent = window.invoke_timeline_extent();
        let prepended = messages_changed
            && !selection_changed
            && self.messages.first().is_some_and(|first| {
                state.messages.first().is_some_and(|new| first.id != new.id)
                    && state.messages.iter().any(|message| message.id == first.id)
            });
        let query = window.get_filter().to_lowercase();
        let room = state.selected_room();
        let voice = &state.voice;
        let voice_here =
            room.is_some_and(|room| voice.room.as_ref() == Some(&room.id)) && voice.active();
        window.set_workspace(state.preview || state.account.is_some());
        window.set_preview(state.preview);
        window.set_busy(state.busy);
        window.set_trusted(state.trusted);
        window.set_online(state.online);
        window.set_restore_available(self.root.join("active.json").exists());
        window.set_dms_selected(state.navigation == Navigation::DirectMessages);
        window.set_sidebar_title(match &state.navigation {
            Navigation::DirectMessages => "Direct messages".into(),
            Navigation::Server(id) => state
                .servers
                .iter()
                .find(|server| &server.id == id)
                .map_or("Server".into(), |server| server.name.clone().into()),
        });
        window.set_filter_placeholder(
            if state.navigation == Navigation::DirectMessages {
                "Find a DM"
            } else {
                "Find a channel"
            }
            .into(),
        );
        update_model(
            &self.models.servers,
            state
                .servers
                .iter()
                .map(|server| ServerRow {
                    id: server.id.clone().into(),
                    name: server.name.clone().into(),
                    initial: initial(&server.name),
                    selected: state.navigation == Navigation::Server(server.id.clone()),
                })
                .collect(),
        );
        let mut listed_text = false;
        let mut listed_voice = false;
        let rows: Vec<_> = state
            .rooms
            .iter()
            .filter(|room| state.in_scope(room) && room.name.to_lowercase().contains(&query))
            .map(|room| {
                let heading = if state.navigation == Navigation::DirectMessages {
                    ""
                } else if room.voice && !listed_voice {
                    listed_voice = true;
                    "Voice channels"
                } else if !room.voice && !listed_text {
                    listed_text = true;
                    "Text channels"
                } else {
                    ""
                };
                RoomRow {
                    id: room.id.clone().into(),
                    name: room.name.clone().into(),
                    heading: heading.into(),
                    icon: if room.voice {
                        self.icons.voice.clone()
                    } else if room.direct {
                        self.icons.dm.clone()
                    } else {
                        self.icons.text.clone()
                    },
                    selected: Some(&room.id) == state.selected.as_ref(),
                }
            })
            .collect();
        window.set_sidebar_empty(if !rows.is_empty() { "" }
            else if state.rooms.iter().any(|room| state.in_scope(room)) { "No matching conversations." }
            else if state.navigation == Navigation::DirectMessages { "No direct messages yet. Existing DMs appear here after syncing." }
            else { "No joined channels in this server. Join a channel in another Matrix client to see it here." }.into());
        update_model(&self.models.rooms, rows);
        window.set_account(state.account.as_deref().unwrap_or("Local preview").into());
        window.set_account_status(
            if state.preview {
                "Messages stay here"
            } else if state.online {
                "Connected to Matrix"
            } else {
                "Sync paused or connecting"
            }
            .into(),
        );
        window.set_storage(state.storage.unwrap_or_default().into());
        window.set_notice(state.notice.as_deref().unwrap_or_default().into());
        window.set_room_selected(room.is_some());
        window.set_room_name(
            room.map_or("Choose a conversation", |room| room.name.as_str())
                .into(),
        );
        window.set_room_icon(if room.is_some_and(|room| room.voice) {
            self.icons.voice.clone()
        } else if room.is_some_and(|room| room.direct) {
            self.icons.dm.clone()
        } else {
            self.icons.text.clone()
        });
        window.set_members(room.map_or(SharedString::default(), |room| {
            format!(
                "{} {}",
                room.members,
                if room.members == 1 {
                    "member"
                } else {
                    "members"
                }
            )
            .into()
        }));
        window.set_voice_room(room.is_some_and(|room| room.voice));
        window.set_room_encrypted(room.is_some_and(|room| room.encrypted));
        window.set_room_ready(state.room_ready);
        window.set_draft(state.draft().into());
        window.set_composer_enabled(
            state.room_ready
                && !state.busy
                && room.is_some_and(|room| !room.voice && (state.preview || room.encrypted)),
        );
        window.set_can_send(state.can_send());
        window.set_sending(state.sending.is_some());
        window.set_loading_history(state.loading_history);
        window.set_composer_hint(
            if room.is_none() {
                "Choose a DM or server channel to write a message."
            } else if state.preview {
                "Enter to send locally."
            } else if !state.trusted {
                "Sending is locked until this device is verified."
            } else if room.is_some_and(|room| room.encrypted) {
                "End-to-end encrypted. Enter to send."
            } else {
                "Unencrypted room. Reading only in this build."
            }
            .into(),
        );
        if messages_changed || selection_changed || reset {
            let rows = state
                .messages
                .iter()
                .map(|message| MessageRow {
                    id: message.id.clone().into(),
                    author: message.author.clone().into(),
                    initial: initial(&message.author),
                    body: message.body.clone().into(),
                    time: message.time.clone().into(),
                    own: message.own,
                })
                .collect();
            update_model(&self.models.messages, rows);
            let weak = window.as_weak();
            if selection_changed || at_end {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = weak.upgrade() {
                        window.invoke_scroll_to_latest();
                    }
                });
            } else if prepended {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = weak.upgrade() {
                        window.invoke_preserve_history_position(
                            (window.invoke_timeline_extent() - old_extent).max(0.),
                        );
                    }
                });
            }
        }
        window.set_voice_active(voice.active());
        window.set_voice_visible(voice.active() || voice.error.is_some());
        window.set_voice_ready(matches!(
            voice.phase,
            Phase::Connected | Phase::Reconnecting
        ));
        window.set_voice_leaving(voice.phase == Phase::Leaving);
        window.set_voice_here(voice_here);
        window.set_can_join(room.is_some_and(|room| state.can_join_voice(&room.id)));
        window.set_muted(voice.muted);
        window.set_deafened(voice.deafened);
        window.set_voice_status(
            match voice.phase {
                Phase::Disconnected => "Voice disconnected",
                Phase::Joining => "Joining voice...",
                Phase::Connected if state.preview => "Preview voice",
                Phase::Connected => "Encrypted voice",
                Phase::Reconnecting => "Reconnecting voice...",
                Phase::Leaving => "Leaving voice...",
            }
            .into(),
        );
        window.set_voice_name(voice.name.clone().into());
        window.set_voice_microphone_status(
            if voice.deafened {
                "Deafened and muted"
            } else if voice.muted {
                "Microphone muted"
            } else {
                "Microphone live"
            }
            .into(),
        );
        window.set_voice_description(
            if voice_here && voice.participants.len() == 1 {
                "You're here alone. Stay as long as you like; others can join later."
            } else if voice_here {
                "Voice stays connected while you browse other conversations."
            } else {
                "Join muted and hang out. The channel stays here when empty."
            }
            .into(),
        );
        window.set_voice_error(voice.error.as_deref().unwrap_or_default().into());
        update_model(
            &self.models.participants,
            if voice_here {
                voice
                    .participants
                    .iter()
                    .map(|person| PersonRow {
                        name: if person.local {
                            "You".into()
                        } else {
                            person.user.clone().into()
                        },
                        status: if person.muted {
                            "Muted"
                        } else if person.speaking {
                            "Speaking"
                        } else {
                            "Listening"
                        }
                        .into(),
                        muted: person.muted,
                    })
                    .collect()
            } else {
                vec![]
            },
        );
        window.set_switch_prompt(
            if state.switch_voice.as_ref() == state.selected.as_ref()
                && state.switch_voice.is_some()
            {
                format!(
                    "Leave {} and join {} muted?",
                    voice.name,
                    room.map_or("voice", |room| room.name.as_str())
                )
                .into()
            } else {
                SharedString::default()
            },
        );
        update_model(
            &self.models.microphones,
            voice
                .microphones
                .iter()
                .map(|device| device.name.clone().into())
                .collect(),
        );
        update_model(
            &self.models.speakers,
            voice
                .speakers
                .iter()
                .map(|device| device.name.clone().into())
                .collect(),
        );
        window.set_microphone_index(device_index(
            &voice.microphones,
            voice.microphone.as_deref(),
        ));
        window.set_speaker_index(device_index(&voice.speakers, voice.speaker.as_deref()));
        if selection_changed && room.is_some_and(|room| !room.voice) {
            window.invoke_focus_composer();
        } else if reset && !state.preview && state.account.is_none() {
            window.invoke_focus_sign_in();
        }
        self.navigation = state.navigation.clone();
        self.selection_version = state.selection_version;
        self.reset_version = state.reset_version;
        self.messages = state.messages.clone();
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.stop_backend();
    }
}

fn bind_callbacks(window: &TalkWindow, adapter: &Rc<RefCell<Adapter>>) {
    macro_rules! bind {
        ($callback:ident, $action:expr) => {{
            let adapter = adapter.clone();
            let weak = window.as_weak();
            window.$callback(move || {
                if let Some(window) = weak.upgrade() {
                    adapter.borrow_mut().action(&window, $action);
                }
            });
        }};
    }
    bind!(on_explore_preview, Action::Preview);
    bind!(on_restore, Action::Restore);
    bind!(on_logout, Action::Logout);
    bind!(on_refresh_trust, Action::RefreshTrust);
    bind!(on_retry_sync, Action::RetrySync);
    bind!(
        on_navigate_dms,
        Action::Navigate(Navigation::DirectMessages)
    );
    bind!(on_send, Action::Send);
    bind!(on_earlier, Action::Earlier);
    bind!(on_leave_voice, Action::Voice(voice::Command::Leave));
    bind!(on_confirm_switch, Action::ConfirmVoiceSwitch);
    bind!(on_cancel_switch, Action::CancelVoiceSwitch);
    bind!(on_dismiss_voice_error, Action::DismissVoiceError);
    bind!(on_refresh_devices, Action::Voice(voice::Command::Devices));
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_login(move || {
        if let Some(window) = weak.upgrade() {
            let mut state = state.borrow_mut();
            let busy = state.app.state.busy;
            state.action(
                &window,
                Action::Login {
                    homeserver: window.get_homeserver().into(),
                    username: window.get_username().into(),
                    password: window.get_password().into(),
                },
            );
            if !busy && state.app.state.busy {
                window.set_password(SharedString::default());
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_recover(move || {
        if let Some(window) = weak.upgrade() {
            let mut state = state.borrow_mut();
            let busy = state.app.state.busy;
            state.action(&window, Action::Recover(window.get_recovery().into()));
            if !busy && state.app.state.busy {
                window.set_recovery(SharedString::default());
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_navigate_server(move |id| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Navigate(Navigation::Server(id.into())));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_select_room(move |id| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::SelectRoom(id.into()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_draft_edited(move |text| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::SetDraft(text.into()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_filter_edited(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().refresh(&window);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_join_voice(move || {
        if let Some(window) = weak.upgrade() {
            let id = state.borrow().app.state.selected.clone();
            if let Some(id) = id {
                state
                    .borrow_mut()
                    .action(&window, Action::RequestJoinVoice(id));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_mute(move || {
        if let Some(window) = weak.upgrade() {
            let muted = state.borrow().app.state.voice.muted;
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::Mute(!muted)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_toggle_deafen(move || {
        if let Some(window) = weak.upgrade() {
            let deafened = state.borrow().app.state.voice.deafened;
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::Deafen(!deafened)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_select_microphone(move |index| {
        if let Some(window) = weak.upgrade() {
            let id = usize::try_from(index).ok().and_then(|index| {
                state
                    .borrow()
                    .app
                    .state
                    .voice
                    .microphones
                    .get(index)
                    .map(|device| device.id.clone())
            });
            if let Some(id) = id {
                state
                    .borrow_mut()
                    .action(&window, Action::Voice(voice::Command::Microphone(id)));
            }
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_select_speaker(move |index| {
        if let Some(window) = weak.upgrade() {
            let id = usize::try_from(index).ok().and_then(|index| {
                state
                    .borrow()
                    .app
                    .state
                    .voice
                    .speakers
                    .get(index)
                    .map(|device| device.id.clone())
            });
            if let Some(id) = id {
                state
                    .borrow_mut()
                    .action(&window, Action::Voice(voice::Command::Speaker(id)));
            }
        }
    });
}

fn initial(text: &str) -> SharedString {
    text.chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string()
        .into()
}

fn device_index(devices: &[voice::Device], selected: Option<&str>) -> i32 {
    devices
        .iter()
        .position(|device| Some(device.id.as_str()) == selected)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(-1)
}

fn update_model<T: Clone + PartialEq + 'static>(model: &VecModel<T>, rows: Vec<T>) {
    let common = model.row_count().min(rows.len());
    for (index, row) in rows.iter().take(common).enumerate() {
        if model.row_data(index).as_ref() != Some(row) {
            model.set_row_data(index, row.clone());
        }
    }
    while model.row_count() > rows.len() {
        model.remove(model.row_count() - 1);
    }
    for row in rows.into_iter().skip(common) {
        model.push(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_input_reaches_navigation_buttons() {
        i_slint_backend_testing::init_no_event_loop();
        let directory = tempfile::tempdir().unwrap();
        let window = TalkWindow::new().unwrap();
        let adapter = Adapter::new(
            Arc::new(BackgroundRuntime::new().unwrap()),
            directory.path().into(),
            &window,
        )
        .unwrap();
        bind_callbacks(&window, &adapter);
        window.invoke_explore_preview();
        window.show().unwrap();
        window
            .window()
            .set_size(slint::LogicalSize::new(1120., 760.));
        let click = |x, y| {
            let position = slint::LogicalPosition::new(x, y);
            let button = slint::platform::PointerEventButton::Left;
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerPressed { position, button });
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerReleased { position, button });
        };
        click(32., 36.);
        assert!(window.get_dms_selected());
        assert_eq!(window.get_sidebar_title(), "Direct messages");
        click(32., 101.);
        assert!(!window.get_dms_selected());
        assert_eq!(window.get_sidebar_title(), "Friends");
        window.hide().unwrap();
    }

    #[test]
    fn callbacks_preserve_drafts_scope_voice_and_clear_session_inputs() {
        i_slint_backend_testing::init_no_event_loop();
        let directory = tempfile::tempdir().unwrap();
        let window = TalkWindow::new().unwrap();
        let adapter = Adapter::new(
            Arc::new(BackgroundRuntime::new().unwrap()),
            directory.path().into(),
            &window,
        )
        .unwrap();
        bind_callbacks(&window, &adapter);
        window.invoke_explore_preview();
        assert!(window.get_workspace() && window.get_preview());
        let server = adapter.borrow().app.state.servers[0].id.clone();
        window.invoke_navigate_server(server.clone().into());
        let rooms = adapter
            .borrow()
            .app
            .state
            .rooms
            .iter()
            .filter(|room| adapter.borrow().app.state.in_scope(room))
            .cloned()
            .collect::<Vec<_>>();
        let text_rooms = rooms.iter().filter(|room| !room.voice).collect::<Vec<_>>();
        window.invoke_select_room(text_rooms[0].id.clone().into());
        window.invoke_draft_edited("Draft survives navigation".into());
        window.invoke_select_room(text_rooms[1].id.clone().into());
        window.invoke_draft_edited("Different channel draft".into());
        window.invoke_select_room(text_rooms[0].id.clone().into());
        assert_eq!(window.get_draft(), "Draft survives navigation");
        assert!(window.get_can_send());
        let count = window.get_messages().row_count();
        window.invoke_send();
        assert_eq!(window.get_messages().row_count(), count + 1);
        assert!(window.get_draft().is_empty());
        let voice_room = rooms.iter().find(|room| room.voice).unwrap();
        window.invoke_select_room(voice_room.id.clone().into());
        window.invoke_join_voice();
        assert!(window.get_voice_here() && window.get_voice_active() && window.get_muted());
        window.invoke_navigate_dms();
        assert!(window.get_voice_active() && window.get_voice_visible());
        assert!(!window.get_voice_here());
        assert!(window.get_rooms().iter().all(|row| {
            adapter
                .borrow()
                .app
                .state
                .rooms
                .iter()
                .any(|room| room.id.as_str() == row.id.as_str() && room.direct)
        }));
        window.invoke_toggle_mute();
        assert!(!window.get_muted());
        window.invoke_toggle_deafen();
        assert!(window.get_muted() && window.get_deafened());
        window.set_password("secret that must be cleared".into());
        window.set_recovery("recovery that must be cleared".into());
        window.set_audio_settings(true);
        window.invoke_logout();
        assert!(!window.get_workspace() && !window.get_voice_active());
        assert!(window.get_password().is_empty() && window.get_recovery().is_empty());
        assert!(!window.get_audio_settings());
    }

    #[test]
    fn signed_in_controls_keep_trust_and_plaintext_sending_gates() {
        i_slint_backend_testing::init_no_event_loop();
        let directory = tempfile::tempdir().unwrap();
        let window = TalkWindow::new().unwrap();
        let adapter = Adapter::new(
            Arc::new(BackgroundRuntime::new().unwrap()),
            directory.path().into(),
            &window,
        )
        .unwrap();
        bind_callbacks(&window, &adapter);
        window.invoke_explore_preview();
        {
            let mut adapter = adapter.borrow_mut();
            adapter.app.state.preview = false;
            adapter.app.state.account = Some("@test:example.invalid".into());
            adapter.app.state.trusted = false;
            adapter.refresh(&window);
        }
        window.invoke_draft_edited("Keep encrypted".into());
        assert!(!window.get_can_send());
        window.invoke_send();
        assert!(!window.get_sending());
        {
            let mut adapter = adapter.borrow_mut();
            adapter.app.state.trusted = true;
            let id = adapter.app.state.selected.clone().unwrap();
            adapter
                .app
                .state
                .rooms
                .iter_mut()
                .find(|room| room.id == id)
                .unwrap()
                .encrypted = false;
            adapter.refresh(&window);
        }
        assert!(!window.get_composer_enabled() && !window.get_can_send());
        window.invoke_send();
        assert_eq!(window.get_draft(), "Keep encrypted");
        assert!(!window.get_sending());
    }
}
