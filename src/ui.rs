//! Slint presentation and event-loop adapter for the shared application.
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
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
    application::{
        Action, AddFriend, Application, ClearAfter, Compose, Effect, Navigation, NewDm, Relation,
    },
    backend::Backend,
    chat::{
        ChatMessage, Command, Delivery, EmbedField, Event, Picture, Placement, Presence,
        ProfileRole, Role,
    },
    discord, links, mention,
    meter::{self, InputMeter},
    runtime::BackgroundRuntime,
    spoiler,
    voice::{self, Phase},
};

use crate::{custom_emoji, emoji, graphics};
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
    } else if talk_core::chat::can_restore(&adapter.borrow().root) {
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
    // Without Matrix everything is Discord's, so its badge would say nothing.
    window
        .global::<Badges>()
        .set_discord(cfg!(feature = "matrix"));
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
    window.set_emoji_font(emoji::family().into());
    copy_whole_messages(window);
    // The winit window exists once the event loop runs.
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        if let Some(window) = weak.upgrade() {
            FOCUSED.set(false);
            window.window().on_winit_window_event(|_, event| {
                if let slint::winit_030::winit::event::WindowEvent::Focused(focused) = event {
                    FOCUSED.set(*focused);
                }
                slint::winit_030::EventResult::Propagate
            });
        }
    });
}

thread_local! {
    /// Whether Talk's window has the keyboard focus, so notifications can wait.
    static FOCUSED: Cell<bool> = const { Cell::new(false) };
    /// Opens a conversation from a clicked notification, in the current window.
    static OPEN: RefCell<Option<OpenRoom>> = const { RefCell::new(None) };
}

type OpenRoom = Box<dyn Fn(String)>;

fn open_from_notification(room: String) {
    OPEN.with(|open| {
        if let Some(open) = open.borrow().as_ref() {
            open(room);
        }
    });
}

/// Lets a selection of text copy the messages it covers whole, including those scrolled out of
/// view, which the list no longer shows.
fn copy_whole_messages(window: &TalkWindow) {
    let window = window.as_weak();
    i_slint_core::talk_selection::set_row_text(move |row| {
        Some(message_text(
            &window.upgrade()?.get_messages().row_data(row)?,
        ))
    });
}

/// A message as a selection covering it copies it, reading as its row shows it: texts beside
/// each other joined by a space, others by line breaks, and each picture as `[Picture]`.
fn message_text(row: &MessageRow) -> String {
    let plain = i_slint_core::talk_selection::plain;
    let mut lines = Vec::new();
    if row.first {
        lines.push(format!(
            "{} {}",
            plain(&row.author_styled),
            plain(&row.time_styled)
        ));
    }
    if !row.body.is_empty() {
        lines.push(plain(&row.styled));
    }
    let pictures = |count: usize| std::iter::repeat_n("[Picture]".to_string(), count);
    lines.extend(pictures(row.images.row_count()));
    for embed in row.embeds.iter() {
        let mut card = Vec::new();
        let texts = [
            (embed.has_provider, &embed.provider),
            (embed.has_author, &embed.author),
            (embed.has_title, &embed.title),
            (embed.has_description, &embed.description),
        ];
        card.extend(
            texts
                .iter()
                .filter(|(shown, _)| *shown)
                .map(|(_, text)| plain(text)),
        );
        for fields in embed.field_rows.iter() {
            let (names, values): (Vec<_>, Vec<_>) = fields
                .fields
                .iter()
                .map(|field| (plain(&field.name), plain(&field.value)))
                .unzip();
            card.push(names.join(" "));
            card.push(values.join(" "));
        }
        card.extend(pictures(embed.images.row_count()));
        if embed.has_footer {
            card.push(plain(&embed.footer));
        }
        // The thumbnail sits level with the card's first text, to its right.
        if embed.has_thumbnail {
            card.insert(card.len().min(1), "[Picture]".into());
        }
        lines.extend(card);
    }
    lines.join("\n")
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

/// The layout a person chose, such as the sidebar width, kept beside the session.
const LAYOUT: &str = "layout.json";

#[derive(Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Layout {
    sidebar_width: Option<f32>,
    /// The member list shows until someone closes it.
    members_hidden: bool,
    /// Each server's last selected channel, by server ID.
    channels: HashMap<String, String>,
    /// How often this device added each reaction, for the frequent reactions.
    reactions: HashMap<String, u32>,
    /// List Discord channels hidden from this account, locked.
    show_hidden_channels: bool,
    /// Reading in Talk marks Discord channels read on Discord; on unless turned off.
    sync_discord_reads: Option<bool>,
    /// Message times as 9:41 PM rather than 21:41.
    twelve_hour_clock: bool,
    /// Message text: 0 small, 1 default, 2 large.
    text_size: Option<i32>,
    /// Link previews under messages; on unless turned off.
    link_previews: Option<bool>,
    /// Joining a call turns the microphone on.
    unmute_on_join: bool,
    /// Your voice is evened out in calls; on unless turned off.
    level_voice: Option<bool>,
    /// How loud people sound in calls, and their streams, by Discord or
    /// Matrix user id; see [`talk_core::volume`]. Unlisted people are as they
    /// come.
    volumes: std::collections::BTreeMap<String, f32>,
    stream_volumes: std::collections::BTreeMap<String, f32>,
    /// Others see when this account is typing; on unless turned off.
    share_typing: Option<bool>,
    /// Desktop notifications, their sound, and whether they say what messages say; on
    /// unless turned off.
    notifications: Option<bool>,
    notification_sound: Option<bool>,
    notification_text: Option<bool>,
}

