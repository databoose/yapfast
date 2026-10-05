//! Slint presentation and event-loop adapter for the shared application.
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use slint::{
    ComponentHandle, Model, ModelRc, SharedString, VecModel, winit_030::WinitWindowAccessor,
};
use talk_core::{
    application::{Action, Application, Compose, Effect, Navigation},
    backend::Backend,
    chat::{ChatMessage, Delivery, Event, Picture},
    runtime::BackgroundRuntime,
    voice::{self, Phase},
};

use crate::graphics;
use talk_ui::*;

/// How the desktop window ended.
pub enum Exit {
    Closed,
    /// GPU rendering failed; restart with Skia Software for this reason.
    UseSoftware(String),
}

/// Slint releases the first window after at most 500 ms of desktop appearance
/// queries; this leaves time for the window, its GPU surface and first frames.
/// Skia disables partial repainting while a rendering notifier is installed,
/// so startup is confirmed by the native window rather than by a frame.
const GPU_START_CONFIRMATION: Duration = Duration::from_secs(3);

pub fn run(
    runtime: BackgroundRuntime,
    root: PathBuf,
    preview: bool,
    plan: graphics::Plan,
) -> Result<Exit> {
    // Request OpenGL explicitly so Slint cannot silently replace a failed GPU
    // surface; Talk owns the fallback and tells the user about it.
    let renderer = match plan.renderer {
        graphics::Renderer::Gpu => "skia-opengl",
        graphics::Renderer::Software => "skia-software",
    };
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        .select()
        .context("could not select the Slint desktop backend")?;
    let gpu = plan.renderer == graphics::Renderer::Gpu;
    let failed = |error: slint::PlatformError, context: &'static str| -> Result<Exit> {
        let reason = error.to_string();
        if gpu {
            tracing::error!(%reason, "GPU rendering failed; restarting with Skia Software");
            Ok(Exit::UseSoftware(reason))
        } else {
            Err(anyhow::Error::new(error).context(context))
        }
    };
    let window = match TalkWindow::new() {
        Ok(window) => window,
        Err(error) => return failed(error, "could not create the Talk window"),
    };
    if let Err(error) = window.show() {
        return failed(error, "could not show Talk");
    }
    prepare(&window);
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
    let notice = SharedString::from(plan.notice.unwrap_or_default());
    window.set_graphics_notice(notice.clone());
    // Slint 1.18 keeps its event loop running when it creates a window late and
    // the GPU surface fails, leaving no window. Check for the native window.
    let missing_window = Rc::new(Cell::new(false));
    if gpu {
        let missing_window = missing_window.clone();
        let weak = window.as_weak();
        slint::Timer::single_shot(GPU_START_CONFIRMATION, move || {
            if !weak
                .upgrade()
                .is_some_and(|window| window.window().has_winit_window())
            {
                missing_window.set(true);
                let _ = slint::quit_event_loop();
            }
        });
    }
    tracing::info!(renderer, "starting the Slint desktop window");
    let result = match test_control() {
        Some(path) => {
            let _control = control(path, window, adapter.clone(), notice);
            slint::run_event_loop_until_quit()
        }
        None => window.run(),
    };
    adapter.borrow_mut().stop_backend();
    match result {
        Err(error) => failed(error, "Slint's desktop event loop failed"),
        Ok(()) if missing_window.get() => failed(
            "the window did not appear".into(),
            "Talk's window did not appear",
        ),
        Ok(()) => Ok(Exit::Closed),
    }
}

/// Only the private test compositor can request a different initial size.
fn isolated_display() -> bool {
    std::env::var("TALK_FRAMEWORK_ISOLATED_DISPLAY").as_deref() == Ok("1")
}

fn prepare(window: &TalkWindow) {
    if isolated_display()
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
    window.set_emoji_font(
        if cfg!(windows) {
            "Segoe UI Emoji"
        } else if cfg!(target_os = "macos") {
            "Apple Color Emoji"
        } else {
            "Noto Color Emoji"
        }
        .into(),
    );
}

