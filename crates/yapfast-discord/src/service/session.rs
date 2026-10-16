//! The gateway session: connecting, then answering Yapfast's requests and
//! taking in Discord's events until the connection ends, and reconnecting.

use std::{
    pin::Pin,
    sync::{Arc, RwLock},
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::{
    sync::{broadcast, mpsc, watch},
    time::{Duration, Instant, Sleep},
};

use super::{
    Live, Request, Snapshot, Status, live::live_events, member_lists::MemberLists,
    member_search::MemberSearches, now_ms, publisher::Publisher, read_acks::ReadAcks, server,
    stream_openings::StreamOpenings, tracker::Tracker, voice_join::VoiceJoin,
};
use crate::{
    api::Api,
    cache,
    event::Event,
    gateway::{Closed, Dispatch, Gateway, Reconnects, TooManyIdentifies},
    message::{self, Names},
    ready,
    settings::Away,
    token,
};

/// What outlasts a session: the handle's channels and what Yapfast has on screen.
pub(super) struct Shared {
    pub(super) snapshots: watch::Sender<Arc<Snapshot>>,
    pub(super) live: broadcast::Sender<Live>,
    pub(super) names: Arc<RwLock<Names>>,
    pub(super) api: Arc<Api>,
    pub(super) lists: MemberLists,
    /// The presence this session last sent, which it identifies with again.
    pub(super) presence: Value,
    /// Nobody has used Yapfast since then, in Unix milliseconds, and whether
    /// it may show idle; see [`settings::Away`].
    pub(super) inactive: Option<(u64, bool)>,
    /// The server and channel or thread on screen, and a thread beside it,
    /// kept through reconnecting.
    pub(super) viewing: Option<(String, String)>,
    pub(super) beside: Option<(String, String)>,
}

pub(super) async fn run(
    mut token: String,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut shared: Shared,
) {
    let snapshots = shared.snapshots.clone();
    let mut reconnects = Reconnects::default();
    loop {
        snapshots.send_modify(|snapshot| Arc::make_mut(snapshot).status = Status::Connecting);
        let connect = async {
            shared.api.learn_build_number().await;
            Gateway::connect(&token, shared.presence.clone(), &mut reconnects).await
        };
        let connected = tokio::select! {
            connected = connect => connected,
            () = snapshots.closed() => return,
        };
        let (error, up) = match connected {
            Ok((gateway, ready)) => {
                if let Some(refreshed) = ready::auth_token(&ready) {
                    token = refreshed.to_owned();
                    relogin(&shared.api, &token).await;
                }
                let started = tokio::time::Instant::now();
                let ended = connected_session(gateway, ready, &mut requests, &mut shared).await;
                shared.lists.disconnected();
                match ended {
                    Ended::Dropped => return,
                    Ended::Failed(error) => (error, Some(started.elapsed())),
                }
            }
            Err(error) => (error, None),
        };
        tracing::warn!(%error, "Discord connection ended");
        let stopped = if error.downcast_ref::<Closed>().is_some_and(Closed::is_final) {
            Some("Discord refused the saved login. Link your Discord account again.")
        } else if error.downcast_ref::<TooManyIdentifies>().is_some() {
            Some(
                "Discord keeps ending the connection, so Yapfast stopped reconnecting. Restart Yapfast to try again.",
            )
        } else {
            None
        };
        if let Some(reason) = stopped {
            snapshots.send_modify(|snapshot| {
                let snapshot = Arc::make_mut(snapshot);
                snapshot.status = Status::Offline(reason.into());
                snapshot.voice.clear();
            });
            // Keep answering until Yapfast drops the handles.
            while let Some(request) = requests.recv().await {
                refuse(request, &mut shared);
            }
            return;
        }
        snapshots.send_modify(|snapshot| {
            let snapshot = Arc::make_mut(snapshot);
            snapshot.status = Status::Offline("Can't reach Discord. Reconnecting...".into());
            snapshot.voice.clear();
        });
        let wait = tokio::time::sleep(reconnects.wait(&error, up));
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                request = requests.recv() => match request {
                    Some(request) => refuse(request, &mut shared),
                    None => return,
                },
            }
        }
    }
}

