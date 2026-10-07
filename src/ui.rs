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
        Action, AddFriend, AppState, Application, ClearAfter, Compose, DmScreen, Effect,
        Navigation, NewDm, ProfileView, Relation,
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

use crate::{custom_emoji, emoji, graphics, zoom};

mod callbacks;
mod messages;
mod pictures;
mod profile;
mod refresh;

use callbacks::bind_callbacks;
use messages::*;
use pictures::*;
use profile::*;
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
    watch_window(&window, &adapter);
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
}

/// The most backend events applied together before the window refreshes, so
/// a long burst still lets the window keep up.
const EVENT_BATCH: usize = 256;

/// How long the zoom level shows after it changes.
const ZOOM_NOTE: Duration = Duration::from_millis(1200);

/// Follows the native window's focus, for notifications, and zooms the window
/// with Ctrl and +, - and 0 or the scroll wheel, keeping the level in the layout.
fn watch_window(window: &TalkWindow, adapter: &Rc<RefCell<Adapter>>) {
    use slint::winit_030::{EventResult, winit::event::WindowEvent};
    let saved = zoom::saved(adapter.borrow().layout.zoom);
    let adapter = Rc::downgrade(adapter);
    // The winit window exists once the event loop runs.
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::ZERO, move || {
        let Some(window) = weak.upgrade() else { return };
        FOCUSED.set(false);
        // The display's own scale factor, which the zoom multiplies.
        let base = Rc::new(Cell::new(window.window().scale_factor()));
        let level = Rc::new(Cell::new(
            saved
                .min(zoom::most(window.window().size(), base.get()))
                .max(zoom::LEVELS[0]),
        ));
        zoom::apply(window.window(), base.get(), level.get());
        let note = Rc::new(slint::Timer::default());
        let mut keys = zoom::Keys::default();
        let weak = window.as_weak();
        window
            .window()
            .on_winit_window_event(move |slint_window, event| {
                if let WindowEvent::Focused(focused) = event {
                    FOCUSED.set(*focused);
                }
                // Moving to a display of another density: Slint takes its scale,
                // and then the zoom goes back on top.
                if let WindowEvent::ScaleFactorChanged { scale_factor, .. } = event
                    && std::env::var("SLINT_SCALE_FACTOR").is_err()
                {
                    base.set(*scale_factor as f32);
                    let (weak, base, level) = (weak.clone(), base.clone(), level.clone());
                    slint::Timer::single_shot(Duration::ZERO, move || {
                        if let Some(window) = weak.upgrade() {
                            zoom::apply(window.window(), base.get(), level.get());
                        }
                    });
                }
                let step = match keys.event(event) {
                    zoom::Outcome::Pass => return EventResult::Propagate,
                    zoom::Outcome::Taken => return EventResult::PreventDefault,
                    zoom::Outcome::Step(step) => step,
                };
                let most = zoom::most(slint_window.size(), base.get());
                let next = zoom::stepped(level.get(), step, most);
                if let Some(window) = weak.upgrade() {
                    window.set_zoom_note(format!("{}%", (next * 100.).round()).into());
                    let weak = weak.clone();
                    note.start(slint::TimerMode::SingleShot, ZOOM_NOTE, move || {
                        if let Some(window) = weak.upgrade() {
                            window.set_zoom_note(SharedString::default());
                        }
                    });
                }
                if next != level.get() {
                    level.set(next);
                    zoom::apply(slint_window, base.get(), next);
                    if let Some(adapter) = adapter.upgrade() {
                        let mut adapter = adapter.borrow_mut();
                        adapter.layout.zoom = Some(next);
                        adapter.layout.write(&adapter.root);
                    }
                }
                EventResult::PreventDefault
            });
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
    /// So does the profile beside a one-to-one DM.
    profile_panel_hidden: bool,
    /// How far the whole window is zoomed, as one of [`zoom::LEVELS`]; 100%
    /// unless changed.
    zoom: Option<f32>,
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
        window.set_profile_panel(!self.profile_panel_hidden);
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
    /// Discord's message requests and spam, on the requests screen.
    message_requests: Rc<VecModel<RoomRow>>,
    spam_rooms: Rc<VecModel<RoomRow>>,
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
    /// Who is in the open DM's call, shown above its messages.
    call_people: Rc<VecModel<PersonRow>>,
    /// Discord friend requests, and friends: all of them, and those around.
    friend_requests: Rc<VecModel<FriendRequestRow>>,
    friends: Rc<VecModel<FriendRow>>,
    friends_online: Rc<VecModel<FriendRow>>,
    microphones: Rc<VecModel<SharedString>>,
    speakers: Rc<VecModel<SharedString>>,
    staged: Rc<VecModel<StagedFile>>,
}

impl Models {
    fn attach(&self, window: &TalkWindow) {
        window.set_servers(ModelRc::from(self.servers.clone()));
        window.set_rooms(ModelRc::from(self.rooms.clone()));
        window.set_message_requests(ModelRc::from(self.message_requests.clone()));
        window.set_spam_rooms(ModelRc::from(self.spam_rooms.clone()));
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
        window.set_call_people(ModelRc::from(self.call_people.clone()));
        window.set_friend_requests(ModelRc::from(self.friend_requests.clone()));
        window.set_friends(ModelRc::from(self.friends.clone()));
        window.set_friends_online(ModelRc::from(self.friends_online.clone()));
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
    /// Linking a Discord account by QR code; dropping it stops the linking.
    #[cfg(feature = "discord")]
    linking: Option<talk_core::runtime::BackgroundTask<()>>,
    /// Counts linking runs, so a replaced run's last words are ignored.
    #[cfg(feature = "discord")]
    link_run: u64,
    /// The code shown for linking, by the address it carries.
    #[cfg(feature = "discord")]
    link_code: Option<(String, slint::Image)>,
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
        app.dispatch(Action::ShowProfilePanel(!layout.profile_panel_hidden));
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
                #[cfg(feature = "discord")]
                linking: None,
                #[cfg(feature = "discord")]
                link_run: 0,
                #[cfg(feature = "discord")]
                link_code: None,
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
                            let mut adapter = adapter.borrow_mut();
                            adapter.apply(event);
                            // What else has arrived meanwhile applies too, before
                            // one refresh: a burst, such as a page of avatars,
                            // lays the window out once rather than per event.
                            for _ in 1..EVENT_BATCH {
                                match events.try_recv() {
                                    Ok(event) => adapter.apply(event),
                                    Err(_) => break,
                                }
                            }
                            if let Some(window) = window {
                                adapter.refresh(&window);
                            }
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
                Effect::LinkDiscord => {
                    #[cfg(feature = "discord")]
                    self.link_discord();
                }
                Effect::StopDiscordLink => {
                    #[cfg(feature = "discord")]
                    {
                        self.linking = None;
                        self.link_run += 1;
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

    /// Starts linking a Discord account, replacing any linking under way. The
    /// work runs on Tokio; its progress reaches the app on Slint's executor.
    #[cfg(feature = "discord")]
    fn link_discord(&mut self) {
        self.link_run += 1;
        let run = self.link_run;
        let (task, mut progress) = talk_core::discord_link::start(&self.runtime);
        self.linking = Some(task);
        let weak = self.weak.clone();
        let task = slint::spawn_local(async move {
            while let Some(step) = progress.recv().await {
                let Some(adapter) = weak.upgrade() else {
                    return;
                };
                let window = adapter.borrow().window.upgrade();
                let mut adapter = adapter.borrow_mut();
                if adapter.link_run != run {
                    return;
                }
                let effects = adapter.app.linking(step);
                adapter.execute(effects);
                if let Some(window) = window {
                    adapter.refresh(&window);
                }
            }
        });
        if let Err(error) = task {
            tracing::error!(%error, "could not follow linking Discord");
            self.linking = None;
        }
    }

    fn action(&mut self, window: &TalkWindow, action: Action) {
        // A keystroke in the composer changes only the composer; the rest of
        // the window stays as it is.
        let composing = matches!(action, Action::SetDraft(_) | Action::MentionQuery(_));
        let effects = self.app.dispatch(action);
        self.execute(effects);
        if composing {
            self.show_composer(window);
        } else {
            self.refresh(window);
        }
    }

    /// Applies a backend event; the caller refreshes the window after.
    fn apply(&mut self, event: Event) {
        let effects = self.app.apply(event);
        self.execute(effects);
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        self.stop_backend();
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
    count_badge(room.mentions)
}

/// A count as a badge shows it: nothing for none, and at most "99+".
fn count_badge(count: u64) -> SharedString {
    match count {
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

use talk_core::chat::file_size;

thread_local! {
    /// Animations' frames handed to Slint, by the picture they're from, so a
    /// row built again shares them.
    static FRAMES: RefCell<Vec<(std::sync::Weak<Picture>, ModelRc<slint::Image>)>> =
        RefCell::default();
}

/// An attachment's place in a message, the attachment, and its preview once
/// it has arrived.
type Preview<'a> = (
    usize,
    &'a talk_core::chat::Attachment,
    Option<&'a Arc<Picture>>,
);

/// The size Discord shows stickers at, as their pictures are made.
const STICKER_SIZE: i32 = 160;

/// The line under a Discord DM in the list: the person's custom status or
/// what they're doing on one line, or a group's size.
fn dm_detail(room: &talk_core::chat::ChatRoom) -> String {
    if !room.direct || !discord::is_discord(&room.id) {
        return String::new();
    }
    if room.person.is_none() {
        return format!("{} members", room.members);
    }
    room.activity.as_deref().map(one_line).unwrap_or_default()
}

/// A count with its noun, such as "1 member" or "12 members".
fn count_of(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// A status on one line, as Discord shows it under a name; the profile keeps
/// its own line breaks.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// How the UI names a presence; empty when there's none.
/// `new`, unless it has the same rows as `old`, which then stays. Slint tells
/// lists apart by identity, so a row or profile holding an equal but new list
/// would count as changed and be laid out again, losing its hover and focus.
fn same_list<T: Clone + PartialEq + 'static>(old: &ModelRc<T>, new: ModelRc<T>) -> ModelRc<T> {
    let equal = old.row_count() == new.row_count() && old.iter().eq(new.iter());
    if equal { old.clone() } else { new }
}

/// The width role chips fill in the profile card and the full profile: the
/// cards' widths less their padding.
const PROFILE_CARD_WIDTH: f32 = 300. - 2. * 16.;
const PROFILE_LARGE_WIDTH: f32 = 600. - 2. * 24.;

/// The most custom emoji the picker fetches pictures for at once.
const MAX_PICKER_EMOJI: usize = 600;

/// The link that shows a blocked message, followed by its item.
const BLOCKED_LINK: &str = "talk:blocked/";

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
mod tests;