/// A command file through which the private test compositor can hide, destroy
/// and recreate the window while Talk keeps running, as a tray would.
fn test_control() -> Option<PathBuf> {
    isolated_display()
        .then(|| std::env::var_os("TALK_UI_TEST_CONTROL"))
        .flatten()
        .map(PathBuf::from)
}

/// Run each new line of the command file. The event loop then runs until quit,
/// so a hidden or destroyed window does not end Talk.
fn control(
    path: PathBuf,
    window: TalkWindow,
    adapter: Rc<RefCell<Adapter>>,
    notice: SharedString,
) -> slint::Timer {
    let window = RefCell::new(Some(window));
    let handled = Cell::new(0);
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(500),
        move || {
            let Ok(text) = std::fs::read_to_string(&path) else {
                return;
            };
            for command in text.lines().skip(handled.get()) {
                handled.set(handled.get() + 1);
                tracing::info!(command, "test window control");
                let result = match command {
                    "hide" => window
                        .borrow()
                        .as_ref()
                        .map_or(Ok(()), |window| window.hide()),
                    "show" => window
                        .borrow()
                        .as_ref()
                        .map_or(Ok(()), |window| window.show()),
                    // A shown window keeps itself alive; hide it, then drop the last handle.
                    "destroy" => window.take().map_or(Ok(()), |window| {
                        let weak = window.as_weak();
                        let result = window.hide();
                        drop(window);
                        tracing::info!(
                            released = weak.upgrade().is_none(),
                            "test window destroyed"
                        );
                        result
                    }),
                    "create" if window.borrow().is_none() => {
                        recreate(&adapter, &notice).map(|new| *window.borrow_mut() = Some(new))
                    }
                    "trim" => {
                        trim();
                        Ok(())
                    }
                    "stats" => {
                        allocator_stats();
                        Ok(())
                    }
                    "quit" => slint::quit_event_loop().map_err(|error| error.to_string().into()),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    tracing::error!(command, %error, "test window control failed");
                }
            }
        },
    );
    timer
}

fn recreate(
    adapter: &Rc<RefCell<Adapter>>,
    notice: &SharedString,
) -> Result<TalkWindow, slint::PlatformError> {
    let window = TalkWindow::new()?;
    window.show()?;
    prepare(&window);
    window.set_graphics_notice(notice.clone());
    adapter.borrow_mut().attach(&window);
    bind_callbacks(&window, adapter);
    adapter.borrow_mut().refresh(&window);
    Ok(window)
}

/// Return freed heap pages to the system, to separate memory still in use from
/// memory glibc keeps for reuse.
fn trim() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: malloc_trim only releases free memory held by the allocator.
        unsafe { malloc_trim(0) };
    }
}

/// Print the allocator's per-arena totals to standard error: memory held from
/// the system against memory in use.
fn allocator_stats() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_stats();
        }
        // SAFETY: malloc_stats only reads allocator state and writes to stderr.
        unsafe { malloc_stats() };
    }
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