/// Discord refreshed the login in READY, and the one Yapfast identified with
/// is to go: requests, identifying again, the credential store and the
/// files kept all take the new one. The gateway takes it from READY itself.
async fn relogin(api: &Api, token: &str) {
    tracing::info!("Discord refreshed the login");
    api.relogin(token);
    let token = token.to_owned();
    let stored = tokio::task::spawn_blocking(move || {
        cache::relogin(&token);
        token::store(&token)
    })
    .await;
    if let Ok(Err(error)) = stored {
        tracing::warn!(%error, "could not save Discord's refreshed login");
    }
}

/// Answers a request while disconnected. Viewing a channel, and being
/// idle, is remembered.
fn refuse(request: Request, shared: &mut Shared) {
    match request {
        Request::Join { reply, .. } => {
            let _ = reply.send(Err(anyhow!(
                "Discord isn't connected right now. Try again in a moment."
            )));
        }
        Request::View { guild, channel } => {
            shared.viewing = channel.clone().map(|channel| (guild.clone(), channel));
            shared.lists.view_offline(&shared.snapshots, guild, channel);
        }
        Request::Beside { guild, thread } => {
            shared.beside = thread.map(|thread| (guild, thread));
        }
        // The next session shows it once it connects.
        Request::Inactive(since, show_idle) => {
            shared.inactive = since.map(|since| (since, show_idle));
        }
        _ => {}
    }
}

enum Ended {
    /// Every handle was dropped.
    Dropped,
    Failed(anyhow::Error),
}

async fn connected_session(
    gateway: Gateway,
    ready: Value,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    shared: &mut Shared,
) -> Ended {
    let Some(me) = ready::user_id(&ready).map(str::to_owned) else {
        return Ended::Failed(anyhow!("Discord's READY had no user"));
    };
    let mut tracker = Tracker::from_ready(me, &ready, shared.names.clone());
    // READY can contain tens of megabytes of JSON. Only the parsed account
    // state is needed while connected; release the payload before waiting.
    drop(ready);
    trim_startup_heap();
    tracker.keep_unavailable(&shared.snapshots.borrow().servers);
    // Only what's kept of the earlier servers stays: held to the end of
    // this function, they would last as long as the session.
    let earlier = shared.snapshots.borrow().servers.clone();
    for (guild, channel) in shared.viewing.iter().chain(&shared.beside) {
        tracker.keep_thread(&earlier, guild, channel);
    }
    drop(earlier);
    if let Some((guild, channel)) = &shared.viewing {
        tracker.view(guild, Some(channel));
    }
    tracker.beside(shared.beside.as_ref().map(|(_, thread)| thread.as_str()));
    shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
    let quiet = tracker.next_quiet(now_ms());
    let mut session = Session {
        gateway,
        publisher: Publisher::new(shared.snapshots.clone()),
        acks: ReadAcks::new(shared.api.clone()),
        status_ends: Box::pin(tokio::time::sleep(status_left(&tracker))),
        mute_end: tracker.unreads.next_mute_end(),
        mutes_end: Box::pin(tokio::time::sleep(until(tracker.unreads.next_mute_end()))),
        quiet,
        quiets: Box::pin(tokio::time::sleep(until(quiet))),
        away_change: None,
        away_changes: Box::pin(tokio::time::sleep(until(None))),
        tracker,
        shared,
        join: VoiceJoin::new(),
        streams: StreamOpenings::new(),
        searches: MemberSearches::default(),
    };
    if let Err(error) = session.connect().await {
        return Ended::Failed(error);
    }
    session.run(requests).await
}

/// glibc otherwise holds freed READY allocations for the entire session.
/// Trim once after parsing, before the session starts handling requests.
fn trim_startup_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        // SAFETY: malloc_trim only returns unused allocator pages to the OS.
        unsafe { malloc_trim(0) };
    }
}