impl Layout {
    fn read(root: &std::path::Path) -> Self {
        std::fs::read(root.join(LAYOUT))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn apply(&self, window: &TalkWindow) {
        if let Some(width) = self.sidebar_width {
            window.set_sidebar_width(width);
        }
        window.set_member_list(!self.members_hidden);
        window.set_twelve_hour_clock(self.twelve_hour_clock);
        window.set_text_size(self.text_size.unwrap_or(1).clamp(0, 2));
        window.set_link_previews(self.link_previews.unwrap_or(true));
        window.set_unmute_on_join(self.unmute_on_join);
        window.set_level_voice(self.level_voice.unwrap_or(true));
        talk_core::leveling::set_enabled(self.level_voice.unwrap_or(true));
        for (person, volume) in &self.volumes {
            talk_core::volume::set(talk_core::volume::Kind::Voice, person, *volume);
        }
        for (person, volume) in &self.stream_volumes {
            talk_core::volume::set(talk_core::volume::Kind::Stream, person, *volume);
        }
        window.set_share_typing(self.share_typing.unwrap_or(true));
        window.set_notifications(self.notifications.unwrap_or(true));
        window.set_notification_sound(self.notification_sound.unwrap_or(true));
        window.set_notification_text(self.notification_text.unwrap_or(true));
    }

    /// A message's row as these preferences show it: its time on the chosen clock, and its
    /// link previews only when they're on.
    fn restyle(&self, message: &ChatMessage, mut row: MessageRow) -> MessageRow {
        if message.timestamp != 0 {
            let time = talk_core::clock::time_on(message.timestamp, self.twelve_hour_clock);
            // The gutter beside a grouped message is narrow, so it leaves out AM and PM.
            row.short_time = time.split_whitespace().next().unwrap_or_default().into();
            row.time_styled = slint::StyledText::from_plain_text(&time);
            row.time = time.into();
        }
        if !self.link_previews.unwrap_or(true) {
            row.embeds = ModelRc::default();
            // A bot's card can be the whole message; say it's there.
            if message.body.trim().is_empty()
                && !message.attachments.iter().any(|attachment| {
                    matches!(
                        attachment.placement,
                        Placement::Message | Placement::Sticker
                    )
                })
                && let Some(embed) = message.embeds.first()
            {
                row.hidden_card = if embed.title.is_empty() {
                    "A card, hidden while link previews are off.".into()
                } else {
                    format!(
                        "A card, \"{}\", hidden while link previews are off.",
                        embed.title
                    )
                    .into()
                };
            }
        }
        row
    }

    /// A lost layout only costs the person their arrangement; it never stops Talk.
    fn write(&self, root: &std::path::Path) {
        let result = std::fs::create_dir_all(root).and_then(|()| {
            std::fs::write(
                root.join(LAYOUT),
                serde_json::to_vec(self).expect("layout serializes"),
            )
        });
        if let Err(error) = result {
            tracing::warn!(%error, "could not store the window layout");
        }
    }
}

#[derive(Default)]
struct Models {
    servers: Rc<VecModel<ServerRow>>,
    rooms: Rc<VecModel<RoomRow>>,
    messages: Rc<VecModel<MessageRow>>,
    roster: Rc<VecModel<MemberRow>>,
    mentions: Rc<VecModel<MentionRow>>,
    new_dm_people: Rc<VecModel<ContactRow>>,
    role_members: Rc<VecModel<MemberRow>>,
    bar_reactions: Rc<VecModel<EmojiCell>>,
    menu_reactions: Rc<VecModel<EmojiCell>>,
    emoji_rows: Rc<VecModel<EmojiRow>>,
    forward_targets: Rc<VecModel<ForwardRow>>,
    participants: Rc<VecModel<PersonRow>>,
    microphones: Rc<VecModel<SharedString>>,
    speakers: Rc<VecModel<SharedString>>,
    staged: Rc<VecModel<StagedFile>>,
}

impl Models {
    fn attach(&self, window: &TalkWindow) {
        window.set_servers(ModelRc::from(self.servers.clone()));
        window.set_rooms(ModelRc::from(self.rooms.clone()));
        window.set_messages(ModelRc::from(self.messages.clone()));
        window.set_roster(ModelRc::from(self.roster.clone()));
        window.set_mentions(ModelRc::from(self.mentions.clone()));
        window.set_new_dm_people(ModelRc::from(self.new_dm_people.clone()));
        window.set_role_members(ModelRc::from(self.role_members.clone()));
        window.set_bar_reactions(ModelRc::from(self.bar_reactions.clone()));
        window.set_menu_reactions(ModelRc::from(self.menu_reactions.clone()));
        window.set_emoji_rows(ModelRc::from(self.emoji_rows.clone()));
        window.set_forward_targets(ModelRc::from(self.forward_targets.clone()));
        window.set_participants(ModelRc::from(self.participants.clone()));
        window.set_microphones(ModelRc::from(self.microphones.clone()));
        window.set_speakers(ModelRc::from(self.speakers.clone()));
        window.set_staged(ModelRc::from(self.staged.clone()));
    }
}

struct Icons {
    dm: slint::Image,
    text: slint::Image,
    voice: slint::Image,
    lock: slint::Image,
}

struct Adapter {
    // Cancel the UI event receiver before releasing the actor and runtime.
    events: Option<slint::JoinHandle<()>>,
    backend: Option<Backend>,
    runtime: Arc<BackgroundRuntime>,
    root: PathBuf,
    layout: Layout,
    app: Application,
    models: Models,
    icons: Icons,
    weak: std::rc::Weak<RefCell<Self>>,
    window: slint::Weak<TalkWindow>,
    navigation: Navigation,
    /// Whether the forward dialog was open at the last refresh.
    forwarding: bool,
    selection_version: u64,
    reset_version: u64,
    messages: Arc<Vec<ChatMessage>>,
    /// Rows already built for unchanged messages, so the list keeps their state.
    rows: HashMap<String, (ChatMessage, MessageRow)>,
    /// Server icons and avatars already handed to Slint, by address.
    pictures: HashMap<String, slint::Image>,
    /// How many pictures had arrived when the message rows were last built.
    pictures_seen: usize,
    /// The last jump to a message the timeline made; see `AppState::jump`.
    jumped: Option<(String, u64)>,
    /// Spoilers clicked; see `message_row`.
    revealed: HashSet<String>,
    /// The preferences that shape message rows changed, so they're built again.
    restyle: bool,
    /// Where the `@` of the word being mentioned is in the composer text.
    mention_at: Option<usize>,
    /// Suggestions for the `@` here were closed with Escape; they stay closed until
    /// the cursor leaves the word.
    mention_dismissed: Option<usize>,
    notifier: crate::notify::Notifier,
    /// The viewer's image, converted for Slint while it shows.
    viewer_image: Option<(Arc<Picture>, slint::Image)>,
    /// The size the viewer opened an image at, by item and attachment, kept
    /// so the full image replaces the preview without the image moving.
    viewer_size: Option<(String, usize, u32, u32)>,
    /// One open or save dialog at a time; dropping it cancels the dialog's task.
    dialog: Option<slint::JoinHandle<()>>,
    /// Reads the microphone while the input menu shows its level.
    meter: Option<InputMeter>,
    /// Previews of the pictures waiting in the composer, by file; `None` while
    /// one is read, or when it can't be.
    thumbnails: HashMap<PathBuf, Option<slint::Image>>,
    /// Where pasted pictures are saved until they're sent; removed as Talk closes.
    pasted: Option<tempfile::TempDir>,
    /// Ringing for a DM call that rings this account.
    #[cfg(feature = "discord")]
    ringer: Option<talk_core::ring::Ringer>,
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
            lock: slint::Image::load_from_svg_data(include_bytes!("../ui/icons/lock.svg"))?,
        };
        let models = Models::default();
        models.attach(window);
        let layout = Layout::read(&root);
        layout.apply(window);
        let mut app = Application::default();
        app.remember_reaction_uses(layout.reactions.clone());
        app.dispatch(Action::ShowHiddenChannels(layout.show_hidden_channels));
        app.dispatch(Action::SyncDiscordReads(
            layout.sync_discord_reads.unwrap_or(true),
        ));
        Ok(Rc::new_cyclic(|weak| {
            RefCell::new(Self {
                events: None,
                backend: None,
                runtime,
                root,
                layout,
                app,
                models,
                icons,
                weak: weak.clone(),
                window: window.as_weak(),
                navigation: Navigation::default(),
                forwarding: false,
                selection_version: 0,
                reset_version: 0,
                messages: Arc::new(Vec::new()),
                rows: HashMap::new(),
                pictures: HashMap::new(),
                pictures_seen: 0,
                jumped: None,
                revealed: HashSet::new(),
                restyle: false,
                mention_at: None,
                mention_dismissed: None,
                notifier: crate::notify::Notifier::start(open_from_notification),
                viewer_image: None,
                viewer_size: None,
                dialog: None,
                meter: None,
                thumbnails: HashMap::new(),
                pasted: None,
                #[cfg(feature = "discord")]
                ringer: None,
            })
        }))
    }

    /// Present the existing models and state in a new window.
    fn attach(&mut self, window: &TalkWindow) {
        self.models.attach(window);
        self.layout.apply(window);
        self.window = window.as_weak();
    }

    /// Ctrl+V in the composer: files and pictures on the clipboard wait to go
    /// with the message, and anything else is pasted as text.
    fn paste(&mut self) {
        if self.pasted.is_none() {
            self.pasted = tempfile::Builder::new()
                .prefix("talk-pasted-")
                .tempdir()
                .inspect_err(
                    |error| tracing::warn!(%error, "could not make a folder for pasted pictures"),
                )
                .ok();
        }
        let folder = self.pasted.as_ref().map(|dir| dir.path().to_owned());
        let reading = self
            .runtime
            .spawn_blocking(move || match crate::paste::read() {
                crate::paste::Clipboard::Files(files) => Some(files),
                crate::paste::Clipboard::Picture(bytes, extension) => {
                    // A folder for each paste, so each picture is image.png, as in Discord.
                    let folder = tempfile::Builder::new()
                        .prefix("")
                        .tempdir_in(folder?)
                        .ok()?
                        .keep();
                    let path = folder.join(format!("image.{extension}"));
                    std::fs::write(&path, bytes).ok()?;
                    Some(vec![path])
                }
                crate::paste::Clipboard::Other => None,
            });
        let weak = self.weak.clone();
        let task = slint::spawn_local(async move {
            let files = reading.await.ok().flatten();
            let Some(adapter) = weak.upgrade() else {
                return;
            };
            let window = adapter.borrow().window.upgrade();
            let Some(window) = window else {
                return;
            };
            match files {
                Some(files) => adapter.borrow_mut().action(&window, Action::Stage(files)),
                None => window.invoke_paste_composer_text(),
            }
        });
        if let Err(error) = task {
            tracing::error!(%error, "could not read the clipboard");
        }
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
                    self.app.remember_channels(self.layout.channels.clone());
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
                Effect::ListDevices => {
                    let weak = self.weak.clone();
                    // Listing starts the platform's audio service, which can block.
                    let listing = self.runtime.spawn_blocking(|| {
                        voice::list_devices().map_err(|error| error.to_string())
                    });
                    let task = slint::spawn_local(async move {
                        let devices = listing
                            .await
                            .unwrap_or_else(|_| Err("Could not list audio devices.".into()));
                        if let Some(adapter) = weak.upgrade() {
                            let window = adapter.borrow().window.upgrade();
                            let mut adapter = adapter.borrow_mut();
                            adapter.app.preview_devices(devices);
                            if let Some(window) = window {
                                adapter.refresh(&window);
                            }
                        }
                    });
                    if let Err(error) = task {
                        tracing::error!(%error, "could not list audio devices for the preview");
                    }
                }
                Effect::Notify(alert) => {
                    // Only while Talk isn't in front: there the lists show what's new.
                    if self.layout.notifications.unwrap_or(true) && !FOCUSED.get() {
                        let style = crate::notify::Style {
                            text: self.layout.notification_text.unwrap_or(true),
                            sound: self.layout.notification_sound.unwrap_or(true),
                        };
                        self.notifier.show(alert, style);
                    }
                }
                Effect::Backend(Command::Typing { .. })
                    if !self.layout.share_typing.unwrap_or(true) => {}
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

    /// The open profile card, with what's known so far.
    fn show_profile(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        window.set_profile_open(state.profile.is_some());
        let Some(view) = &state.profile else {
            window.set_profile_member(SharedString::default());
            return;
        };
        window.set_profile_large(view.large);
        window.set_profile_member(if view.large {
            SharedString::default()
        } else {
            view.profile.id.clone().into()
        });
        let profile = &view.profile;
        let handle = [profile.handle.as_str(), profile.pronouns.as_str()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
        let strings = |items: &[String]| {
            ModelRc::new(VecModel::from(
                items.iter().map(SharedString::from).collect::<Vec<_>>(),
            ))
        };
        window.set_profile(ProfileData {
            id: profile.id.clone().into(),
            name: profile.name.clone().into(),
            initial: initial(&profile.name),
            tint: tint(&profile.name),
            picture: cached_picture(&mut self.pictures, state, profile.avatar.as_deref()),
            handle: handle.into(),
            banner: cached_picture(&mut self.pictures, state, profile.banner.as_deref()),
            accent: role_color(profile.accent),
            has_accent: profile.accent.is_some(),
            presence: presence_name(profile.presence).into(),
            activity: profile.activity.clone().unwrap_or_default().into(),
            bio: markdown(&profile.bio),
            has_bio: !profile.bio.trim().is_empty(),
            role_rows: role_rows(&profile.roles, PROFILE_CARD_WIDTH),
            role_rows_large: role_rows(&profile.roles, PROFILE_LARGE_WIDTH),
            created: profile.created.clone().unwrap_or_default().into(),
            joined: profile.joined.clone().unwrap_or_default().into(),
            servers: strings(&profile.servers),
            friends: strings(&profile.friends),
            note: if view.loading {
                "Loading the profile...".into()
            } else {
                view.note.clone().unwrap_or_default().into()
            },
            // Matrix IDs start with `@`; Discord user ids are numbers.
            friend: if profile.id.starts_with('@') {
                0
            } else {
                friend_offer(state.relation(&profile.id))
            },
        });
    }

    /// The image viewer: the full image once it arrives, the preview until then.
    fn show_viewer(&mut self, window: &TalkWindow) {
        let state = &self.app.state;
        let viewing = state.viewer.as_ref().and_then(|viewer| {
            let message = state.message(&viewer.item)?;
            Some((viewer, message, message.attachments.get(viewer.index)?))
        });
        window.set_viewer_open(viewing.is_some());
        let Some((viewer, message, attachment)) = viewing else {
            self.viewer_image = None;
            self.viewer_size = None;
            return;
        };
        let loaded = viewer.pictures.get(&viewer.index);
        let full = match loaded {
            Some(Some(Ok(picture))) => Some(picture),
            _ => None,
        };
        window.set_viewer_note(
            match loaded {
                Some(Some(Err(error))) => error,
                Some(None) => "Loading the full image...",
                _ => "",
            }
            .into(),
        );
        // The full image's size from the start where it's known, else the
        // preview's. Either way the image keeps the size it opened at, so the
        // full image only sharpens it on arrival.
        let (width, height) = match &self.viewer_size {
            Some((item, index, width, height))
                if *item == viewer.item && *index == viewer.index =>
            {
                (*width, *height)
            }
            _ => {
                let size = match (full, attachment.dimensions, &attachment.preview) {
                    (Some(full), _, _) => Some((full.width, full.height)),
                    (None, Some((width, height)), _) => Some(fit(width, height)),
                    (None, None, Some(preview)) => Some((preview.width, preview.height)),
                    _ => None,
                };
                self.viewer_size =
                    size.map(|(width, height)| (viewer.item.clone(), viewer.index, width, height));
                size.unwrap_or((1, 1))
            }
        };
        window.set_viewer_width(width as i32);
        window.set_viewer_height(height as i32);
        let shown = full.or(attachment.preview.as_ref());
        let image = match (shown, &self.viewer_image) {
            (Some(shown), Some((cached, image))) if Arc::ptr_eq(shown, cached) => image.clone(),
            (Some(shown), _) => {
                let image = picture(shown);
                self.viewer_image = Some((shown.clone(), image.clone()));
                image
            }
            (None, _) => slint::Image::default(),
        };
        window.set_viewer_image(image);
        let (frames, delays) = shown.map(animation).unwrap_or_default();
        window.set_viewer_frames(frames);
        window.set_viewer_delays(delays);
        window.set_viewer_name(attachment.name.clone().into());
        window.set_viewer_item(viewer.item.clone().into());
        window.set_viewer_index(viewer.index as i32);
        let images: Vec<usize> = message
            .attachments
            .iter()
            .enumerate()
            .filter(|(_, attachment)| attachment.image)
            .map(|(index, _)| index)
            .collect();
        let place = images.iter().position(|index| *index == viewer.index);
        window.set_viewer_position(match place {
            Some(place) if images.len() > 1 => format!("{} / {}", place + 1, images.len()).into(),
            _ => SharedString::default(),
        });
    }

    fn refresh(&mut self, window: &TalkWindow) {
        self.show_viewer(window);
        self.show_profile(window);
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
            window.set_settings_open(false);
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
        window.set_restore_available(talk_core::chat::can_restore(&self.root));
        window.set_dms_selected(state.navigation == Navigation::DirectMessages);
        window.set_dms_unread(
            state
                .rooms
                .iter()
                .any(|room| room.direct && room.unread > 0),
        );
        window.set_discord_linked(state.servers.iter().any(|server| server.discord));
        window.set_show_hidden_channels(state.show_hidden_channels);
        window.set_sync_discord_reads(state.sync_discord_reads);
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
                    picture: cached_picture(&mut self.pictures, state, server.icon.as_deref()),
                    selected: state.navigation == Navigation::Server(server.id.clone()),
                    unread: state.rooms.iter().any(|room| {
                        (room.mentions > 0 || (room.unread > 0 && !server.muted))
                            && !room.direct
                            && server.channels.contains(&room.id)
                    }),
                    discord: server.discord,
                })
                .collect(),
        );
        let mut listed_text = false;
        let mut listed_voice = false;
        let mut category: Option<&str> = None;
        let mut listed: Vec<_> = state
            .rooms
            .iter()
            .filter(|room| state.in_scope(room) && room.name.to_lowercase().contains(&query))
            .collect();
        let direct = state.navigation == Navigation::DirectMessages;
        if direct {
            // Message requests come first, as in Discord, and spam last.
            listed.sort_by_key(|room| match room.category.as_str() {
                discord::REQUESTS => 0,
                discord::SPAM => 2,
                _ => 1,
            });
        }
        let counted = |name: &str| listed.iter().filter(|room| room.category == name).count();
        // Friend requests head the DM list, matched by name like the DMs.
        let requests: Vec<_> = state
            .friend_requests
            .iter()
            .filter(|_| direct)
            .filter(|request| request.name.to_lowercase().contains(&query))
            .map(|request| FriendRequestRow {
                id: request.id.as_str().into(),
                name: request.name.as_str().into(),
                initial: initial(&request.name),
                tint: tint(&request.id),
                picture: cached_picture(&mut self.pictures, state, request.avatar.as_deref()),
                incoming: request.incoming,
                detail: match (request.incoming, request.spam) {
                    (true, true) => "Maybe spam",
                    (true, false) => "Incoming request",
                    (false, _) => "Request sent",
                }
                .into(),
            })
            .collect();
        let friend_requests = !requests.is_empty();
        window.set_friend_requests(ModelRc::new(VecModel::from(requests)));
        let (requests, spam) = (counted(discord::REQUESTS), counted(discord::SPAM));
        let rows: Vec<_> = listed
            .into_iter()
            .map(|room| {
                let heading = if direct {
                    // Headings only once requests are listed.
                    match (category.replace(&room.category), room.category.as_str()) {
                        (previous, discord::REQUESTS) if previous != Some(discord::REQUESTS) => {
                            format!("MESSAGE REQUESTS — {requests}")
                        }
                        (previous, discord::SPAM) if previous != Some(discord::SPAM) => {
                            format!("SPAM — {spam}")
                        }
                        (Some(discord::REQUESTS), "") => "DIRECT MESSAGES".into(),
                        (None, "") if friend_requests => "DIRECT MESSAGES".into(),
                        _ => String::new(),
                    }
                } else if discord::is_discord(&room.id) {
                    // Discord channels sit under their categories, as in Discord.
                    if category.replace(&room.category) != Some(&room.category) {
                        room.category.to_uppercase()
                    } else {
                        String::new()
                    }
                } else if room.voice && !listed_voice {
                    listed_voice = true;
                    "VOICE CHANNELS".into()
                } else if !room.voice && !listed_text {
                    listed_text = true;
                    "TEXT CHANNELS".into()
                } else {
                    String::new()
                };
                RoomRow {
                    id: room.id.clone().into(),
                    name: room.name.clone().into(),
                    heading: heading.into(),
                    icon: if room.locked {
                        self.icons.lock.clone()
                    } else if room.voice {
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
                    picture: cached_picture(&mut self.pictures, state, room.avatar.as_deref()),
                    discord: room.direct && discord::is_discord(&room.id),
                    locked: room.locked,
                    person: room.person.is_some(),
                    people: if room.people.is_empty() {
                        ModelRc::default()
                    } else {
                        let people: Vec<_> = room
                            .people
                            .iter()
                            .map(|person| PersonRow {
                                id: person.id.clone().into(),
                                name: person.name.clone().into(),
                                initial: initial(&person.name),
                                tint: tint(&person.id),
                                picture: cached_picture(
                                    &mut self.pictures,
                                    state,
                                    person.avatar.as_deref(),
                                ),
                                muted: person.muted,
                                deafened: person.deafened,
                                live: person.streaming,
                                connected: true,
                                ..Default::default()
                            })
                            .collect();
                        ModelRc::new(VecModel::from(people))
                    },
                    call: room.direct && !room.people.is_empty(),
                }
            })
            .collect();
        window.set_sidebar_empty(if !rows.is_empty() { "" }
            else if state.rooms.iter().any(|room| state.in_scope(room)) { "No matching conversations." }
            else if state.navigation == Navigation::DirectMessages { "No direct messages yet. Existing DMs appear here after syncing." }
            else { "No joined channels in this server. Join a channel in another Matrix client to see it here." }.into());
        update_model(&self.models.rooms, rows);
        let account = state.account.as_deref().unwrap_or("Preview");
        let account_name = state
            .account_name
            .as_deref()
            .unwrap_or_else(|| display_part(account));
        window.set_account(account.into());
        window.set_account_name(account_name.into());
        window.set_account_initial(initial(account_name));
        window.set_account_tint(tint(account_name));
        window.set_account_picture(cached_picture(
            &mut self.pictures,
            state,
            state.account_avatar.as_deref(),
        ));
        let custom_status = state
            .custom_status
            .as_ref()
            .filter(|_| state.online)
            .map(status_line);
        window.set_account_status(
            if state.preview {
                "Local only"
            } else if let Some(status) = &custom_status {
                status
            } else if let Some(status) = &state.account_status {
                status
            } else if state.online {
                "Online"
            } else {
                "Offline, sync paused"
            }
            .into(),
        );
        window.set_has_custom_status(state.custom_status.is_some());
        window.set_storage(state.storage.unwrap_or_default().into());
        window.set_notice(state.notice.as_deref().unwrap_or_default().into());
        window.set_room_selected(room.is_some());
        window.set_room_name(room.map_or("", |room| room.name.as_str()).into());
        window.set_room_direct(room.is_some_and(|room| room.direct));
        window.set_room_request(room.is_some_and(|room| room.request));
        window.set_request_title(
            match room {
                Some(room) if room.category == discord::SPAM => {
                    format!("Discord thinks {} may be spam", room.name)
                }
                Some(room) => format!("{} wants to message you", room.name),
                None => String::new(),
            }
            .into(),
        );
        window.set_room_initial(room.map_or(SharedString::default(), |room| initial(&room.name)));
        window.set_room_tint(room.map_or(slint::Color::default(), |room| tint(&room.id)));
        window.set_room_picture(cached_picture(
            &mut self.pictures,
            state,
            room.and_then(|room| room.avatar.as_deref()),
        ));
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
        window.set_members_available(state.lists_members());
        let members = match &state.members {
            Some(Ok(members)) => members.as_slice(),
            _ => &[],
        };
        window.set_roster_note(
            match &state.members {
                None => "Loading members...",
                Some(Err(error)) => *error,
                Some(Ok(members)) if members.is_empty() => "No one else has joined yet.",
                Some(Ok(_)) => "",
            }
            .into(),
        );
        update_model(
            &self.models.roster,
            members
                .iter()
                .enumerate()
                .map(|(index, member)| MemberRow {
                    id: member.id.clone().into(),
                    name: member.name.clone().into(),
                    initial: initial(&member.name),
                    tint: tint(&member.name),
                    picture: cached_picture(&mut self.pictures, state, member.avatar.as_deref()),
                    presence: presence_name(member.presence).into(),
                    color: role_color(member.color),
                    has_color: member.color.is_some(),
                    // One line under the name, as Discord shows it; the profile
                    // keeps the status's own line breaks.
                    activity: member
                        .activity
                        .as_deref()
                        .map(|activity| activity.split_whitespace().collect::<Vec<_>>().join(" "))
                        .unwrap_or_default()
                        .into(),
                    // Discord names its groups; Matrix members group by role.
                    heading: if let Some(group) = &member.group {
                        if index == 0 || members[index - 1].group.as_ref() != Some(group) {
                            group.to_uppercase().into()
                        } else {
                            SharedString::default()
                        }
                    } else if index == 0 || members[index - 1].role != member.role {
                        let count = members
                            .iter()
                            .filter(|other| other.role == member.role)
                            .count();
                        let role = match member.role {
                            Role::Admin => "ADMINS",
                            Role::Moderator => "MODERATORS",
                            Role::Member => "MEMBERS",
                        };
                        format!("{role} — {count}").into()
                    } else {
                        SharedString::default()
                    },
                })
                .collect(),
        );
        update_model(
            &self.models.bar_reactions,
            self.app
                .frequent_reactions(3)
                .iter()
                .map(|key| emoji::cell(key))
                .collect(),
        );
        update_model(
            &self.models.menu_reactions,
            self.app
                .frequent_reactions(4)
                .iter()
                .map(|key| emoji::cell(key))
                .collect(),
        );
        let role = state.role.as_ref();
        window.set_role_open(role.is_some());
        if let Some(role) = role {
            window.set_role_name(role.name.clone().into());
            window.set_role_color(role_color(role.color));
            window.set_role_has_color(role.color.is_some());
            window.set_role_note(
                match &role.members {
                    None => "Loading...",
                    Some(members) if members.is_empty() => {
                        "Talk hasn't seen anyone with this role yet."
                    }
                    Some(_) => "",
                }
                .into(),
            );
        }
        let role_members: Vec<MemberRow> = role
            .and_then(|role| role.members.as_ref())
            .into_iter()
            .flatten()
            .map(|member| MemberRow {
                id: member.id.clone().into(),
                name: member.name.clone().into(),
                initial: initial(&member.name),
                tint: tint(&member.name),
                picture: cached_picture(&mut self.pictures, state, member.avatar.as_deref()),
                color: role_color(member.color),
                has_color: member.color.is_some(),
                ..Default::default()
            })
            .collect();
        update_model(&self.models.role_members, role_members);
        let add_friend = state.add_friend.as_ref();
        if add_friend.is_some() && !window.get_add_friend_open() {
            window.set_add_friend_name(SharedString::default());
        }
        window.set_add_friend_open(add_friend.is_some());
        window.set_add_friend_sending(add_friend.is_some_and(|dialog| dialog.sending));
        let sent = add_friend.is_some_and(|dialog| matches!(dialog.outcome, Some(Ok(_))));
        // A request that went clears the name, ready for the next.
        if sent && !window.get_add_friend_sent() {
            window.set_add_friend_name(SharedString::default());
        }
        window.set_add_friend_sent(sent);
        window.set_add_friend_note(match add_friend {
            Some(AddFriend { sending: true, .. }) => "Sending...".into(),
            Some(AddFriend {
                outcome: Some(Ok(name)),
                ..
            }) => format!("Sent {name} a friend request.").into(),
            Some(AddFriend {
                outcome: Some(Err(error)),
                ..
            }) => error.as_str().into(),
            _ => SharedString::default(),
        });
        let editor = state.status_editor.as_ref();
        // Opening the dialog starts from the status there is.
        if editor.is_some() && !window.get_status_open() {
            let current = state.custom_status.as_ref();
            window.set_status_text(current.map_or("", |status| status.text.as_str()).into());
            window.set_status_emoji(
                current
                    .and_then(status_key)
                    .map_or_else(EmojiCell::default, |key| emoji::cell(&key)),
            );
            window.set_status_clear_after(0);
        }
        window.set_status_open(editor.is_some());
        window.set_status_saving(editor.is_some_and(|editor| editor.saving));
        window.set_status_error(
            editor
                .and_then(|editor| editor.error.as_deref())
                .unwrap_or_default()
                .into(),
        );
        let new_dm = state.new_dm.as_ref();
        if new_dm.is_some() && !window.get_new_dm_open() {
            window.set_new_dm_query(SharedString::default());
        }
        window.set_new_dm_open(new_dm.is_some());
        window.set_new_dm_starting(new_dm.is_some_and(|new_dm| new_dm.starting));
        let people: Vec<ContactRow> = new_dm
            .into_iter()
            .flat_map(|new_dm| new_dm.people())
            .map(|person| ContactRow {
                name: person.name.clone().into(),
                detail: person.detail.clone().into(),
                initial: initial(&person.name),
                tint: tint(&person.name),
                picture: cached_picture(&mut self.pictures, state, person.avatar.as_deref()),
                discord: person.discord,
            })
            .collect();
        window.set_new_dm_note(match new_dm {
            Some(NewDm { error: Some(error), .. }) => error.as_str().into(),
            Some(NewDm { starting: true, .. }) => "Starting the conversation...".into(),
            Some(new_dm) if people.is_empty() && new_dm.query.trim().is_empty() => {
                "Type a name to find people you know on Discord, or a full Matrix id such as @name:example.org.".into()
            }
            Some(_) if people.is_empty() => {
                "No one found. For Matrix, type a full id such as @name:example.org.".into()
            }
            _ => SharedString::default(),
        });
        update_model(&self.models.new_dm_people, people);
        let forwarding = state.forwarding.as_ref();
        window.set_forward_open(forwarding.is_some());
        if forwarding.is_some() != self.forwarding {
            window.set_forward_query(SharedString::default());
        }
        let forward_query = window.get_forward_query().to_lowercase();
        window.set_forward_preview(
            forwarding
                .and_then(|forwarding| state.message(&forwarding.item))
                .map(|message| match message.attachments.first() {
                    Some(attachment) if message.body.is_empty() => attachment.name.clone(),
                    _ => talk_core::chat::plain_snippet(&message.body),
                })
                .unwrap_or_default()
                .into(),
        );
        update_model(
            &self.models.forward_targets,
            forwarding.map_or_else(Vec::new, |forwarding| {
                state
                    .rooms
                    .iter()
                    .filter(|room| {
                        state.can_forward_to(room)
                            && room.name.to_lowercase().contains(&forward_query)
                    })
                    .map(|room| ForwardRow {
                        id: room.id.clone().into(),
                        name: room.name.clone().into(),
                        place: if room.direct {
                            "Direct message".into()
                        } else {
                            state
                                .servers
                                .iter()
                                .find(|server| server.channels.contains(&room.id))
                                .map_or("Channel".into(), |server| server.name.clone().into())
                        },
                        icon: if room.direct {
                            self.icons.dm.clone()
                        } else {
                            self.icons.text.clone()
                        },
                        direct: room.direct,
                        initial: initial(&room.name),
                        tint: tint(&room.id),
                        sent: forwarding.sent.contains(&room.id),
                    })
                    .collect()
            }),
        );
        window.set_voice_room(room.is_some_and(|room| room.voice));
        window.set_room_encrypted(room.is_some_and(|room| room.encrypted));
        window.set_room_ready(state.room_ready);
        window.set_draft(state.draft().into());
        let discord_room = room.is_some_and(|room| discord::is_discord(&room.id));
        // An unverified Matrix device can type; the hint says why it can't send.
        window.set_composer_enabled(
            state.room_ready
                && !state.busy
                && room.is_some_and(|room| {
                    if discord_room {
                        state.writable(room)
                    } else {
                        !room.voice && (state.preview || room.encrypted)
                    }
                }),
        );
        window.set_can_send(state.can_send());
        window.set_can_attach(state.can_attach());
        let staged = state.staged();
        self.thumbnails
            .retain(|path, _| staged.iter().any(|file| &file.path == path));
        let mut rows = Vec::with_capacity(staged.len());
        for file in staged {
            let picture = match self.thumbnails.get(&file.path) {
                Some(picture) => picture.clone(),
                None => {
                    if file.image {
                        self.thumbnails.insert(file.path.clone(), None);
                        load_thumbnail(&self.runtime, self.weak.clone(), file.path.clone());
                    }
                    None
                }
            };
            rows.push(StagedFile {
                name: file.name.as_str().into(),
                size: file_size(file.size).into(),
                has_picture: picture.is_some(),
                picture: picture.unwrap_or_default(),
            });
        }
        update_model(&self.models.staged, rows);
        window.set_confirm_delete(state.confirm_delete.clone().unwrap_or_default().into());
        window.set_compose_label(match state.compose() {
            Some(Compose::Reply { author, body, .. }) => {
                format!("Replying to {author}: {body}").into()
            }
            Some(Compose::Edit { .. }) => "Editing your message".into(),
            None => SharedString::default(),
        });
        window.set_sending(state.sending.is_some());
        window.set_typing(state.typing_line().into());
        let mentions: Vec<MentionRow> = state
            .mention_suggestions()
            .into_iter()
            .map(|candidate| MentionRow {
                initial: initial(&candidate.name),
                tint: tint(&candidate.name),
                picture: cached_picture(&mut self.pictures, state, candidate.avatar.as_deref()),
                color: role_color(candidate.color),
                has_color: candidate.color.is_some(),
                group: candidate.group,
                detail: match (candidate.group, candidate.mention.as_str()) {
                    (false, _) => "",
                    (true, "@everyone") => "Everyone in this channel",
                    (true, "@here") => "Everyone online here",
                    (true, _) => "Role",
                }
                .into(),
                name: candidate.name.into(),
            })
            .collect();
        if window.get_mention_index() >= mentions.len() as i32 {
            window.set_mention_index(0);
        }
        update_model(&self.models.mentions, mentions);
        window.set_loading_history(state.loading_history);
        window.set_composer_hint(
            if room.is_none() {
                "Choose a DM or server channel to write a message."
            } else if state.preview {
                "Enter to send locally, Shift+Enter for a new line."
            } else if room.is_some_and(|room| room.request) {
                "Discord · not end-to-end encrypted."
            } else if discord_room && room.is_some_and(|room| room.read_only || room.locked) {
                "You can't send messages in this Discord channel."
            } else if discord_room && !state.discord_online {
                "Discord is disconnected. Talk is reconnecting."
            } else if discord_room {
                "Discord · not end-to-end encrypted. Enter to send, Shift+Enter for a new line."
            } else if !state.trusted {
                "Sending is locked until this device is verified."
            } else if room.is_some_and(|room| room.encrypted) {
                "End-to-end encrypted. Enter to send, Shift+Enter for a new line."
            } else {
                "Unencrypted room. Reading only in this build."
            }
            .into(),
        );
        // Avatars that arrived since show on rows built before them.
        let pictures_arrived = self.pictures_seen != state.pictures.len();
        self.pictures_seen = state.pictures.len();
        // Custom emoji whose pictures arrived get glyphs, and the rows and picker
        // cells that show them by name are built again.
        if pictures_arrived || messages_changed {
            let usable = if pictures_arrived {
                self.app.usable_emoji()
            } else {
                Vec::new()
            };
            let state = &self.app.state;
            let added = custom_emoji::add(
                state
                    .messages
                    .iter()
                    .flat_map(|message| &message.emoji)
                    .chain(usable.iter().flat_map(|set| &set.emoji))
                    .filter_map(|emoji| {
                        Some((
                            emoji.id.as_str(),
                            state.pictures.get(&emoji.picture)?.as_ref(),
                        ))
                    }),
            );
            if added {
                self.restyle = true;
                if !window.get_react_item().is_empty() {
                    update_model(
                        &self.models.emoji_rows,
                        emoji::rows(
                            &window.get_emoji_query(),
                            &self.app.frequent_reactions(16),
                            &usable,
                        ),
                    );
                }
            }
        }
        let state = &self.app.state;
        let restyle = std::mem::take(&mut self.restyle);
        if messages_changed || selection_changed || reset || pictures_arrived || restyle {
            if selection_changed || reset {
                window.set_react_item(SharedString::default());
                window.set_active_item(SharedString::default());
                window.set_jump_item(SharedString::default());
            }
            let mut previous = std::mem::take(&mut self.rows);
            let mut before: Option<&ChatMessage> = None;
            let rows = state
                .messages
                .iter()
                .map(|message| {
                    let mut row = match previous.remove(&message.id) {
                        Some((old, row)) if &old == message && !restyle => row,
                        _ => self
                            .layout
                            .restyle(message, message_row(message, &self.revealed)),
                    };
                    self.rows
                        .insert(message.id.clone(), (message.clone(), row.clone()));
                    let day = day(message.timestamp);
                    let new_day = !day.is_empty()
                        && before.is_none_or(|before| self::day(before.timestamp) != day);
                    row.first = new_day || before.is_none_or(|before| !continues(before, message));
                    row.picture = if collapsed(message, &self.revealed) {
                        slint::Image::default()
                    } else {
                        cached_picture(&mut self.pictures, state, message.avatar.as_deref())
                    };
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
            // A jump to a message places the list itself, even one made after
            // this refresh; see `show_message`.
            let weak = window.as_weak();
            let jumped = move || {
                weak.upgrade()
                    .filter(|window| window.get_jump_item().is_empty())
            };
            if selection_changed || at_end {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = jumped() {
                        window.invoke_scroll_to_latest();
                    }
                });
            } else if prepended {
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if let Some(window) = jumped() {
                        window.invoke_preserve_history_position(
                            (window.invoke_timeline_extent() - old_extent).max(0.),
                        );
                    }
                });
            }
        }
        if state.jump != self.jumped {
            self.jumped = state.jump.clone();
            if let Some((item, _)) = &state.jump
                && let Some(index) = state
                    .messages
                    .iter()
                    .position(|message| &message.item == item)
            {
                show_message(window, index, item);
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
        window.set_can_share(
            !state.preview
                && voice.phase == Phase::Connected
                && voice
                    .room
                    .as_deref()
                    .is_some_and(talk_core::discord::is_discord),
        );
        window.set_sharing(voice.sharing.is_some());
        window.set_share_resolution(match voice.quality.resolution {
            voice::Resolution::P720 => 0,
            voice::Resolution::P1080 => 1,
            voice::Resolution::Source => 2,
        });
        window.set_share_fps(voice.quality.fps as i32);
        window.set_share_live(
            voice
                .sharing
                .as_ref()
                .is_some_and(|sharing| sharing.phase == voice::SharePhase::Live),
        );
        window.set_share_status(
            match voice.sharing.as_ref().map(|sharing| sharing.phase) {
                Some(voice::SharePhase::Choosing) => "Choose what to share",
                Some(voice::SharePhase::Starting) => "Starting your stream...",
                Some(voice::SharePhase::Live) => "Live",
                None => "",
            }
            .into(),
        );
        if voice.sharing.is_none() {
            window.set_share_frame(slint::Image::default());
        }
        window.set_watching(voice.watching.is_some());
        match &voice.watching {
            Some(watching) => {
                window.set_watch_title(format!("{}'s screen", watching.name).into());
                window.set_watch_status(
                    if watching.frames.is_none() {
                        format!("Connecting to {}'s stream...", watching.name)
                    } else {
                        format!("Waiting for {}'s picture...", watching.name)
                    }
                    .into(),
                );
            }
            None => window.set_watch_frame(slint::Image::default()),
        }
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
        let present = room.map_or(&[][..], |room| room.people.as_slice());
        // A Discord DM's call, shown above its messages.
        let dm_call = room.filter(|room| room.direct && discord::is_discord(&room.id));
        let dm_person = dm_call.and_then(|room| room.person.as_deref());
        window.set_dm_person(dm_person.unwrap_or_default().into());
        window.set_dm_friend(dm_person.map_or(0, |person| match state.relation(person) {
            // The header offers no "sent", which the profile card shows.
            Relation::Outgoing => 0,
            relation => friend_offer(relation),
        }));
        window.set_can_call(dm_call.is_some_and(|room| !room.request) && !voice_here);
        window.set_call_going(!present.is_empty());
        window.set_call_shown(
            dm_call.is_some_and(|room| voice_here || !room.people.is_empty() || room.incoming),
        );
        window.set_call_room(dm_call.map_or_else(Default::default, |room| room.id.as_str().into()));
        let incoming = dm_call.is_some_and(|room| room.incoming) && !voice_here;
        window.set_call_incoming(incoming);
        window.set_call_note(
            match (incoming, present.is_empty()) {
                (true, true) => "Incoming call.".into(),
                (true, false) => format!("{} calling you.", who_is_here(present)),
                (false, _) => format!("{} in the call.", who_is_here(present)),
            }
            .into(),
        );
        let call_people: Vec<_> = present
            .iter()
            .map(|person| PersonRow {
                id: person.id.as_str().into(),
                name: person.name.as_str().into(),
                initial: initial(&person.name),
                tint: tint(&person.id),
                picture: cached_picture(&mut self.pictures, state, person.avatar.as_deref()),
                connected: true,
                ..Default::default()
            })
            .collect();
        window.set_call_people(ModelRc::new(VecModel::from(call_people)));
        let ringing = show_incoming(window, state, &mut self.pictures);
        #[cfg(feature = "discord")]
        if !ringing {
            self.ringer = None;
        } else if self.ringer.is_none() {
            self.ringer = talk_core::ring::Ringer::start(
                talk_core::ring::Tone::Incoming,
                voice.speaker.as_deref(),
            )
            .inspect_err(|error| tracing::warn!(%error, "could not ring"))
            .ok();
        }
        #[cfg(not(feature = "discord"))]
        let _ = ringing;
        window.set_voice_description(
            if room.is_some_and(|room| room.locked) {
                "Your roles on this Discord server don't let you join this channel.".into()
            } else if let Some(room) = dm_call.filter(|_| voice_here) {
                let ringing: Vec<&str> = room
                    .ringing
                    .iter()
                    .map(|person| person.name.as_str())
                    .collect();
                if !ringing.is_empty() {
                    format!("Calling {}...", ringing.join(", "))
                } else if voice.participants.len() == 1 {
                    "Nobody else is in the call yet.".into()
                } else {
                    "The call stays connected while you browse other conversations.".into()
                }
            } else if voice_here && voice.participants.len() == 1 {
                "You're here alone. Stay as long as you like; others can join later.".into()
            } else if voice_here {
                "Voice stays connected while you browse other conversations.".into()
            } else if !present.is_empty() {
                format!("{} here. Join muted and hang out.", who_is_here(present))
            } else {
                "Join muted and hang out. The channel stays here when empty.".into()
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
                        id: person.id.clone().into(),
                        local: person.local,
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
                        } else if person.deafened {
                            "Deafened"
                        } else if person.muted {
                            "Muted"
                        } else if person.speaking {
                            "Speaking"
                        } else {
                            "Listening"
                        }
                        .into(),
                        muted: person.muted,
                        deafened: person.deafened,
                        live: person.streaming,
                        person: person.person.clone().into(),
                        picture: cached_picture(
                            &mut self.pictures,
                            state,
                            person.avatar.as_deref(),
                        ),
                    })
                    .collect()
            } else {
                vec![]
            },
        );
        let switching = state.switch_voice.as_ref().map(|id| {
            state
                .rooms
                .iter()
                .find(|room| &room.id == id)
                .map_or("the other channel", |room| room.name.as_str())
        });
        window.set_switch_prompt(
            switching
                .map(|target| {
                    format!(
                        "You're already in {}. Leave it and join {}? You'll join muted.",
                        voice.name, target
                    )
                })
                .unwrap_or_default()
                .into(),
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
        self.forwarding = state.forwarding.is_some();
        if self.app.reaction_uses() != &self.layout.reactions {
            self.layout.reactions = self.app.reaction_uses().clone();
            self.layout.write(&self.root);
        }
        if selection_changed && !state.preview && state.account.is_some() {
            let mut layout = self.layout.clone();
            layout.channels.extend(self.app.last_channels());
            if layout != self.layout {
                layout.write(&self.root);
                self.layout = layout;
            }
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.stop_backend();
    }
}