impl Models {
    fn attach(&self, window: &TalkWindow) {
        window.set_servers(ModelRc::from(self.servers.clone()));
        window.set_rooms(ModelRc::from(self.rooms.clone()));
        window.set_messages(ModelRc::from(self.messages.clone()));
        window.set_participants(ModelRc::from(self.participants.clone()));
        window.set_microphones(ModelRc::from(self.microphones.clone()));
        window.set_speakers(ModelRc::from(self.speakers.clone()));
    }
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
    /// Rows already built for unchanged messages, so the list keeps their state.
    rows: HashMap<String, (ChatMessage, MessageRow)>,
    /// One open or save dialog at a time; dropping it cancels the dialog's task.
    dialog: Option<slint::JoinHandle<()>>,
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
        models.attach(window);
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
                rows: HashMap::new(),
                dialog: None,
            })
        }))
    }

    /// Present the existing models and state in a new window.
    fn attach(&mut self, window: &TalkWindow) {
        self.models.attach(window);
        self.window = window.as_weak();
    }

    /// Run a native file dialog on Slint's executor, replacing any open one.
    fn dialog(&mut self, task: impl std::future::Future<Output = ()> + 'static) {
        if let Some(previous) = self.dialog.take() {
            previous.abort();
        }
        match slint::spawn_local(task) {
            Ok(handle) => self.dialog = Some(handle),
            Err(error) => {
                tracing::error!(%error, "could not open the file dialog");
                self.app.state.notice = Some("Talk could not open a file dialog.".into());
            }
        }
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
                    // Network work stays on Tokio. Only channel delivery runs on
                    // Slint's executor, which wakes on an event and never polls idle.
                    // Events still apply without a window, so the call and chat go on.
                    match slint::spawn_local(async move {
                        while let Some(event) = events.recv().await {
                            let Some(adapter) = weak.upgrade() else {
                                return;
                            };
                            let window = adapter.borrow().window.upgrade();
                            adapter.borrow_mut().event(window.as_ref(), event);
                        }
                        if let Some(adapter) = weak.upgrade() {
                            let window = adapter.borrow().window.upgrade();
                            let mut adapter = adapter.borrow_mut();
                            adapter.app.stopped();
                            adapter.backend = None;
                            if let Some(window) = window {
                                adapter.refresh(&window);
                            }
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

    fn event(&mut self, window: Option<&TalkWindow>, event: Event) {
        let effects = self.app.apply(event);
        self.execute(effects);
        if let Some(window) = window {
            self.refresh(window);
        }
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
        window.set_dms_unread(
            state
                .rooms
                .iter()
                .any(|room| room.direct && room.unread > 0),
        );
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
                    initial: initials(&server.name),
                    selected: state.navigation == Navigation::Server(server.id.clone()),
                    unread: state.rooms.iter().any(|room| {
                        room.unread > 0 && !room.direct && server.channels.contains(&room.id)
                    }),
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
                    "VOICE CHANNELS"
                } else if !room.voice && !listed_text {
                    listed_text = true;
                    "TEXT CHANNELS"
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
                    unread: room.unread > 0,
                    badge: mention_badge(room),
                    voice: room.voice,
                    direct: room.direct,
                    initial: initial(&room.name),
                    tint: tint(&room.id),
                }
            })
            .collect();
        window.set_sidebar_empty(if !rows.is_empty() { "" }
            else if state.rooms.iter().any(|room| state.in_scope(room)) { "No matching conversations." }
            else if state.navigation == Navigation::DirectMessages { "No direct messages yet. Existing DMs appear here after syncing." }
            else { "No joined channels in this server. Join a channel in another Matrix client to see it here." }.into());
        update_model(&self.models.rooms, rows);
        let account = state.account.as_deref().unwrap_or("Preview");
        window.set_account(account.into());
        window.set_account_name(display_part(account).into());
        window.set_account_initial(initial(display_part(account)));
        window.set_account_tint(tint(display_part(account)));
        window.set_account_status(
            if state.preview {
                "Local only"
            } else if state.online {
                "Online"
            } else {
                "Offline, sync paused"
            }
            .into(),
        );
        window.set_storage(state.storage.unwrap_or_default().into());
        window.set_notice(state.notice.as_deref().unwrap_or_default().into());
        window.set_room_selected(room.is_some());
        window.set_room_name(room.map_or("", |room| room.name.as_str()).into());
        window.set_room_direct(room.is_some_and(|room| room.direct));
        window.set_room_initial(room.map_or(SharedString::default(), |room| initial(&room.name)));
        window.set_room_tint(room.map_or(slint::Color::default(), |room| tint(&room.id)));
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
        window.set_can_attach(state.can_attach());
        window.set_attaching(state.attaching);
        window.set_confirm_delete(state.confirm_delete.clone().unwrap_or_default().into());
        window.set_compose_label(match state.compose() {
            Some(Compose::Reply { author, body, .. }) => {
                format!("Replying to {author}: {body}").into()
            }
            Some(Compose::Edit { .. }) => "Editing your message".into(),
            None => SharedString::default(),
        });
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
            if selection_changed || reset {
                window.set_react_item(SharedString::default());
                window.set_active_item(SharedString::default());
            }
            let mut previous = std::mem::take(&mut self.rows);
            let mut before: Option<&ChatMessage> = None;
            let rows = state
                .messages
                .iter()
                .map(|message| {
                    let mut row = match previous.remove(&message.id) {
                        Some((old, row)) if &old == message => row,
                        _ => message_row(message),
                    };
                    self.rows
                        .insert(message.id.clone(), (message.clone(), row.clone()));
                    let day = day(message.timestamp);
                    let new_day = !day.is_empty()
                        && before.is_none_or(|before| self::day(before.timestamp) != day);
                    row.first = new_day || before.is_none_or(|before| !continues(before, message));
                    row.day = if new_day {
                        day.into()
                    } else {
                        SharedString::default()
                    };
                    before = Some(message);
                    row
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
        window.set_voice_channel(
            voice
                .room
                .as_deref()
                .filter(|_| voice.active())
                .unwrap_or_default()
                .into(),
        );
        window.set_voice_tone(match voice.phase {
            Phase::Connected => 1,
            Phase::Disconnected => 2,
            _ => 0,
        });
        window.set_voice_detail(
            if state.preview {
                format!("{} · Sample", voice.name)
            } else {
                format!("{} · End-to-end encrypted", voice.name)
            }
            .into(),
        );
        window.set_can_join(room.is_some_and(|room| state.can_join_voice(&room.id)));
        window.set_muted(voice.muted);
        window.set_deafened(voice.deafened);
        window.set_voice_status(
            match voice.phase {
                Phase::Disconnected => "Voice disconnected",
                Phase::Joining => "Joining voice...",
                Phase::Connected if state.preview => "Preview voice",
                Phase::Connected => "Voice connected",
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
        window.set_voice_warning(voice.warning.as_deref().unwrap_or_default().into());
        update_model(
            &self.models.participants,
            if voice.active() {
                voice
                    .participants
                    .iter()
                    .map(|person| PersonRow {
                        name: if person.local {
                            "You".into()
                        } else {
                            display_part(&person.user).into()
                        },
                        initial: initial(if person.local {
                            display_part(account)
                        } else {
                            display_part(&person.user)
                        }),
                        tint: tint(display_part(if person.local {
                            account
                        } else {
                            &person.user
                        })),
                        speaking: person.speaking && !person.muted && person.connected,
                        connected: person.connected,
                        status: if let Some(problem) = &person.problem {
                            problem.as_str()
                        } else if !person.connected {
                            "Connection lost"
                        } else if person.muted {
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
                .map(|device| device_label(&device.name))
                .collect(),
        );
        update_model(
            &self.models.speakers,
            voice
                .speakers
                .iter()
                .map(|device| device_label(&device.name))
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
    bind!(on_confirm_delete_message, Action::ConfirmDelete);
    bind!(on_cancel_delete, Action::CancelDelete);
    bind!(on_cancel_compose, Action::CancelCompose);
    bind!(on_dismiss_notice, Action::DismissNotice);
    macro_rules! bind_item {
        ($callback:ident, $action:expr) => {{
            let adapter = adapter.clone();
            let weak = window.as_weak();
            window.$callback(move |item| {
                if let Some(window) = weak.upgrade() {
                    adapter.borrow_mut().action(&window, $action(item.into()));
                }
            });
        }};
    }
    bind_item!(on_reply, Action::Reply);
    bind_item!(on_edit, Action::Edit);
    bind_item!(on_request_delete, Action::RequestDelete);
    bind_item!(on_retry, Action::Retry);
    bind_item!(on_cancel_send, Action::CancelSend);
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_react(move |item, key| {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().action(
                &window,
                Action::React {
                    item: item.into(),
                    key: key.into(),
                },
            );
        }
    });
    let state = adapter.clone();
    window.on_attach(move || {
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            let file = rfd::AsyncFileDialog::new()
                .set_title("Attach a file")
                .pick_file()
                .await;
            if let (Some(file), Some(adapter)) = (file, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    adapter
                        .borrow_mut()
                        .action(&window, Action::Attach(file.path().to_owned()));
                }
            }
        });
    });
    let state = adapter.clone();
    window.on_save_attachment(move |item| {
        let item = item.to_string();
        let name = state
            .borrow()
            .app
            .state
            .message(&item)
            .and_then(|message| message.attachment.as_ref())
            .map(|attachment| attachment.name.clone())
            .unwrap_or_default();
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            // Only the file name is suggested; a sender cannot choose the folder.
            let name = std::path::Path::new(&name)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "attachment".into());
            let file = rfd::AsyncFileDialog::new()
                .set_title("Save attachment")
                .set_file_name(name)
                .save_file()
                .await;
            if let (Some(file), Some(adapter)) = (file, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    adapter.borrow_mut().action(
                        &window,
                        Action::SaveAttachment {
                            item,
                            path: file.path().to_owned(),
                        },
                    );
                }
            }
        });
    });
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

/// Only mentions get a number; plain unread activity is a dot, because sync
/// does not deliver every message of a room that isn't open.
fn mention_badge(room: &talk_core::chat::ChatRoom) -> SharedString {
    match room.mentions {
        0 => SharedString::default(),
        count if count > 99 => "99+".into(),
        count => count.to_string().into(),
    }
}

/// Avatar tints: mid tones that keep a white initial legible.
const TINTS: [(u8, u8, u8); 8] = [
    (0xc2, 0x55, 0x4d),
    (0xb8, 0x6e, 0x22),
    (0x3a, 0x93, 0x57),
    (0x2b, 0x8a, 0x98),
    (0x4a, 0x74, 0xc9),
    (0x7a, 0x5f, 0xc9),
    (0xb3, 0x4f, 0x8c),
    (0x64, 0x6b, 0x7a),
];

/// The same name always gets the same tint. People are keyed by display name,
/// which is all a reply quote carries, so a quote matches its author's avatar.
fn tint(key: &str) -> slint::Color {
    let hash = key
        .to_lowercase()
        .bytes()
        .fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
    // FNV-1a mixes its high bits far better than its low ones.
    let (red, green, blue) = TINTS[(hash >> 20) as usize % TINTS.len()];
    slint::Color::from_rgb_u8(red, green, blue)
}

/// The readable part of a Matrix ID: `@alice:example.org` becomes `alice`.
fn display_part(name: &str) -> &str {
    name.strip_prefix('@')
        .and_then(|rest| rest.split_once(':'))
        .map_or(name, |(local, _)| local)
}

/// A server icon shows the first letter of up to three words, as in "Co-op crew" → "Cc".
fn initials(name: &str) -> SharedString {
    let mut words = name
        .split_whitespace()
        .filter_map(|word| word.chars().next());
    let mut text: String = words
        .next()
        .map(|first| first.to_uppercase().collect())
        .unwrap_or_else(|| "?".into());
    text.extend(words.take(2).flat_map(char::to_lowercase));
    text.into()
}

/// A message continues the previous one's group when the same person sent it
/// within seven minutes, without replying to something else.
fn continues(before: &ChatMessage, message: &ChatMessage) -> bool {
    let sender = |message: &ChatMessage| {
        if message.sender.is_empty() {
            message.author.clone()
        } else {
            message.sender.clone()
        }
    };
    let close = before.timestamp == 0
        || message.timestamp == 0
        || message.timestamp.abs_diff(before.timestamp) <= 7 * 60;
    message.reply.is_none() && sender(before) == sender(message) && close
}

/// The UTC calendar date of a timestamp, such as "3 October 2026"; empty when unknown.
fn day(timestamp: u64) -> String {
    if timestamp == 0 {
        return String::new();
    }
    // Howard Hinnant's days-to-civil algorithm.
    let days = (timestamp / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    format!("{day} {} {year}", MONTHS[(month - 1) as usize])
}

fn file_size(bytes: u64) -> String {
    match bytes {
        0..1_024 => format!("{bytes} B"),
        1_024..1_048_576 => format!("{:.0} KB", bytes as f64 / 1_024.),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    }
}

fn picture(picture: &Picture) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &picture.rgba,
        picture.width,
        picture.height,
    );
    slint::Image::from_rgba8(buffer)
}

fn message_row(message: &ChatMessage) -> MessageRow {
    // Slint's Markdown subset rejects some syntax, such as headings; show those as typed.
    let styled = if message.formatted {
        slint::StyledText::from_markdown(&message.body)
            .unwrap_or_else(|_| slint::StyledText::from_plain_text(&message.body))
    } else {
        slint::StyledText::default()
    };
    let attachment = message.attachment.as_ref();
    let preview = attachment.and_then(|attachment| attachment.preview.as_deref());
    let detail = attachment.map(|attachment| {
        let size = attachment.size.map(file_size);
        match (attachment.image, attachment.preview_failed, size) {
            (true, true, Some(size)) => format!("{size} · Preview unavailable"),
            (true, true, None) => "Preview unavailable".into(),
            (_, _, Some(size)) => size,
            _ => String::new(),
        }
    });
    let (delivery, can_retry, can_cancel) = match message.delivery {
        Delivery::Sent => ("", false, false),
        Delivery::Sending => ("Sending...", false, true),
        Delivery::Waiting => (
            "Not sent yet. Talk sends it when the connection returns.",
            false,
            true,
        ),
        Delivery::Failed => ("Not sent. The server refused this message.", true, true),
    };
    let reply_author = message
        .reply
        .as_ref()
        .map(|reply| reply.author.as_str())
        .unwrap_or_default();
    MessageRow {
        id: message.id.clone().into(),
        item: message.item.clone().into(),
        author: message.author.clone().into(),
        initial: initial(&message.author),
        tint: tint(&message.author),
        body: message.body.clone().into(),
        time: message.time.clone().into(),
        // The gutter beside a grouped message shows only the clock time.
        short_time: message
            .time
            .split_whitespace()
            .next()
            .filter(|time| time.contains(':'))
            .unwrap_or_default()
            .into(),
        own: message.own,
        // Grouping depends on the neighbouring messages; refresh sets both.
        first: true,
        day: SharedString::default(),
        formatted: message.formatted,
        styled,
        edited: message.edited,
        deleted: message.deleted,
        has_reply: message.reply.is_some(),
        reply_author: reply_author.into(),
        reply_initial: initial(reply_author),
        reply_tint: tint(reply_author),
        reply_body: message
            .reply
            .as_ref()
            .map(|reply| reply.body.clone())
            .unwrap_or_default()
            .into(),
        reactions: ModelRc::new(VecModel::from(
            message
                .reactions
                .iter()
                .map(|reaction| ReactionChip {
                    key: reaction.key.clone().into(),
                    count: i32::try_from(reaction.count).unwrap_or(i32::MAX),
                    mine: reaction.mine,
                })
                .collect::<Vec<_>>(),
        )),
        // An image with a preview shows the picture; any other attachment shows a file card.
        has_attachment: attachment.is_some(),
        attachment: attachment
            .map(|attachment| attachment.name.clone())
            .unwrap_or_default()
            .into(),
        attachment_detail: detail.unwrap_or_default().into(),
        has_image: preview.is_some(),
        image: preview.map(picture).unwrap_or_default(),
        image_width: preview.map_or(0, |preview| preview.width as i32),
        image_height: preview.map_or(0, |preview| preview.height as i32),
        delivery: delivery.into(),
        delivery_failed: message.delivery == Delivery::Failed,
        can_retry,
        can_cancel,
        can_reply: message.can_reply,
        can_edit: message.can_edit,
        can_delete: message.can_delete,
        can_react: message.can_react,
    }
}

fn initial(text: &str) -> SharedString {
    text.chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string()
        .into()
}

/// WebRTC names its default entry "default: Device"; show it as "Default (Device)".
fn device_label(name: &str) -> SharedString {
    match name.strip_prefix("default: ") {
        Some(device) => format!("Default ({device})").into(),
        None => name.into(),
    }
}

/// Without a choice, voice uses WebRTC's default device, which it lists first.
fn device_index(devices: &[voice::Device], selected: Option<&str>) -> i32 {
    let index = match selected {
        Some(selected) => devices.iter().position(|device| device.id == selected),
        None => (!devices.is_empty()).then_some(0),
    };
    index
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
    fn scrolling_up_leaves_the_latest_message() {
        i_slint_backend_testing::init_no_event_loop();
        let window = TalkWindow::new().unwrap();
        window.set_workspace(true);
        window.set_room_selected(true);
        window.set_room_ready(true);
        let rows: Vec<MessageRow> = (0..200)
            .map(|index| MessageRow {
                id: format!("m{index}").into(),
                item: format!("i{index}").into(),
                author: "Bob".into(),
                body: if index % 2 == 0 {
                    format!("Message {index}").into()
                } else {
                    format!("Message {index}, a longer line that wraps across the timeline at ordinary window widths, and then some more words").into()
                },
                first: index % 5 == 0,
                ..Default::default()
            })
            .collect();
        window.set_messages(ModelRc::new(VecModel::from(rows)));
        window.show().unwrap();
        window
            .window()
            .set_size(slint::LogicalSize::new(1120., 760.));
        let settle = || {
            for _ in 0..5 {
                slint::platform::update_timers_and_animations();
                let _ = window.invoke_timeline_extent();
            }
        };
        settle();
        window.invoke_scroll_to_latest();
        settle();
        assert!(
            window.invoke_timeline_at_end(),
            "starts at the latest message"
        );
        for _ in 0..8 {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(700., 380.),
                    delta_x: 0.,
                    delta_y: 50.,
                });
            settle();
        }
        assert!(
            !window.invoke_timeline_at_end(),
            "scrolling up leaves the latest message"
        );
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
    fn message_actions_reach_the_shared_application_in_the_preview() {
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
        let row = |item: &SharedString| {
            window
                .get_messages()
                .iter()
                .find(|row| row.item == *item)
                .unwrap()
        };
        let own = window.get_messages().iter().find(|row| row.own).unwrap();
        let other = window.get_messages().iter().find(|row| !row.own).unwrap();
        assert!(own.can_edit && !other.can_edit && !window.get_can_attach());

        window.invoke_react(other.item.clone(), "👍".into());
        let reactions = row(&other.item).reactions;
        assert_eq!(reactions.row_count(), 1);
        assert!(reactions.row_data(0).unwrap().mine);
        window.invoke_react(other.item.clone(), "👍".into());
        assert_eq!(row(&other.item).reactions.row_count(), 0);

        window.invoke_reply(other.item.clone());
        assert!(window.get_compose_label().starts_with("Replying to Sam"));
        window.invoke_draft_edited("**Sure**".into());
        window.invoke_send();
        let reply = window.get_messages().iter().last().unwrap();
        assert!(reply.has_reply && reply.formatted && reply.reply_author == "Sam");
        assert!(window.get_compose_label().is_empty() && window.get_draft().is_empty());

        window.invoke_draft_edited("Unsent draft".into());
        window.invoke_edit(own.item.clone());
        assert_eq!(window.get_draft(), own.body);
        assert_eq!(window.get_compose_label(), "Editing your message");
        window.invoke_draft_edited("Changed".into());
        window.invoke_send();
        let edited = row(&own.item);
        assert!(edited.edited && edited.body == "Changed");
        assert_eq!(window.get_draft(), "Unsent draft");

        window.invoke_request_delete(other.item.clone());
        assert!(
            window.get_confirm_delete().is_empty(),
            "only own messages can be deleted"
        );
        window.invoke_request_delete(own.item.clone());
        assert_eq!(window.get_confirm_delete(), own.item);
        window.invoke_confirm_delete_message();
        assert!(row(&own.item).deleted && window.get_confirm_delete().is_empty());
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