/// A connected session, and what it waits on.
struct Session<'a> {
    gateway: Gateway,
    tracker: Tracker,
    shared: &'a mut Shared,
    publisher: Publisher,
    join: VoiceJoin,
    streams: StreamOpenings,
    searches: MemberSearches,
    acks: ReadAcks,
    /// When the custom status ends by itself.
    status_ends: Pin<Box<Sleep>>,
    /// When the soonest mute in force ends, in Unix milliseconds, and the
    /// wait for it, after which what shows muted changes by itself.
    mute_end: Option<u64>,
    mutes_end: Pin<Box<Sleep>>,
    /// When the next thread this account is in goes quiet, in Unix
    /// milliseconds, and the wait for it, after which it stops listing.
    quiet: Option<u64>,
    quiets: Pin<Box<Sleep>>,
    /// When being away next changes the presence, in Unix milliseconds,
    /// and the wait for it.
    away_change: Option<u64>,
    away_changes: Pin<Box<Sleep>>,
}

impl Session<'_> {
    async fn connect(&mut self) -> Result<()> {
        // This session shows the status the settings hold, as Discord's own apps do.
        self.show_presence().await?;
        self.shared.lists.publish_group(&self.tracker);
        self.shared
            .lists
            .connect(&self.tracker, &mut self.gateway)
            .await
    }

    async fn run(mut self, requests: &mut mpsc::UnboundedReceiver<Request>) -> Ended {
        loop {
            let done = tokio::select! {
                () = self.publisher.due(), if self.publisher.stale() => {
                    self.publisher.publish(&mut self.tracker, &self.shared.lists);
                    self.quiets_changed();
                    Ok(())
                }
                request = requests.recv() => match request {
                    Some(request) => self.request(request).await,
                    None => {
                        self.gateway.close().await;
                        return Ended::Dropped;
                    }
                },
                () = self.status_ends.as_mut(), if self.tracker.custom_status.as_ref().is_some_and(|custom| custom.expires_at_ms.is_some()) => {
                    self.tracker.custom_status = None;
                    self.publisher.changed();
                    self.show_presence().await
                }
                () = self.away_changes.as_mut(), if self.away_change.is_some() => {
                    self.show_presence().await
                }
                () = self.quiets.as_mut(), if self.quiet.is_some() => {
                    // Published again, it waits for the next.
                    self.quiet = None;
                    self.publisher.changed();
                    Ok(())
                }
                () = self.mutes_end.as_mut(), if self.mute_end.is_some() => {
                    self.publisher.changed();
                    self.mutes_changed();
                    Ok(())
                }
                () = self.shared.lists.went_stale(), if self.shared.lists.unsent() => {
                    // The list on screen went stale without Discord sending it.
                    self.shared.lists.refresh(&mut self.gateway).await
                }
                () = self.acks.send_when_due(), if self.acks.waiting() => Ok(()),
                () = self.streams.timed_out(), if self.streams.waiting() => Ok(()),
                () = self.join.timed_out(), if self.join.waiting() => Ok(()),
                event = self.gateway.next() => match event {
                    Ok(event) => self.event(event).await,
                    Err(error) => {
                        self.join.fail("Discord disconnected while joining voice.");
                        self.streams.fail("Discord disconnected while opening the stream.");
                        Err(error)
                    }
                },
            };
            if let Err(error) = done {
                return Ended::Failed(error);
            }
        }
    }

    /// Acts on a request from the handle; an error ends the session.
    async fn request(&mut self, request: Request) -> Result<()> {
        let gateway = &mut self.gateway;
        match request {
            Request::Join {
                guild,
                channel,
                reply,
            } => {
                let sent = gateway
                    .update_voice_state(server(&guild), Some(&channel), true, false, false)
                    .await;
                self.join.start(guild, channel, reply);
                sent
            }
            Request::Leave { guild } => {
                // A call there on another client stays: no channel would
                // take it out of that one too.
                if !self.join.leave(&guild) && !self.tracker.in_voice_here(&guild) {
                    return Ok(());
                }
                gateway
                    .update_voice_state(server(&guild), None, false, false, false)
                    .await
            }
            Request::SelfState {
                guild,
                channel,
                mute,
                deaf,
                video,
            } => {
                gateway
                    .update_voice_state(server(&guild), Some(&channel), mute, deaf, video)
                    .await
            }
            // It opens once it lists, so it lists as on screen already.
            Request::OpenThread {
                guild,
                id,
                thread,
                beside,
            } => {
                if let Some(thread) = &thread {
                    self.tracker.put_thread(&guild, thread);
                }
                if beside {
                    self.tracker.beside(Some(&id));
                    self.shared.beside = Some((guild, id));
                } else {
                    self.tracker.view(&guild, Some(&id));
                    self.shared.viewing = Some((guild, id));
                }
                self.publisher.changed();
                Ok(())
            }
            Request::Beside { guild, thread } => {
                if self.tracker.beside(thread.as_deref()) {
                    self.publisher.changed();
                }
                self.shared.beside = thread.map(|thread| (guild, thread));
                Ok(())
            }
            Request::View { guild, channel } => {
                self.shared.viewing = channel.clone().map(|channel| (guild.clone(), channel));
                let threads = self.tracker.view(&guild, channel.as_deref());
                let channel = channel.map(|channel| self.tracker.listed_channel(&guild, channel));
                let listed = self
                    .shared
                    .lists
                    .view(&self.tracker, gateway, guild, channel)
                    .await?;
                if listed || threads {
                    self.publisher.changed();
                }
                Ok(())
            }
            Request::MarkRead {
                guild,
                channel,
                sync,
            } => {
                if let Some(message) = self.tracker.unreads.mark_read(&guild, &channel) {
                    self.publisher.changed();
                    if sync {
                        self.acks.read(channel, message);
                    }
                }
                Ok(())
            }
            Request::GoLive {
                guild,
                channel,
                reply,
            } => {
                self.streams
                    .go_live(gateway, &self.tracker, guild, channel, reply)
                    .await
            }
            Request::Watch { stream_key, reply } => {
                self.streams
                    .watch(gateway, &self.tracker, stream_key, reply)
                    .await
            }
            Request::EndStream { stream_key } => self.streams.end(gateway, &stream_key).await,
            Request::PauseStream { stream_key, paused } => {
                gateway.pause_stream(&stream_key, paused).await
            }
            Request::CustomStatus(custom) => {
                // The settings update that follows says the same; this
                // shows it without waiting.
                self.tracker.custom_status = custom;
                self.publisher.changed();
                self.custom_status_changed();
                self.show_presence().await
            }
            Request::Status(status) => {
                self.tracker.status = status;
                self.publisher.changed();
                self.show_presence().await
            }
            Request::Inactive(since, show_idle) => {
                self.shared.inactive = since.map(|since| (since, show_idle));
                self.show_presence().await
            }
            Request::Search {
                guild,
                query,
                reply,
            } => self.searches.start(gateway, &guild, &query, reply).await,
            Request::Members {
                guild,
                users,
                reply,
            } => {
                // Discord answers at most 100 users per request; a reply
                // waits for the first hundred.
                let mut chunks = users.chunks(100);
                if let (Some(reply), Some(users)) = (reply, chunks.next()) {
                    self.searches.members(gateway, &guild, users, reply).await?;
                }
                for users in chunks {
                    gateway.request_members(&guild, users, None).await?;
                }
                Ok(())
            }
        }
    }

    /// Takes in a dispatched event; an error ends the session.
    async fn event(&mut self, dispatch: Dispatch) -> Result<()> {
        let (event, data) = (&dispatch.event, &dispatch.data);
        // A server update that no join or stream waits for moves one
        // under way.
        let awaited = match event {
            Event::VoiceServerUpdate => self.join.awaits(data),
            Event::StreamServerUpdate => self.streams.awaits(data),
            _ => true,
        };
        self.join.apply(&self.tracker, &dispatch);
        // Whether the tracker already took this event in, and changed.
        let mut applied = None;
        match event {
            Event::PresenceUpdate => self.shared.lists.presence(&self.tracker, data),
            Event::GuildMemberListUpdate => {
                if self.shared.lists.update(&self.tracker, data) {
                    self.publisher.changed();
                }
            }
            Event::GuildMembersChunk | Event::RateLimited => {
                applied = self.searches.answer(&mut self.tracker, &dispatch);
            }
            _ => {}
        }
        // Parsed once, for the timeline and for a notification.
        let message = matches!(event, Event::MessageCreate | Event::MessageUpdate)
            .then(|| message::parse(data))
            .flatten();
        let (me, session) = (&self.tracker.me, self.tracker.session.as_deref());
        for live in live_events(
            me,
            session,
            &self.shared.names,
            event,
            data,
            message.as_ref(),
        ) {
            let _ = self.shared.live.send(live);
        }
        if *event == Event::MessageCreate
            && self.tracker.notifies(data)
            && let Some(message) = message
        {
            let guild = data["guild_id"].as_str().map(str::to_owned);
            let _ = self.shared.live.send(Live::Notify { guild, message });
        }
        if applied.unwrap_or_else(|| self.tracker.apply(event, data)) {
            self.publisher.changed();
        }
        // Names took the change in, so what shows the member follows.
        if matches!(event, Event::GuildMemberUpdate | Event::GuildMemberRemove) {
            self.shared.lists.member_changed(&self.tracker, data);
            let _ = self.shared.live.send(Live::Names);
        }
        if !awaited && let Some(moved) = self.tracker.moved(event, data) {
            let _ = self.shared.live.send(moved);
        }
        if *event == Event::UserGuildSettingsUpdate {
            self.mutes_changed();
        }
        if *event == Event::UserSettingsProtoUpdate {
            self.custom_status_changed();
            self.show_presence().await?;
        }
        self.shared.lists.group_event(&self.tracker, &dispatch);
        if self.streams.waiting() {
            if event.is_stream() {
                self.streams.apply(&self.tracker, &dispatch);
            } else if *event == Event::VoiceStateUpdate {
                // This account's voice session may be what a stream waited for.
                self.streams.voice_changed(&self.tracker);
            }
        }
        Ok(())
    }

    /// Sends the gateway this session's presence when it changed, and
    /// waits for the next time being away changes it.
    async fn show_presence(&mut self) -> Result<()> {
        let (away, next) = Away::after(
            self.shared.inactive.map(|(since, _)| since),
            self.tracker.afk_timeout,
            self.shared.inactive.is_some_and(|(_, show_idle)| show_idle),
            now_ms(),
        );
        self.away_change = next;
        self.away_changes
            .as_mut()
            .reset(Instant::now() + until(next));
        let presence = self.tracker.own_presence(away);
        if presence != self.shared.presence {
            self.gateway.update_presence(presence.clone()).await?;
            self.shared.presence = presence;
        }
        Ok(())
    }

    /// Waits for the custom status as it is now to end by itself.
    fn custom_status_changed(&mut self) {
        self.status_ends
            .as_mut()
            .reset(Instant::now() + status_left(&self.tracker));
    }
}