fn bind_callbacks(window: &TalkWindow, adapter: &Rc<RefCell<Adapter>>) {
    let state = Rc::downgrade(adapter);
    let weak = window.as_weak();
    OPEN.with(|open| {
        *open.borrow_mut() = Some(Box::new(move |room| {
            let (Some(adapter), Some(window)) = (state.upgrade(), weak.upgrade()) else {
                return;
            };
            let _ = window.show();
            window
                .window()
                .with_winit_window(|winit| winit.focus_window());
            window.set_settings_open(false);
            adapter.borrow_mut().action(&window, Action::Open(room));
        }));
    });
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
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_send(move || {
            let Some(window) = weak.upgrade() else { return };
            // Sending goes to the newest message, as Discord does, so the sent one shows.
            if window.get_can_send() {
                window.invoke_scroll_to_latest();
            }
            adapter.borrow_mut().action(&window, Action::Send);
        });
    }
    bind!(on_earlier, Action::Earlier);
    bind!(on_accept_request, Action::AnswerRequest(true));
    bind!(on_enlarge_profile, Action::EnlargeProfile);
    bind!(on_close_profile, Action::CloseProfile);
    bind!(on_ignore_request, Action::AnswerRequest(false));
    bind!(on_leave_voice, Action::Voice(voice::Command::Leave));
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_befriend(move |user| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Befriend(user.to_string()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_remove_friend_request(move |user| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::RemoveFriendRequest(user.to_string()));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_start_call(move || {
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
    window.on_answer_call(move |room, accept| {
        if let Some(window) = weak.upgrade() {
            let room = room.to_string();
            state.borrow_mut().action(
                &window,
                if accept {
                    Action::AcceptCall(room)
                } else {
                    Action::DeclineCall(room)
                },
            );
        }
    });
    bind!(on_confirm_switch, Action::ConfirmVoiceSwitch);
    bind!(on_cancel_switch, Action::CancelVoiceSwitch);
    bind!(on_dismiss_voice_error, Action::DismissVoiceError);
    bind!(on_refresh_devices, Action::Voice(voice::Command::Devices));
    bind!(on_confirm_delete_message, Action::ConfirmDelete);
    bind!(on_cancel_delete, Action::CancelDelete);
    bind!(on_cancel_compose, Action::CancelCompose);
    bind!(on_close_forward, Action::CloseForward);
    bind!(on_dismiss_notice, Action::DismissNotice);
    bind!(on_close_image, Action::CloseImage);
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_view_image(move |item, index| {
            if let Some(window) = weak.upgrade() {
                let action = Action::ViewImage {
                    item: item.into(),
                    index: usize::try_from(index).unwrap_or_default(),
                };
                adapter.borrow_mut().action(&window, action);
            }
        });
    }
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_step_image(move |step| {
            if let Some(window) = weak.upgrade() {
                adapter
                    .borrow_mut()
                    .action(&window, Action::StepImage(step as isize));
            }
        });
    }
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
    bind_item!(on_jump_to, Action::Jump);
    bind_item!(on_show_dm_profile, Action::ShowDmProfile);
    bind_item!(
        on_show_profile,
        (|user| Action::ShowProfile { user, large: false })
    );
    bind_item!(on_edit, Action::Edit);
    {
        let adapter = adapter.clone();
        let weak = window.as_weak();
        window.on_edit_last(move || {
            let Some(window) = weak.upgrade() else { return };
            adapter.borrow_mut().action(&window, Action::EditLast);
            // The message's text, with the cursor after it.
            let end = window.get_draft().len();
            window.invoke_place_composer_caret(i32::try_from(end).unwrap_or(i32::MAX));
        });
    }
    bind_item!(on_request_delete, Action::RequestDelete);
    bind_item!(on_retry, Action::Retry);
    bind_item!(on_cancel_send, Action::CancelSend);
    bind_item!(on_start_forward, Action::StartForward);
    bind_item!(on_forward_to, Action::Forward);
    bind!(on_open_new_dm, Action::OpenNewDm);
    bind!(on_open_add_friend, Action::OpenAddFriend);
    bind!(on_close_add_friend, Action::CloseAddFriend);
    bind_item!(on_add_friend, Action::AddFriend);
    bind!(on_open_status, Action::OpenStatusEditor);
    bind!(on_close_status, Action::CloseStatusEditor);
    bind!(on_clear_status, Action::ClearCustomStatus);
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_save_status(move || {
        let Some(window) = weak.upgrade() else { return };
        let key = window.get_status_emoji().key.to_string();
        let action = Action::SetCustomStatus {
            emoji: (!key.is_empty()).then_some(key),
            text: window.get_status_text().to_string(),
            clear_after: match window.get_status_clear_after() {
                1 => ClearAfter::Minutes(30),
                2 => ClearAfter::Minutes(60),
                3 => ClearAfter::Minutes(240),
                4 => ClearAfter::Today,
                _ => ClearAfter::Never,
            },
        };
        state.borrow_mut().action(&window, action);
    });
    bind!(on_close_role, Action::CloseRole);
    bind!(on_close_new_dm, Action::CloseNewDm);
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_new_dm_search(move || {
        if let Some(window) = weak.upgrade() {
            let query = window.get_new_dm_query().to_string();
            state
                .borrow_mut()
                .action(&window, Action::NewDmQuery(query));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_start_dm(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let contact = usize::try_from(index).ok().and_then(|index| {
            adapter
                .app
                .state
                .new_dm
                .as_ref()
                .and_then(|new_dm| new_dm.people().nth(index).cloned())
        });
        if let Some(contact) = contact {
            adapter.action(&window, Action::StartDm(contact));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_forward_search(move || {
        if let Some(window) = weak.upgrade() {
            state.borrow_mut().refresh(&window);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_emoji_search(move |query| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let custom = adapter.app.usable_emoji();
        // Opening the picker fetches the custom emoji's pictures.
        if query.is_empty() {
            let pictures = custom
                .iter()
                .flat_map(|set| &set.emoji)
                .map(|emoji| emoji.picture.clone())
                .take(MAX_PICKER_EMOJI)
                .collect();
            adapter.action(&window, Action::LoadPictures(pictures));
        }
        update_model(
            &adapter.models.emoji_rows,
            emoji::rows(&query, &adapter.app.frequent_reactions(16), &custom),
        );
    });
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
    window.on_sidebar_resized(move |width| {
        let mut adapter = state.borrow_mut();
        adapter.layout.sidebar_width = Some(width);
        adapter.layout.write(&adapter.root);
    });
    let state = adapter.clone();
    window.on_member_list_toggled(move |shown| {
        let mut adapter = state.borrow_mut();
        adapter.layout.members_hidden = !shown;
        adapter.layout.write(&adapter.root);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_show_hidden_channels_toggled(move |shown| {
        let mut adapter = state.borrow_mut();
        adapter.layout.show_hidden_channels = shown;
        adapter.layout.write(&adapter.root);
        if let Some(window) = weak.upgrade() {
            adapter.action(&window, Action::ShowHiddenChannels(shown));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_reveal_attachment(move |item, index| {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        adapter.revealed.insert(format!("{item}/a{index}"));
        adapter.restyle = true;
        adapter.refresh(&window);
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_open_link(move |link| {
        // A blocked message shows when asked for.
        if let Some(item) = link.strip_prefix(BLOCKED_LINK) {
            let Some(window) = weak.upgrade() else { return };
            let mut adapter = state.borrow_mut();
            adapter.revealed.insert(format!("{item}/blocked"));
            adapter.restyle = true;
            adapter.refresh(&window);
            return;
        }
        // A spoiler shows what it hides.
        if let Some((item, index)) = spoiler::target(&link) {
            let Some(window) = weak.upgrade() else { return };
            let mut adapter = state.borrow_mut();
            adapter.revealed.insert(format!("{item}/{index}"));
            adapter.restyle = true;
            adapter.refresh(&window);
            return;
        }
        // A mention opens the person's card, or the role's members, beside the
        // mention, or by the pointer when the link's place isn't known.
        if let Some(target) = mention::target(&link) {
            let Some(window) = weak.upgrade() else { return };
            if let Some(rect) = i_slint_core::talk_selection::clicked_link() {
                window.invoke_anchor_link(
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height,
                );
            }
            let action = match target {
                mention::Target::User(user) => {
                    window.set_profile_at_pointer(true);
                    Action::ShowProfile { user, large: false }
                }
                mention::Target::Role(role) => Action::ShowRole(role),
            };
            state.borrow_mut().action(&window, action);
            return;
        }
        // Only web pages open; a message can't start other programs.
        if links::openable(&link)
            && let Err(error) = webbrowser::open(&link)
        {
            tracing::warn!(%error, "could not open a link");
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_preferences_changed(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        let layout = &mut adapter.layout;
        layout.twelve_hour_clock = window.get_twelve_hour_clock();
        layout.text_size = Some(window.get_text_size());
        layout.link_previews = Some(window.get_link_previews());
        layout.unmute_on_join = window.get_unmute_on_join();
        layout.level_voice = Some(window.get_level_voice());
        talk_core::leveling::set_enabled(window.get_level_voice());
        layout.share_typing = Some(window.get_share_typing());
        layout.notifications = Some(window.get_notifications());
        layout.notification_sound = Some(window.get_notification_sound());
        layout.notification_text = Some(window.get_notification_text());
        adapter.layout.write(&adapter.root);
        adapter.restyle = true;
        adapter.refresh(&window);
    });
    window.on_volume_of(|person, stream| {
        let kind = if stream {
            talk_core::volume::Kind::Stream
        } else {
            talk_core::volume::Kind::Voice
        };
        (talk_core::volume::get(kind, &person) * 100.0).round() as i32
    });
    let state = adapter.clone();
    window.on_volume_changed(move |person, stream, percent, done| {
        let volume = percent as f32 / 100.0;
        let kind = if stream {
            talk_core::volume::Kind::Stream
        } else {
            talk_core::volume::Kind::Voice
        };
        talk_core::volume::set(kind, &person, volume);
        // The drag's last step is remembered, rather than every one.
        if done {
            let mut adapter = state.borrow_mut();
            let volumes = if stream {
                &mut adapter.layout.stream_volumes
            } else {
                &mut adapter.layout.volumes
            };
            if percent == 100 {
                volumes.remove(person.as_str());
            } else {
                volumes.insert(person.into(), volume);
            }
            adapter.layout.write(&adapter.root);
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_sync_discord_reads_toggled(move |sync| {
        let mut adapter = state.borrow_mut();
        adapter.layout.sync_discord_reads = Some(sync);
        adapter.layout.write(&adapter.root);
        if let Some(window) = weak.upgrade() {
            adapter.action(&window, Action::SyncDiscordReads(sync));
        }
    });
    let state = adapter.clone();
    window.on_attach(move || {
        let weak = Rc::downgrade(&state);
        state.borrow_mut().dialog(async move {
            let files = rfd::AsyncFileDialog::new()
                .set_title("Attach files")
                .pick_files()
                .await;
            if let (Some(files), Some(adapter)) = (files, weak.upgrade()) {
                let window = adapter.borrow().window.upgrade();
                if let Some(window) = window {
                    let paths = files.iter().map(|file| file.path().to_owned()).collect();
                    adapter.borrow_mut().action(&window, Action::Stage(paths));
                }
            }
        });
    });
    let state = adapter.clone();
    window.on_unstage(move |index| {
        let mut adapter = state.borrow_mut();
        if let (Ok(index), Some(window)) = (usize::try_from(index), adapter.window.upgrade()) {
            adapter.action(&window, Action::Unstage(index));
        }
    });
    let state = adapter.clone();
    window.on_paste_into_composer(move || state.borrow_mut().paste());
    let state = adapter.clone();
    window.on_save_attachment(move |item, index| {
        let item = item.to_string();
        let index = usize::try_from(index).unwrap_or_default();
        let name = state
            .borrow()
            .app
            .state
            .message(&item)
            .and_then(|message| message.attachments.get(index))
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
                            index,
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
    window.on_composer_caret(move |text, offset| {
        let Some(window) = weak.upgrade() else { return };
        // Choosing a suggestion moves the cursor while it's being handled.
        let Ok(mut adapter) = state.try_borrow_mut() else {
            return;
        };
        // The cursor moves before the edit is reported; take the text first, so the
        // refresh below doesn't put back the text before it.
        if adapter.app.state.draft() != text.as_str() {
            adapter.action(&window, Action::SetDraft(text.to_string()));
        }
        let found = usize::try_from(offset)
            .ok()
            .and_then(|offset| mention::query(&text, offset));
        if found.is_none() {
            adapter.mention_dismissed = None;
        }
        let found = found.filter(|(at, _)| adapter.mention_dismissed != Some(*at));
        adapter.mention_at = found.map(|(at, _)| at);
        let query = found.map(|(_, query)| query.to_owned());
        if adapter.app.state.mention_query != query {
            window.set_mention_index(0);
            adapter.action(&window, Action::MentionQuery(query));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_mention_chosen(move |index| {
        let Some(window) = weak.upgrade() else { return };
        let caret = {
            let mut adapter = state.borrow_mut();
            let chosen = usize::try_from(index).ok().and_then(|index| {
                adapter
                    .app
                    .state
                    .mention_suggestions()
                    .into_iter()
                    .nth(index)
            });
            let (Some(chosen), Some(at), Some(query)) = (
                chosen,
                adapter.mention_at,
                adapter.app.state.mention_query.clone(),
            ) else {
                return;
            };
            let text = adapter.app.state.draft().to_owned();
            let end = at + 1 + query.len();
            if text.get(at..end) != Some(format!("@{query}").as_str()) {
                return;
            }
            let inserted = format!("@{} ", chosen.name);
            let text = format!("{}{inserted}{}", &text[..at], &text[end..]);
            adapter.mention_at = None;
            adapter.action(&window, Action::MentionQuery(None));
            adapter.action(&window, Action::SetDraft(text));
            at + inserted.len()
        };
        window.invoke_place_composer_caret(i32::try_from(caret).unwrap_or(i32::MAX));
        window.invoke_focus_composer();
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_mention_dismissed(move || {
        let Some(window) = weak.upgrade() else { return };
        let mut adapter = state.borrow_mut();
        adapter.mention_dismissed = adapter.mention_at.take();
        adapter.action(&window, Action::MentionQuery(None));
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
    window.on_toggle_share(move || {
        if let Some(window) = weak.upgrade() {
            let command = {
                let state = state.borrow();
                let voice = &state.app.state.voice;
                if voice.sharing.is_some() {
                    voice::Command::StopLive
                } else {
                    voice::Command::GoLive(voice.quality)
                }
            };
            state.borrow_mut().action(&window, Action::Voice(command));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_go_live(move |resolution, fps| {
        if let Some(window) = weak.upgrade() {
            let resolution = match resolution {
                0 => voice::Resolution::P720,
                2 => voice::Resolution::Source,
                _ => voice::Resolution::P1080,
            };
            let quality = voice::StreamQuality {
                resolution,
                fps: fps.clamp(1, 60) as u32,
            };
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::GoLive(quality)));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_watch_stream(move |id| {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::Watch(id.into())));
        }
    });
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_stop_watching(move || {
        if let Some(window) = weak.upgrade() {
            state
                .borrow_mut()
                .action(&window, Action::Voice(voice::Command::StopWatching));
        }
    });
    // Frames skip the application state, which would redraw far more than the pictures.
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_stream_tick(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let adapter = state.borrow();
        let voice = &adapter.app.state.voice;
        let image = |width, height, pixels: &[u8]| {
            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                pixels, width, height,
            ))
        };
        if let Some(frame) = voice
            .sharing
            .as_ref()
            .and_then(|sharing| sharing.frames.take(image))
        {
            window.set_share_frame(frame);
        }
        if let Some(frame) = voice
            .watching
            .as_ref()
            .and_then(|watching| watching.frames.as_ref())
            .and_then(|frames| frames.take(image))
        {
            window.set_watch_frame(frame);
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
                let mut adapter = state.borrow_mut();
                if adapter.meter.is_some() {
                    adapter.meter = Some(InputMeter::start(Some(&id)));
                }
                adapter.action(&window, Action::Voice(voice::Command::Microphone(id)));
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
    let state = adapter.clone();
    window.on_metering(move |on| {
        let mut adapter = state.borrow_mut();
        let microphone = adapter.app.state.voice.microphone.clone();
        adapter.meter = on.then(|| InputMeter::start(microphone.as_deref()));
    });
    // The level skips the application state, which would redraw far more than the meter.
    let state = adapter.clone();
    let weak = window.as_weak();
    window.on_meter_tick(move || {
        if let (Some(window), Some(meter)) = (weak.upgrade(), &state.borrow().meter) {
            window.set_input_level(meter.level());
            window.set_meter_note(SharedString::from(if !meter::AVAILABLE {
                "The level meter isn't available on this system yet."
            } else if meter.failed() {
                "Talk can't read this microphone. Check that it's connected, or choose another."
            } else {
                ""
            }));
        }
    });
}

/// Only mentions get a number; plain unread activity is a dot, because sync
/// does not deliver every message of a room that isn't open.
/// "Ann is", "Ann and Bob are", or "Ann, Bob and 3 others are".
/// What a profile or DM offers for a Discord friendship, as in
/// ProfileData.friend.
fn friend_offer(relation: Relation) -> i32 {
    match relation {
        Relation::Stranger => 1,
        Relation::Outgoing => 2,
        Relation::Incoming => 3,
        Relation::Me | Relation::Friend => 0,
    }
}

/// Shows a DM call ringing this account in another conversation over
/// everything. Returns whether any call rings it, which Talk rings for.
fn show_incoming(
    window: &TalkWindow,
    state: &talk_core::application::AppState,
    pictures: &mut HashMap<String, slint::Image>,
) -> bool {
    let voice = &state.voice;
    let ringing: Vec<_> = state
        .rooms
        .iter()
        .filter(|room| room.incoming && !(voice.active() && voice.room.as_ref() == Some(&room.id)))
        .collect();
    // The open DM's own call answers in its panel.
    let shown = ringing
        .iter()
        .find(|room| state.selected.as_ref() != Some(&room.id));
    window.set_incoming_open(shown.is_some());
    if let Some(room) = shown {
        window.set_incoming_room(room.id.as_str().into());
        window.set_incoming_name(room.name.as_str().into());
        window.set_incoming_detail(
            if room.person.is_some() || room.people.is_empty() {
                "Incoming call".into()
            } else {
                format!("{} calling", who_is_here(&room.people))
            }
            .into(),
        );
        window.set_incoming_initial(initial(&room.name));
        window.set_incoming_tint(tint(&room.id));
        window.set_incoming_picture(cached_picture(pictures, state, room.avatar.as_deref()));
    }
    !ringing.is_empty()
}

fn who_is_here(people: &[talk_core::chat::VoicePerson]) -> String {
    let names: Vec<&str> = people.iter().map(|person| person.name.as_str()).collect();
    match names.as_slice() {
        [one] => format!("{one} is"),
        [first, second] => format!("{first} and {second} are"),
        [first, second, third] => format!("{first}, {second} and {third} are"),
        [first, second, rest @ ..] => format!("{first}, {second} and {} others are", rest.len()),
        [] => String::new(),
    }
}

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
/// The custom status as the account panel shows it: its emoji, then its words.
fn status_line(status: &talk_core::chat::CustomStatus) -> String {
    let emoji = match (&status.emoji, &status.emoji_id) {
        (_, Some(id)) => custom_emoji::glyph(id).map(String::from),
        (Some(emoji), None) => Some(emoji.clone()),
        (None, None) => None,
    };
    match emoji {
        Some(emoji) if status.text.is_empty() => emoji,
        Some(emoji) => format!("{emoji} {}", status.text),
        None => status.text.clone(),
    }
}

/// The picker's key for the custom status's emoji: the emoji, or `name:id`.
fn status_key(status: &talk_core::chat::CustomStatus) -> Option<String> {
    match (&status.emoji, &status.emoji_id) {
        (Some(name), Some(id)) => Some(format!("{name}:{id}")),
        (Some(emoji), None) => Some(emoji.clone()),
        _ => None,
    }
}

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

/// The local calendar date of a timestamp, such as "3 October 2026"; empty when unknown.
fn day(timestamp: u64) -> String {
    talk_core::clock::day(timestamp)
}

use talk_core::chat::file_size;

/// The picture at `url` once it has arrived, converted for Slint only once.
/// Reads a picture waiting in the composer off the UI thread, then shows its preview.
fn load_thumbnail(
    runtime: &BackgroundRuntime,
    weak: std::rc::Weak<RefCell<Adapter>>,
    path: PathBuf,
) {
    let reading = {
        let path = path.clone();
        runtime.spawn_blocking(move || {
            std::fs::read(&path)
                .ok()
                .and_then(|data| talk_core::chat::decode_picture(&data, 240, 240))
        })
    };
    let task = slint::spawn_local(async move {
        let picture = reading.await.ok().flatten();
        let Some(adapter) = weak.upgrade() else {
            return;
        };
        let window = adapter.borrow().window.upgrade();
        let mut adapter = adapter.borrow_mut();
        // Unless it was taken out of the composer meanwhile.
        let Some(slot) = adapter.thumbnails.get_mut(&path) else {
            return;
        };
        *slot = picture.map(|picture| {
            slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(
                &picture.rgba,
                picture.width,
                picture.height,
            ))
        });
        if let Some(window) = window {
            adapter.refresh(&window);
        }
    });
    if let Err(error) = task {
        tracing::error!(%error, "could not preview a file to send");
    }
}

fn cached_picture(
    cache: &mut HashMap<String, slint::Image>,
    state: &talk_core::application::AppState,
    url: Option<&str>,
) -> slint::Image {
    let Some(url) = url else {
        return slint::Image::default();
    };
    if let Some(image) = cache.get(url) {
        return image.clone();
    }
    let Some(arrived) = state.pictures.get(url) else {
        return slint::Image::default();
    };
    let image = picture(arrived);
    cache.insert(url.to_owned(), image.clone());
    image
}

/// A full image's size once shrunk to the viewer's largest.
fn fit(width: u32, height: u32) -> (u32, u32) {
    use talk_core::chat::{VIEW_HEIGHT, VIEW_WIDTH};
    let scale = f64::min(
        1.0,
        f64::min(
            f64::from(VIEW_WIDTH) / f64::from(width.max(1)),
            f64::from(VIEW_HEIGHT) / f64::from(height.max(1)),
        ),
    );
    let side = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
    (side(width), side(height))
}

fn picture(picture: &Picture) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &picture.rgba,
        picture.width,
        picture.height,
    );
    slint::Image::from_rgba8(buffer)
}

thread_local! {
    /// Animations' frames handed to Slint, by the picture they're from, so a
    /// row built again shares them.
    static FRAMES: RefCell<Vec<(std::sync::Weak<Picture>, ModelRc<slint::Image>)>> =
        RefCell::default();
}

/// An animation's frames for Slint and how long each shows, in milliseconds;
/// both empty for a still picture.
fn animation(picture: &Arc<Picture>) -> (ModelRc<slint::Image>, ModelRc<i32>) {
    if picture.frames.len() < 2 {
        return (ModelRc::default(), ModelRc::default());
    }
    let delays = ModelRc::new(VecModel::from(
        picture
            .frames
            .iter()
            .map(|frame| i32::try_from(frame.delay_ms).unwrap_or(i32::MAX))
            .collect::<Vec<_>>(),
    ));
    let frames = FRAMES.with_borrow_mut(|cache| {
        cache.retain(|(source, _)| source.strong_count() > 0);
        if let Some((_, frames)) = cache
            .iter()
            .find(|(source, _)| std::ptr::eq(source.as_ptr(), Arc::as_ptr(picture)))
        {
            return frames.clone();
        }
        let frames = ModelRc::new(VecModel::from(
            picture
                .frames
                .iter()
                .map(|frame| {
                    slint::Image::from_rgba8(
                        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                            &frame.rgba,
                            picture.width,
                            picture.height,
                        ),
                    )
                })
                .collect::<Vec<_>>(),
        ));
        cache.push((Arc::downgrade(picture), frames.clone()));
        frames
    });
    (frames, delays)
}

/// A picture shrunk to a few pixels, which Slint smooths into a blur when it's
/// drawn at full size: a spoiler's picture before it's revealed.
fn blurred(picture: &Picture) -> slint::Image {
    const SIDE: u32 = 10;
    let scale = (picture.width.max(picture.height) / SIDE).max(1);
    let (width, height) = (
        picture.width.div_ceil(scale).max(1),
        picture.height.div_ceil(scale).max(1),
    );
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = [0u32; 4];
            let mut count = 0;
            for sy in y * scale..((y + 1) * scale).min(picture.height) {
                for sx in x * scale..((x + 1) * scale).min(picture.width) {
                    let at = ((sy * picture.width + sx) * 4) as usize;
                    for (channel, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(picture.rgba[at + channel]);
                    }
                    count += 1;
                }
            }
            rgba.extend(sum.map(|total| (total / count.max(1)) as u8));
        }
    }
    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&rgba, width, height);
    slint::Image::from_rgba8(buffer)
}

/// A message's previews laid out: one at its own size, several as square
/// tiles two to a row, together as wide as one preview.
fn gallery(
    previews: &[(usize, &talk_core::chat::Attachment, &Arc<Picture>)],
    covered: &dyn Fn(usize) -> bool,
) -> (Vec<ImageCell>, i32, i32) {
    const TILE: i32 = 178;
    const GAP: i32 = 4;
    let cell = |index: usize, attachment: &talk_core::chat::Attachment, preview: &Arc<Picture>| {
        let spoiler = attachment.spoiler && covered(index);
        if spoiler {
            return ImageCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                image: blurred(preview),
                spoiler,
                ..Default::default()
            };
        }
        let (frames, delays) = animation(preview);
        ImageCell {
            index: index as i32,
            name: attachment.name.clone().into(),
            image: picture(preview),
            frames,
            delays,
            ..Default::default()
        }
    };
    match previews {
        [] => (vec![], 0, 0),
        [(index, attachment, preview)] => {
            let (width, height) = (preview.width as i32, preview.height as i32);
            let image = ImageCell {
                width,
                height,
                ..cell(*index, attachment, preview)
            };
            (vec![image], width, height)
        }
        _ => {
            let cells = previews
                .iter()
                .enumerate()
                .map(|(place, (index, attachment, preview))| ImageCell {
                    x: (place % 2) as i32 * (TILE + GAP),
                    y: (place / 2) as i32 * (TILE + GAP),
                    width: TILE,
                    height: TILE,
                    ..cell(*index, attachment, preview)
                })
                .collect();
            let rows = previews.len().div_ceil(2) as i32;
            (cells, 2 * TILE + GAP, rows * TILE + (rows - 1) * GAP)
        }
    }
}

/// How the UI names a presence; empty when there's none.
fn presence_name(presence: Option<Presence>) -> &'static str {
    match presence {
        Some(Presence::Online) => "online",
        Some(Presence::Idle) => "idle",
        Some(Presence::DoNotDisturb) => "dnd",
        Some(Presence::Offline) => "offline",
        None => "",
    }
}

/// The width role chips fill in the profile card and the full profile: the
/// cards' widths less their padding.
const PROFILE_CARD_WIDTH: f32 = 300. - 2. * 16.;
const PROFILE_LARGE_WIDTH: f32 = 600. - 2. * 24.;

/// Roles as chips, a row at a time. Slint has no wrapping layout, so chips
/// are measured roughly: Satoshi Medium at 13px averages under 7px a letter.
fn role_rows(roles: &[ProfileRole], width: f32) -> ModelRc<ProfileRoleRow> {
    const GAP: f32 = 4.;
    // Padding, the colour dot and the space after it.
    const CHROME: f32 = 8. + 10. + 6. + 8.;
    let mut rows: Vec<Vec<talk_ui::ProfileRole>> = Vec::new();
    let mut used = width;
    for role in roles {
        let chip = CHROME + 7. * role.name.chars().count() as f32;
        if used + GAP + chip > width || rows.is_empty() {
            rows.push(Vec::new());
            used = chip;
        } else {
            used += GAP + chip;
        }
        if let Some(row) = rows.last_mut() {
            row.push(talk_ui::ProfileRole {
                name: role.name.clone().into(),
                color: role_color(role.color),
                has_color: role.color.is_some(),
            });
        }
    }
    ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|roles| ProfileRoleRow {
                roles: ModelRc::new(VecModel::from(roles)),
            })
            .collect::<Vec<_>>(),
    ))
}

/// Markdown as styled text, with web addresses linked. Slint's Markdown
/// subset rejects some syntax, such as headings; those show as typed,
/// keeping their links.
fn markdown(body: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(&links::linkify(body))
        .ok()
        .or_else(|| plain_with_links(body))
        .unwrap_or_else(|| slint::StyledText::from_plain_text(body))
}

/// An embed's title in bold, linking to its page when it has one.
fn embed_title(title: &str, url: &str) -> slint::StyledText {
    let escaped = escape_markdown(title);
    let markdown = if url.is_empty() {
        format!("**{escaped}**")
    } else {
        format!("**[{escaped}](<{url}>)**")
    };
    slint::StyledText::from_markdown(&markdown)
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(title))
}

/// `text` with Markdown's punctuation escaped by backslashes, so it shows as typed.
fn escape_markdown(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escape = c.is_ascii_punctuation().then_some('\\');
            escape.into_iter().chain([c])
        })
        .collect()
}

/// `text` in bold, as typed.
fn bold(text: &str) -> slint::StyledText {
    slint::StyledText::from_markdown(&format!("**{}**", escape_markdown(text)))
        .unwrap_or_else(|_| slint::StyledText::from_plain_text(text))
}

/// An embed's fields in rows: up to three inline fields share one, and any
/// other field takes a row of its own, as in Discord.
fn field_rows(fields: &[EmbedField]) -> ModelRc<EmbedFieldRow> {
    let mut rows: Vec<Vec<talk_ui::EmbedField>> = Vec::new();
    let mut open = false;
    for field in fields {
        let cell = talk_ui::EmbedField {
            name: bold(&field.name),
            value: markdown(&field.value),
        };
        match rows.last_mut() {
            Some(row) if open && field.inline && row.len() < 3 => row.push(cell),
            _ => rows.push(vec![cell]),
        }
        open = field.inline;
    }
    ModelRc::new(VecModel::from(
        rows.into_iter()
            .map(|fields| EmbedFieldRow {
                fields: ModelRc::new(VecModel::from(fields)),
            })
            .collect::<Vec<_>>(),
    ))
}

/// Plain text with its web addresses linked; none when it has none.
fn plain_with_links(body: &str) -> Option<slint::StyledText> {
    links::plain_with_links(body)
        .and_then(|markdown| slint::StyledText::from_markdown(&markdown).ok())
}

/// A Discord role's RGB colour for Slint.
fn role_color(rgb: Option<u32>) -> slint::Color {
    rgb.map_or_else(slint::Color::default, |rgb| {
        slint::Color::from_rgb_u8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    })
}

/// The most custom emoji the picker fetches pictures for at once.
const MAX_PICKER_EMOJI: usize = 600;

/// Scrolls the timeline to the message at `index` and highlights it for a moment. The
/// list builds rows as they come into view, so the row settles its place a few times
/// as those around it measure.
fn show_message(window: &TalkWindow, index: usize, item: &str) {
    // At once, so the list stops following the newest message before anything
    // else scrolls it.
    window.invoke_show_message(i32::try_from(index).unwrap_or(i32::MAX), item.into());
    for delay in [0, 30, 100, 250] {
        let weak = window.as_weak();
        slint::Timer::single_shot(Duration::from_millis(delay), move || {
            if let Some(window) = weak.upgrade() {
                window.invoke_settle_jump();
            }
        });
    }
    let weak = window.as_weak();
    let shown: SharedString = item.into();
    slint::Timer::single_shot(Duration::from_millis(2500), move || {
        if let Some(window) = weak.upgrade()
            && window.get_jump_item() == shown
        {
            window.set_jump_item(SharedString::default());
        }
    });
}

/// A message as the timeline shows it. `revealed` holds the spoilers clicked, as
/// `{item}/{index}` for text and `{item}/a{index}` for attachments.
fn message_row(message: &ChatMessage, revealed: &HashSet<String>) -> MessageRow {
    // Plain text shows as typed, its web addresses linked.
    // Mentions link to who they mention, which opens their card.
    // Custom emoji show as their glyphs once they have them.
    let body = custom_emoji::text(&message.body, &message.emoji);
    let jumbo = message.formatted && custom_emoji::only_emoji(&body);
    let styled = if message.formatted {
        let linked = mention::linked(&body, &message.mentions);
        if message.sender.starts_with("discord:") {
            markdown(&spoiler::mark(&linked, &message.item, |index| {
                revealed.contains(&format!("{}/{index}", message.item))
            }))
        } else {
            markdown(&linked)
        }
    } else {
        plain_with_links(&message.body)
            .unwrap_or_else(|| slint::StyledText::from_plain_text(&message.body))
    };
    // An image with a preview shows in the gallery; any other attachment shows
    // a file card. Embeds' pictures show in their cards.
    let previews =
        |placement: Placement| -> Vec<(usize, &talk_core::chat::Attachment, &Arc<Picture>)> {
            message
                .attachments
                .iter()
                .enumerate()
                .filter(|(_, attachment)| attachment.placement == placement)
                .filter_map(|(index, attachment)| {
                    Some((index, attachment, attachment.preview.as_ref()?))
                })
                .collect()
        };
    let covered = |index: usize| !revealed.contains(&format!("{}/a{index}", message.item));
    let (images, gallery_width, gallery_height) = gallery(&previews(Placement::Message), &covered);
    let stickers: Vec<ImageCell> = previews(Placement::Sticker)
        .into_iter()
        .map(|(index, attachment, preview)| {
            let (frames, delays) = animation(preview);
            ImageCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                image: picture(preview),
                width: preview.width as i32,
                height: preview.height as i32,
                frames,
                delays,
                ..Default::default()
            }
        })
        .collect();
    let embeds: Vec<EmbedCard> = message
        .embeds
        .iter()
        .enumerate()
        .map(|(index, embed)| {
            let (images, gallery_width, gallery_height) =
                gallery(&previews(Placement::Embed(index)), &covered);
            let thumbnail = previews(Placement::Thumbnail(index)).into_iter().next();
            EmbedCard {
                color: role_color(embed.color),
                has_color: embed.color.is_some(),
                provider: slint::StyledText::from_plain_text(&embed.provider),
                has_provider: !embed.provider.is_empty(),
                author: bold(&embed.author),
                has_author: !embed.author.is_empty(),
                title: embed_title(&embed.title, &embed.url),
                has_title: !embed.title.is_empty(),
                url: embed.url.clone().into(),
                description: markdown(&embed.description),
                has_description: !embed.description.is_empty(),
                field_rows: field_rows(&embed.fields),
                footer: slint::StyledText::from_plain_text(&embed.footer),
                has_footer: !embed.footer.is_empty(),
                thumbnail: thumbnail
                    .map(|(_, _, preview)| picture(preview))
                    .unwrap_or_default(),
                thumbnail_index: thumbnail.map_or(-1, |(index, _, _)| index as i32),
                has_thumbnail: thumbnail.is_some(),
                images: ModelRc::new(VecModel::from(images)),
                gallery_width,
                gallery_height,
                video: embed.video,
            }
        })
        .collect();
    let files: Vec<FileCell> = message
        .attachments
        .iter()
        .enumerate()
        .filter(|(_, attachment)| {
            attachment.preview.is_none() && attachment.placement == Placement::Message
        })
        .map(|(index, attachment)| {
            let size = attachment.size.map(file_size);
            let detail = match (attachment.image, attachment.preview_failed, size) {
                (true, true, Some(size)) => format!("{size} · Preview unavailable"),
                (true, true, None) => "Preview unavailable".into(),
                (_, _, Some(size)) => size,
                _ => String::new(),
            };
            FileCell {
                index: index as i32,
                name: attachment.name.clone().into(),
                detail: detail.into(),
            }
        })
        .collect();
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
    let row = MessageRow {
        id: message.id.clone().into(),
        item: message.item.clone().into(),
        author: message.author.clone().into(),
        color: role_color(message.color),
        has_color: message.color.is_some(),
        initial: initial(&message.author),
        tint: tint(&message.author),
        // Set from the picture cache when the timeline is built.
        picture: slint::Image::default(),
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
        jumbo,
        styled,
        author_styled: bold(&message.author),
        time_styled: slint::StyledText::from_plain_text(&message.time),
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
        reply_item: message
            .reply
            .as_ref()
            .and_then(|reply| reply.item.clone())
            .unwrap_or_default()
            .into(),
        reactions: ModelRc::new(VecModel::from(
            message
                .reactions
                .iter()
                .map(|reaction| ReactionChip {
                    key: reaction.key.clone().into(),
                    label: custom_emoji::text(&reaction.key, &message.emoji).into(),
                    count: i32::try_from(reaction.count).unwrap_or(i32::MAX),
                    mine: reaction.mine,
                })
                .collect::<Vec<_>>(),
        )),
        images: ModelRc::new(VecModel::from(images)),
        gallery_width,
        gallery_height,
        stickers: ModelRc::new(VecModel::from(stickers)),
        files: ModelRc::new(VecModel::from(files)),
        embeds: ModelRc::new(VecModel::from(embeds)),
        hidden_card: SharedString::default(),
        delivery: delivery.into(),
        delivery_failed: message.delivery == Delivery::Failed,
        can_retry,
        can_cancel,
        can_reply: message.can_reply,
        can_edit: message.can_edit,
        can_delete: message.can_delete,
        can_react: message.can_react,
        can_forward: message.can_forward,
    };
    if collapsed(message, revealed) {
        // Discord shows nothing of a blocked message until asked.
        let note = format!(
            "*Blocked message.* [Show it](<{BLOCKED_LINK}{}>)",
            message.item
        );
        return MessageRow {
            author: "Blocked user".into(),
            author_styled: bold("Blocked user"),
            initial: "?".into(),
            tint: Default::default(),
            color: Default::default(),
            has_color: false,
            body: "Blocked message.".into(),
            styled: slint::StyledText::from_markdown(&note)
                .unwrap_or_else(|_| slint::StyledText::from_plain_text("Blocked message.")),
            has_reply: false,
            reactions: ModelRc::default(),
            images: ModelRc::default(),
            gallery_width: 0,
            gallery_height: 0,
            stickers: ModelRc::default(),
            files: ModelRc::default(),
            embeds: ModelRc::default(),
            can_reply: false,
            can_edit: false,
            can_react: false,
            can_forward: false,
            ..row
        };
    }
    row
}

/// The link that shows a blocked message, followed by its item.
const BLOCKED_LINK: &str = "talk:blocked/";

/// Whether a message shows collapsed, being from someone blocked.
fn collapsed(message: &ChatMessage, revealed: &HashSet<String>) -> bool {
    message.blocked && !revealed.contains(&format!("{}/blocked", message.item))
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

    fn message_row_plain(message: &ChatMessage) -> MessageRow {
        message_row(message, &HashSet::new())
    }

    #[test]
    fn the_composer_sends_on_enter_and_breaks_lines_on_shift_enter() {
        use slint::platform::{Key, WindowEvent};
        let window = one_message("Hi");
        let sent = Rc::new(Cell::new(0));
        let count = sent.clone();
        window.on_send(move || count.set(count.get() + 1));
        window.invoke_focus_composer();
        let press = |text: SharedString| {
            window
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            window
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
        };
        press("a".into());
        window.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
        press(Key::Return.into());
        window.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
        press("b".into());
        assert_eq!(window.get_draft(), "a\nb");
        assert_eq!(sent.get(), 0);
        press(Key::Return.into());
        assert_eq!(sent.get(), 1);
        assert_eq!(
            window.get_draft(),
            "a\nb",
            "sending is the application's to clear"
        );
    }

    #[test]
    fn the_keyboard_steps_through_messages_and_acts_on_them() {
        use slint::platform::{Key, WindowEvent};
        let window = showing(
            ["One", "Two", "Three"]
                .into_iter()
                .map(|body| ChatMessage {
                    author: "Sam".into(),
                    body: body.into(),
                    can_reply: true,
                    ..Default::default()
                })
                .collect(),
        );
        let replied = Rc::new(RefCell::new(String::new()));
        let reply = replied.clone();
        window.on_reply(move |item| *reply.borrow_mut() = item.into());
        let edits = Rc::new(Cell::new(0));
        let count = edits.clone();
        window.on_edit_last(move || count.set(count.get() + 1));
        let key = |text: SharedString, pressed: bool| {
            window.window().dispatch_event(if pressed {
                WindowEvent::KeyPressed { text }
            } else {
                WindowEvent::KeyReleased { text }
            });
        };
        let press = |text: SharedString| {
            key(text.clone(), true);
            key(text, false);
        };
        window.invoke_focus_composer();
        // Up in the empty composer edits; Alt+Up goes to the newest message.
        press(Key::UpArrow.into());
        assert_eq!(edits.get(), 1);
        key(Key::Alt.into(), true);
        press(Key::UpArrow.into());
        key(Key::Alt.into(), false);
        assert_eq!(window.get_active_item(), "i2");
        press(Key::UpArrow.into());
        press(Key::UpArrow.into());
        press(Key::UpArrow.into());
        assert_eq!(window.get_active_item(), "i0", "the first stays");
        press(Key::DownArrow.into());
        assert_eq!(window.get_active_item(), "i1");
        press("r".into());
        assert_eq!(*replied.borrow(), "i1");
        // The reply goes back to the composer, so letters type again.
        press("x".into());
        assert_eq!(window.get_draft(), "x");
    }

    #[test]
    fn preferences_set_the_clock_text_size_previews_and_unmuting() {
        i_slint_backend_testing::init_no_event_loop();
        let message = ChatMessage {
            timestamp: 1_791_329_400,
            time: "23:30".into(),
            embeds: vec![talk_core::chat::Embed::default()],
            ..Default::default()
        };
        // Missing preferences take their defaults: 24 hours, default text, previews.
        let defaults: Layout = serde_json::from_str("{}").unwrap();
        let row = defaults.restyle(&message, message_row(&message, &HashSet::new()));
        assert_eq!(
            row.time,
            talk_core::clock::time_on(message.timestamp, false)
        );
        assert_eq!(row.embeds.row_count(), 1);
        let layout = Layout {
            twelve_hour_clock: true,
            text_size: Some(2),
            link_previews: Some(false),
            unmute_on_join: true,
            ..Default::default()
        };
        let row = layout.restyle(&message, message_row(&message, &HashSet::new()));
        let time = talk_core::clock::time_on(message.timestamp, true);
        assert!(time.ends_with(" AM") || time.ends_with(" PM"));
        assert_eq!(row.time, time);
        assert_eq!(row.time_styled, slint::StyledText::from_plain_text(&time));
        assert_eq!(row.short_time, time.split(' ').next().unwrap());
        assert_eq!(row.embeds.row_count(), 0);
        // A message that is only a card says it's hidden; one with text doesn't.
        assert_eq!(
            row.hidden_card,
            "A card, hidden while link previews are off."
        );
        let titled = ChatMessage {
            embeds: vec![talk_core::chat::Embed {
                title: "Server status".into(),
                ..Default::default()
            }],
            ..message.clone()
        };
        assert!(
            layout
                .restyle(&titled, message_row(&titled, &HashSet::new()))
                .hidden_card
                .contains("\"Server status\"")
        );
        let with_text = ChatMessage {
            body: "Look".into(),
            ..message.clone()
        };
        assert!(
            layout
                .restyle(&with_text, message_row(&with_text, &HashSet::new()))
                .hidden_card
                .is_empty()
        );

        let window = TalkWindow::new().unwrap();
        layout.apply(&window);
        assert!(window.get_twelve_hour_clock() && !window.get_link_previews());
        assert_eq!(window.get_message_font_size(), 18.);
        // Joining a call turns the microphone on once it connects.
        let unmuted = Rc::new(Cell::new(false));
        let record = unmuted.clone();
        window.on_toggle_mute(move || record.set(true));
        window.set_muted(true);
        window.set_voice_ready(true);
        slint::platform::update_timers_and_animations();
        assert!(unmuted.get());
    }

    #[test]
    fn embed_fields_share_rows_three_inline_at_a_time() {
        let field = |name: &str, inline| EmbedField {
            name: name.into(),
            value: String::new(),
            inline,
        };
        let rows = field_rows(&[
            field("a", true),
            field("b", true),
            field("c", true),
            field("d", true),
            field("e", false),
            field("f", true),
        ]);
        let names: Vec<Vec<slint::StyledText>> = rows
            .iter()
            .map(|row| row.fields.iter().map(|field| field.name).collect())
            .collect();
        let expected: Vec<Vec<slint::StyledText>> = [&["a", "b", "c"][..], &["d"], &["e"], &["f"]]
            .iter()
            .map(|row| row.iter().map(|name| bold(name)).collect())
            .collect();
        assert_eq!(names, expected);
    }

    /// A window showing one styled message from Sam, with the composer.
    fn one_message(body: &str) -> TalkWindow {
        messages(&[("Sam", body)])
    }

    /// A window showing styled messages, each starting a group, with the composer.
    fn messages(lines: &[(&str, &str)]) -> TalkWindow {
        showing(
            lines
                .iter()
                .map(|(author, body)| ChatMessage {
                    author: (*author).into(),
                    body: (*body).into(),
                    formatted: true,
                    ..Default::default()
                })
                .collect(),
        )
    }

    /// A window showing these messages, each starting a group and sent at 19:2 and its
    /// index, with the composer.
    fn showing(chat: Vec<ChatMessage>) -> TalkWindow {
        i_slint_backend_testing::init_no_event_loop();
        let window = TalkWindow::new().unwrap();
        copy_whole_messages(&window);
        // Without the adapter's clipboard reading, Ctrl+V pastes text, as it
        // does when the clipboard holds no files or picture.
        let weak = window.as_weak();
        window.on_paste_into_composer(move || weak.unwrap().invoke_paste_composer_text());
        window.set_workspace(true);
        window.set_room_selected(true);
        window.set_room_ready(true);
        window.set_composer_enabled(true);
        let rows: Vec<MessageRow> = chat
            .into_iter()
            .enumerate()
            .map(|(index, message)| {
                message_row_plain(&ChatMessage {
                    id: format!("m{index}"),
                    item: format!("i{index}"),
                    time: format!("19:2{index}"),
                    ..message
                })
            })
            .collect();
        window.set_messages(ModelRc::new(VecModel::from(rows)));
        window.show().unwrap();
        window
            .window()
            .set_size(slint::LogicalSize::new(1120., 760.));
        settle(&window);
        window
    }

    fn settle(window: &TalkWindow) {
        for _ in 0..5 {
            slint::platform::update_timers_and_animations();
            let _ = window.invoke_timeline_extent();
        }
    }

    /// For each height down the window, presses there beside the text, drags up past the top
    /// of the list, turning the wheel `wheel` times on the way, lets go and presses Ctrl+C;
    /// then returns the clipboard. The last drag to start on a message starts at the end of
    /// the lowest, so it selects up to the end of that message. The list returns to the newest
    /// message before each.
    fn copy_upwards(window: &TalkWindow, wheel: usize) -> Option<String> {
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        let dispatch = |event| window.window().dispatch_event(event);
        let at = |x, y| slint::LogicalPosition::new(x, y);
        let button = PointerEventButton::Left;
        for y in (100..700).step_by(4) {
            window.invoke_scroll_to_latest();
            settle(window);
            let y = y as f32;
            dispatch(WindowEvent::PointerMoved {
                position: at(900., y),
            });
            dispatch(WindowEvent::PointerPressed {
                position: at(900., y),
                button,
            });
            dispatch(WindowEvent::PointerMoved {
                position: at(900., y - 8.),
            });
            dispatch(WindowEvent::PointerMoved {
                position: at(900., 60.),
            });
            for _ in 0..wheel {
                dispatch(WindowEvent::PointerScrolled {
                    position: at(900., 60.),
                    delta_x: 0.,
                    delta_y: 400.,
                });
                settle(window);
            }
            dispatch(WindowEvent::PointerReleased {
                position: at(900., 60.),
                button,
            });
            dispatch(WindowEvent::KeyPressed {
                text: Key::Control.into(),
            });
            dispatch(WindowEvent::KeyPressed { text: "c".into() });
            dispatch(WindowEvent::KeyReleased { text: "c".into() });
            dispatch(WindowEvent::KeyReleased {
                text: Key::Control.into(),
            });
        }
        // The composer's single line would turn line breaks into spaces, so
        // read the clipboard itself.
        i_slint_core::window::WindowInner::from_pub(window.window())
            .context()
            .platform()
            .clipboard_text(i_slint_core::platform::Clipboard::DefaultClipboard)
    }

    #[test]
    fn a_selection_runs_across_messages_with_their_names_and_times() {
        let window = messages(&[
            ("Sam", "Thinking **co-op** after nine."),
            ("Riley", "Yep. I should be free after dinner."),
        ]);
        assert_eq!(
            copy_upwards(&window, 0).as_deref(),
            Some(
                "Sam 19:20\nThinking co-op after nine.\nRiley 19:21\nYep. I should be free after dinner."
            )
        );
    }

    #[test]
    fn a_selection_copies_the_pictures_it_runs_through() {
        let picture = Arc::new(Picture {
            width: 40,
            height: 30,
            rgba: vec![128; 40 * 30 * 4],
            ..Default::default()
        });
        let window = showing(vec![
            ChatMessage {
                author: "Sam".into(),
                body: "Look at this.".into(),
                attachments: vec![talk_core::chat::Attachment {
                    name: "cat.png".into(),
                    size: None,
                    image: true,
                    dimensions: Some((400, 300)),
                    preview: Some(picture),
                    preview_failed: false,
                    placement: Placement::Message,
                    spoiler: false,
                }],
                ..Default::default()
            },
            ChatMessage {
                author: "Riley".into(),
                body: "Nice.".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(
            copy_upwards(&window, 0).as_deref(),
            Some("Sam 19:20\nLook at this.\n[Picture]\nRiley 19:21\nNice.")
        );
    }

    #[test]
    fn a_selection_keeps_the_messages_it_scrolls_past() {
        let chat: Vec<ChatMessage> = (0..40)
            .map(|index| ChatMessage {
                author: "Sam".into(),
                body: format!("Message {index}."),
                ..Default::default()
            })
            .collect();
        let window = showing(chat);
        let expected: Vec<String> = (0..40)
            .map(|index| format!("Sam 19:2{index}\nMessage {index}."))
            .collect();
        assert_eq!(copy_upwards(&window, 40), Some(expected.join("\n")));
    }

    #[test]
    fn a_click_on_a_link_in_selectable_text_opens_it() {
        use slint::platform::{PointerEventButton, WindowEvent};
        let window = one_message("See https://example.com/talk");
        let opened = Rc::new(RefCell::new(Vec::<String>::new()));
        let record = opened.clone();
        window.on_open_link(move |link| record.borrow_mut().push(link.into()));
        let button = PointerEventButton::Left;
        // The link starts after "See ", a little way into the text column.
        for y in (100..700).step_by(4) {
            let position = slint::LogicalPosition::new(440., y as f32);
            window
                .window()
                .dispatch_event(WindowEvent::PointerPressed { position, button });
            window
                .window()
                .dispatch_event(WindowEvent::PointerReleased { position, button });
        }
        let opened = opened.borrow();
        assert!(!opened.is_empty(), "a click on the link opens it");
        assert!(opened.iter().all(|link| link == "https://example.com/talk"));
    }

    #[test]
    fn styled_messages_select_by_dragging_and_copy_as_plain_text() {
        use slint::platform::{Key, PointerEventButton, WindowEvent};
        let window = one_message("Thinking **co-op** after nine, if people are up for it.");
        let dispatch = |event| window.window().dispatch_event(event);
        let button = PointerEventButton::Left;
        let at = |x, y| slint::LogicalPosition::new(x, y);
        let ctrl = |key: &str| {
            dispatch(WindowEvent::KeyPressed {
                text: Key::Control.into(),
            });
            dispatch(WindowEvent::KeyPressed { text: key.into() });
            dispatch(WindowEvent::KeyReleased { text: key.into() });
            dispatch(WindowEvent::KeyReleased {
                text: Key::Control.into(),
            });
        };

        // Clicking the message without selecting leaves typing in the composer.
        let click = |x, y| {
            dispatch(WindowEvent::PointerPressed {
                position: at(x, y),
                button,
            });
            dispatch(WindowEvent::PointerReleased {
                position: at(x, y),
                button,
            });
        };
        let type_key = |key: &str| {
            dispatch(WindowEvent::KeyPressed { text: key.into() });
            dispatch(WindowEvent::KeyReleased { text: key.into() });
        };
        click(600., 712.);
        type_key("a");
        for y in (100..700).step_by(4) {
            click(500., y as f32);
        }
        type_key("b");
        assert_eq!(window.get_draft(), "ab");
        window.set_draft(SharedString::default());

        // Without element lookups in this build, drag down the timeline row by
        // row: from right of the text, past its end, to left of the timeline,
        // past its start. A drag that lands on the message selects all of it,
        // and copies it.
        for y in (100..700).step_by(4) {
            let y = y as f32;
            dispatch(WindowEvent::PointerMoved {
                position: at(900., y),
            });
            dispatch(WindowEvent::PointerPressed {
                position: at(900., y),
                button,
            });
            dispatch(WindowEvent::PointerMoved {
                position: at(600., y),
            });
            dispatch(WindowEvent::PointerMoved {
                position: at(200., y),
            });
            dispatch(WindowEvent::PointerReleased {
                position: at(200., y),
                button,
            });
            ctrl("c");
        }

        // Pasting shows what was copied: the text as read, without its Markdown.
        click(600., 712.);
        ctrl("v");
        assert_eq!(
            window.get_draft(),
            "Thinking co-op after nine, if people are up for it."
        );
    }

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
    fn the_input_caret_opens_its_menu_and_measures_only_while_open() {
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
        adapter
            .borrow_mut()
            .action(&window, Action::RequestJoinVoice("lounge".into()));
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
        // The caret sits right of the microphone toggle in the dock.
        click(206., 732.);
        assert!(adapter.borrow().meter.is_some());
        assert!(window.get_muted());
        click(800., 200.);
        assert!(adapter.borrow().meter.is_none());
        window.hide().unwrap();
    }

    #[test]
    fn dragging_the_sidebar_edge_resizes_it_within_limits_and_remembers_it() {
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
        let drag = |from: f32, to: f32| {
            let window = window.window();
            let button = slint::platform::PointerEventButton::Left;
            let at = |x| slint::LogicalPosition::new(x, 400.);
            window
                .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at(from) });
            window.dispatch_event(slint::platform::WindowEvent::PointerPressed {
                position: at(from),
                button,
            });
            window.dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at(to) });
            window.dispatch_event(slint::platform::WindowEvent::PointerReleased {
                position: at(to),
                button,
            });
        };
        assert_eq!(window.get_sidebar_shown(), 240.);
        drag(312., 362.);
        assert_eq!(window.get_sidebar_shown(), 290.);
        assert_eq!(Layout::read(directory.path()).sidebar_width, Some(290.));
        // The sidebar stops at 380px, and at 180px.
        drag(362., 700.);
        assert_eq!(window.get_sidebar_shown(), 380.);
        drag(452., 0.);
        assert_eq!(window.get_sidebar_shown(), 180.);
        // In a narrow window, dragging keeps 420px for the conversation,
        // beside the member list when it shows.
        window
            .window()
            .set_size(slint::LogicalSize::new(800., 560.));
        drag(252., 700.);
        assert_eq!(window.get_sidebar_shown(), 180.);
        window.set_member_list(false);
        drag(252., 700.);
        assert_eq!(window.get_sidebar_shown(), 308.);
        drag(380., 252.);
        // A new window starts at the stored width.
        let next = TalkWindow::new().unwrap();
        adapter.borrow_mut().attach(&next);
        assert_eq!(next.get_sidebar_width(), 180.);
        window.hide().unwrap();
    }

    #[test]
    fn the_member_list_shows_for_text_channels_and_remembers_being_closed() {
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
        assert!(window.get_members_available() && window.get_member_list());
        let roster = window.get_roster();
        assert_eq!(roster.row_count(), 4);
        assert_eq!(roster.row_data(0).unwrap().heading, "ADMINS — 1");
        assert_eq!(roster.row_data(2).unwrap().heading, "MEMBERS — 2");
        assert!(roster.row_data(3).unwrap().heading.is_empty());
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
        // The toggle ends the conversation header.
        click(1080., 24.);
        assert!(!window.get_member_list());
        assert!(Layout::read(directory.path()).members_hidden);
        click(1080., 24.);
        assert!(window.get_member_list());
        // Voice channels and one-to-one DMs have no member list.
        window.invoke_select_room("lounge".into());
        assert!(!window.get_members_available());
        window.invoke_navigate_dms();
        assert!(!window.get_members_available());
        window.invoke_select_room("party".into());
        assert!(window.get_members_available());
        assert_eq!(window.get_roster().row_count(), 3);
        window.hide().unwrap();
    }

    #[test]
    fn forwarding_and_frequent_reactions_reach_the_window() {
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
        let glyphs =
            |model: ModelRc<EmojiCell>| model.iter().map(|cell| cell.glyph).collect::<Vec<_>>();
        assert_eq!(glyphs(window.get_bar_reactions()), ["👍", "❤️", "😂"]);
        let message = window.get_messages().row_data(0).unwrap();
        window.invoke_react(message.item.clone(), "🍕".into());
        assert_eq!(
            glyphs(window.get_menu_reactions()),
            ["🍕", "👍", "❤️", "😂"]
        );
        assert_eq!(Layout::read(directory.path()).reactions.get("🍕"), Some(&1));

        window.invoke_start_forward(message.item.clone());
        assert!(window.get_forward_open());
        assert_eq!(window.get_forward_preview(), "Anyone around tonight?");
        let targets = window.get_forward_targets();
        assert!(
            targets.iter().all(|target| target.id != "lounge"),
            "voice is no target"
        );
        window.set_forward_query("gam".into());
        window.invoke_forward_search();
        assert_eq!(window.get_forward_targets().row_count(), 1);
        window.invoke_forward_to("games".into());
        assert!(window.get_forward_targets().row_data(0).unwrap().sent);
        window.invoke_close_forward();
        assert!(!window.get_forward_open());

        window.invoke_emoji_search("pizza".into());
        assert_eq!(window.get_emoji_rows().row_count(), 1);
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
    fn scrolling_up_through_styled_messages_stays_up() {
        i_slint_backend_testing::init_no_event_loop();
        let window = TalkWindow::new().unwrap();
        window.set_workspace(true);
        window.set_room_selected(true);
        window.set_room_ready(true);
        let rows: Vec<MessageRow> = (0..200)
            .map(|index| {
                let body = match index % 4 {
                    0 => format!("**Message** {index}"),
                    1 => format!("Message {index} with https://example.com/{index} and words that wrap across the timeline at ordinary widths, and then *some* more"),
                    2 => format!("line one {index}\nline two\nline three"),
                    _ => format!("Message {index}"),
                };
                let mut row = message_row_plain(&ChatMessage {
                    id: format!("m{index}"),
                    item: format!("i{index}"),
                    author: "Bob".into(),
                    body,
                    formatted: true,
                    ..Default::default()
                });
                row.first = index % 5 == 0;
                row
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
        for step in 0..8 {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(700., 380.),
                    delta_x: 0.,
                    delta_y: 50.,
                });
            settle();
            assert!(
                !window.invoke_timeline_at_end() && window.invoke_timeline_below() > 0.,
                "step {step} stays scrolled up"
            );
        }
        // Scrolling back down resumes following, and later rows stay in view.
        for _ in 0..40 {
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
                    position: slint::LogicalPosition::new(700., 380.),
                    delta_x: 0.,
                    delta_y: -200.,
                });
            settle();
        }
        assert!(
            window.invoke_timeline_at_end(),
            "back at the latest message"
        );
        let messages = window.get_messages();
        let model = messages
            .as_any()
            .downcast_ref::<VecModel<MessageRow>>()
            .unwrap();
        model.push(message_row_plain(&ChatMessage {
            id: "new".into(),
            item: "new".into(),
            author: "Ann".into(),
            body: "A *new* message with https://example.com\nand a second line".into(),
            formatted: true,
            ..Default::default()
        }));
        settle();
        assert_eq!(window.invoke_timeline_below(), 0., "the new message shows");
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
        // Clicking a voice channel joins it.
        window.invoke_select_room(voice_room.id.clone().into());
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
        window.set_settings_open(true);
        window.invoke_logout();
        assert!(!window.get_workspace() && !window.get_voice_active());
        assert!(window.get_password().is_empty() && window.get_recovery().is_empty());
        assert!(!window.get_settings_open());
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
        assert!(reply.has_reply && reply.reply_author == "Sam");
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