impl Session<'_> {
    /// Waits for the soonest mute in force now to end. The wait is set
    /// again even for the same end, since one that elapsed would otherwise
    /// end at once each time it's polled.
    fn mutes_changed(&mut self) {
        self.mute_end = self.tracker.unreads.next_mute_end();
        self.mutes_end
            .as_mut()
            .reset(Instant::now() + until(self.mute_end));
    }
}

impl Session<'_> {
    /// Waits for the next thread to go quiet, once a snapshot is out. The
    /// wait is set only when its end moves.
    fn quiets_changed(&mut self) {
        let quiet = self.tracker.next_quiet(now_ms());
        if quiet != self.quiet {
            self.quiet = quiet;
            self.quiets.as_mut().reset(Instant::now() + until(quiet));
        }
    }
}

/// How long until `at`, in Unix milliseconds; long when there's no `at`.
fn until(at: Option<u64>) -> Duration {
    at.map_or(Duration::from_secs(86_400 * 365), |at| {
        Duration::from_millis(at.saturating_sub(now_ms()))
    })
}

/// How long until the custom status ends by itself; long when it doesn't.
fn status_left(tracker: &Tracker) -> Duration {
    tracker
        .custom_status
        .as_ref()
        .and_then(|custom| custom.expires_at_ms)
        .map_or(Duration::from_secs(86_400 * 365), |at| {
            Duration::from_millis(at.saturating_sub(now_ms()))
        })
}
