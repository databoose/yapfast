//! The gateway session: connecting, then answering Talk's requests and
//! taking in Discord's events until the connection ends, and reconnecting.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
    time::Duration,
};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::{
    Live, Request, Snapshot, Status, VoiceServer, live::live_events, now_ms, server,
    tracker::Tracker,
};
use crate::{
    api::Api,
    cache,
    gateway::{Closed, Gateway, Reconnects, TooManyIdentifies},
    member_list::MemberList,
    message::{self, Names},
    ready::{self, PRIVATE},
    stream::{self, Opening, StreamServer},
};

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
/// Snapshots go out at most this often: the first change after a quiet
/// spell at once, and a burst of them together.
const PUBLISH_GAP: Duration = Duration::from_millis(100);
/// Reads within this long go to Discord as one acknowledgement per channel.
const ACK_DELAY: Duration = Duration::from_secs(1);

pub(super) struct Shared {
    pub(super) snapshots: watch::Sender<Arc<Snapshot>>,
    pub(super) live: broadcast::Sender<Live>,
    pub(super) names: Arc<RwLock<Names>>,
    pub(super) api: Arc<Api>,
    /// Servers with a channel on screen, subscribed again after reconnecting.
    pub(super) viewing: HashSet<String>,
    /// The server and channel whose member list is on screen.
    pub(super) listing: Option<(String, String)>,
    /// Member lists of channels shown lately, newest last, to show again at
    /// once while Discord sends them afresh.
    pub(super) lists: Vec<MemberList>,
    /// The group DM on screen, whose members Talk lists itself.
    pub(super) group: Option<String>,
    /// The presence this session last sent, which it identifies with again.
    pub(super) presence: Value,
}

/// Member lists kept for channels shown again.
const LISTS: usize = 40;
/// How long a member list Discord sent stands without asking again. A list
/// Discord still sends stays current however old.
const LIST_FRESH: Duration = Duration::from_secs(10 * 60);

impl Shared {
    /// Keeps a member list Discord sent, for when a channel with the same
    /// readers is shown again.
    fn keep_list(&mut self, list: MemberList) {
        if !list.synced() {
            return;
        }
        self.lists
            .retain(|kept| kept.guild() != list.guild() || kept.readers() != list.readers());
        self.lists.push(list);
        if self.lists.len() > LISTS {
            self.lists.remove(0);
        }
        cache::keep_lists(self.lists.clone());
    }

    /// The member list kept for channels with these readers, shown for
    /// `channel`, or an empty one.
    fn take_list(&mut self, guild: &str, channel: &str, readers: &str) -> MemberList {
        match self
            .lists
            .iter()
            .position(|list| list.guild() == guild && list.readers() == readers)
        {
            Some(index) => {
                let mut list = self.lists.remove(index);
                list.show_for(channel);
                list
            }
            None => MemberList::new(guild.to_owned(), channel.to_owned(), readers.to_owned()),
        }
    }
}

/// Asks Discord for the member list on screen, unless Discord already sends
/// it or sent it within [`LIST_FRESH`]; then `stale` fires when it's due.
/// `subscribed` holds the readers each server's subscription lists.
async fn refresh_list(
    gateway: &mut Gateway,
    tracker: &mut Tracker,
    subscribed: &mut HashMap<String, String>,
    stale: std::pin::Pin<&mut tokio::time::Sleep>,
) -> Result<()> {
    let Some(list) = tracker.list.as_mut() else {
        return Ok(());
    };
    if subscribed
        .get(list.guild())
        .is_some_and(|readers| readers == list.readers())
    {
        return Ok(());
    }
    if list.heard_within(LIST_FRESH)
        && let Some(at) = list.stale_at(LIST_FRESH)
    {
        stale.reset(at.into());
        return Ok(());
    }
    // Discord's sync replaces what was kept, even under another id, as when
    // the readers changed meanwhile.
    list.revisit();
    subscribed.insert(list.guild().to_owned(), list.readers().to_owned());
    gateway.subscribe(list.guild(), Some(list.channel())).await
}

pub(super) async fn run(
    token: String,
    mut requests: mpsc::UnboundedReceiver<Request>,
    mut shared: Shared,
) {
    let snapshots = shared.snapshots.clone();
    let mut reconnects = Reconnects::default();
    loop {
        snapshots.send_modify(|snapshot| Arc::make_mut(snapshot).status = Status::Connecting);
        let connected = tokio::select! {
            connected = Gateway::connect(&token, shared.presence.clone(), &mut reconnects) => connected,
            () = snapshots.closed() => return,
        };
        let (error, up) = match connected {
            Ok((gateway, ready)) => {
                let started = tokio::time::Instant::now();
                match connected_session(gateway, &ready, &mut requests, &mut shared).await {
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
                "Discord keeps ending the connection, so Talk stopped reconnecting. Restart Talk to try again.",
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
            // Keep answering until Talk drops the handles.
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

/// Answers a request while disconnected. Viewing a channel is remembered.
fn refuse(request: Request, shared: &mut Shared) {
    match request {
        Request::Join { reply, .. } => {
            let _ = reply.send(Err(anyhow!(
                "Discord isn't connected right now. Try again in a moment."
            )));
        }
        Request::View { guild, channel } if guild == PRIVATE => shared.group = channel,
        Request::View { guild, channel } => {
            if let Some(channel) = channel {
                show_kept_members(shared, &guild, &channel);
                shared.listing = Some((guild.clone(), channel));
            }
            shared.group = None;
            shared.viewing.insert(guild);
        }
        _ => {}
    }
}

/// Shows the member list kept for a channel while Discord can't send it,
/// as when starting from the saved account.
fn show_kept_members(shared: &Shared, guild: &str, channel: &str) {
    let snapshot = shared.snapshots.borrow().clone();
    let Some(server) = snapshot.servers.iter().find(|server| server.id == guild) else {
        return;
    };
    let readers = server.readers(channel);
    let Some(list) = shared
        .lists
        .iter()
        .find(|list| list.guild() == guild && list.readers() == readers && list.synced())
    else {
        return;
    };
    let members = {
        let names = shared
            .names
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        list.members(Some(server), &names)
    };
    let _ = shared.live.send(Live::Members {
        channel: channel.to_owned(),
        members,
    });
    if let Some(count) = list.count() {
        shared.snapshots.send_modify(|snapshot| {
            Arc::make_mut(snapshot).listed = Some((channel.to_owned(), count))
        });
    }
}

/// Passes a stream dispatch to the streams being opened, and answers those
/// that are now ready or that Discord refused.
fn open_streams(
    openings: &mut HashMap<String, PendingStream>,
    tracker: &Tracker,
    name: &str,
    data: &Value,
) {
    let Some(key) = data["stream_key"].as_str() else {
        return;
    };
    let Some(mut waiting) = openings.remove(key) else {
        return;
    };
    if name == "STREAM_DELETE" {
        let reason = match data["reason"].as_str() {
            Some("stream_full") => "The stream is full.",
            Some("unauthorized") => "You aren't allowed to stream or watch here.",
            _ => "The stream ended before it started.",
        };
        let _ = waiting.reply.send(Err(anyhow!(reason)));
        return;
    }
    waiting.opening.apply(key, name, data);
    let session = tracker.voice_session(&waiting.guild);
    match session.and_then(|session| waiting.opening.server(key, &tracker.me, session)) {
        Some(server) => {
            let _ = waiting.reply.send(Ok(server));
        }
        None => {
            openings.insert(key.to_owned(), waiting);
        }
    }
}

enum Ended {
    /// Every handle was dropped.
    Dropped,
    Failed(anyhow::Error),
}

struct Pending {
    guild: String,
    channel: String,
    reply: oneshot::Sender<Result<VoiceServer>>,
    session_id: Option<String>,
    server: Option<(String, String)>,
}

impl Pending {
    /// Whether a voice state or voice server event is about this join: in
    /// its server, or for a DM call, its channel, as those carry no server.
    fn answers(&self, data: &Value) -> bool {
        match server(&self.guild) {
            Some(guild) => data["guild_id"] == guild,
            None => data["guild_id"].is_null() && data["channel_id"] == self.channel.as_str(),
        }
    }
}

/// A stream started or watched, waiting for its server.
struct PendingStream {
    guild: String,
    reply: oneshot::Sender<Result<StreamServer>>,
    opening: Opening,
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

/// Sends the gateway this session's presence when it changed.
async fn show_presence(
    gateway: &mut Gateway,
    tracker: &Tracker,
    shared: &mut Shared,
) -> Result<()> {
    let presence = tracker.own_presence();
    if presence != shared.presence {
        gateway.update_presence(presence.clone()).await?;
        shared.presence = presence;
    }
    Ok(())
}

fn fail_streams(openings: &mut HashMap<String, PendingStream>, message: &str) {
    for (_, waiting) in openings.drain() {
        let _ = waiting.reply.send(Err(anyhow!("{message}")));
    }
}

async fn connected_session(
    mut gateway: Gateway,
    ready: &Value,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    shared: &mut Shared,
) -> Ended {
    let Some(me) = ready::user_id(ready).map(str::to_owned) else {
        return Ended::Failed(anyhow!("Discord's READY had no user"));
    };
    let mut tracker = Tracker::from_ready(me.clone(), ready, shared.names.clone());
    tracker.keep_unavailable(&shared.snapshots.borrow().servers);
    shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
    // This session shows the status the settings hold, as Discord's own apps do.
    if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
        return Ended::Failed(error);
    }
    publish_group(&tracker, shared);
    tracker.list = shared.listing.clone().map(|(guild, channel)| {
        let readers = tracker.readers(&guild, &channel);
        shared.take_list(&guild, &channel, &readers)
    });
    // A new session has no subscriptions: those of servers on screen go again.
    let mut subscribed: HashMap<String, String> = HashMap::new();
    for guild in &shared.viewing {
        let mut channel = None;
        if let Some(list) = tracker.list.as_mut().filter(|list| list.guild() == guild) {
            list.revisit();
            subscribed.insert(guild.clone(), list.readers().to_owned());
            channel = Some(list.channel().to_owned());
        }
        if let Err(error) = gateway.subscribe(guild, channel.as_deref()).await {
            return Ended::Failed(error);
        }
    }
    let list_stale = tokio::time::sleep(LIST_FRESH);
    tokio::pin!(list_stale);
    let mut pending: Option<Pending> = None;
    let deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(deadline);
    let mut openings: HashMap<String, PendingStream> = HashMap::new();
    let stream_deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(stream_deadline);
    // The newest read message per channel, waiting to be acknowledged.
    let mut acks: HashMap<String, String> = HashMap::new();
    // Member searches waiting for their answer, by nonce.
    let mut searches: HashMap<String, oneshot::Sender<Vec<String>>> = HashMap::new();
    let mut search_nonce = 0u64;
    let ack_at = tokio::time::sleep(ACK_DELAY);
    tokio::pin!(ack_at);
    // When the custom status ends by itself.
    let status_ends = tokio::time::sleep(status_left(&tracker));
    tokio::pin!(status_ends);
    // The account changed since the last snapshot; one goes out when
    // `publish_at` comes, PUBLISH_GAP after the last.
    let mut stale = false;
    let mut scheduled = false;
    let mut published = tokio::time::Instant::now();
    let publish_at = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(publish_at);
    loop {
        if stale && !scheduled {
            publish_at
                .as_mut()
                .reset((published + PUBLISH_GAP).max(tokio::time::Instant::now()));
            scheduled = true;
        }
        tokio::select! {
            () = &mut publish_at, if scheduled => {
                shared.snapshots.send_replace(Arc::new(tracker.snapshot()));
                published = tokio::time::Instant::now();
                (stale, scheduled) = (false, false);
            }
            request = requests.recv() => {
                let Some(request) = request else {
                    gateway.close().await;
                    return Ended::Dropped;
                };
                let sent = match request {
                    Request::Join { guild, channel, reply } => {
                        let sent = gateway.update_voice_state(server(&guild), Some(&channel), true, false).await;
                        deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                        if let Some(previous) = pending.replace(Pending { guild, channel, reply, session_id: None, server: None }) {
                            let _ = previous.reply.send(Err(anyhow!("Another voice join replaced this one.")));
                        }
                        sent
                    }
                    Request::Leave { guild } => {
                        if pending.as_ref().is_some_and(|pending| pending.guild == guild) {
                            pending = None;
                        }
                        gateway.update_voice_state(server(&guild), None, false, false).await
                    }
                    Request::SelfState { guild, channel, mute, deaf } => {
                        gateway.update_voice_state(server(&guild), Some(&channel), mute, deaf).await
                    }
                    Request::View { guild, channel } if guild == PRIVATE => {
                        shared.group = channel;
                        publish_group(&tracker, shared);
                        Ok(())
                    }
                    Request::View { guild, channel } => {
                        shared.group = None;
                        let new = shared.viewing.insert(guild.clone());
                        match channel {
                            Some(channel) => {
                                shared.listing = Some((guild.clone(), channel.clone()));
                                let readers = tracker.readers(&guild, &channel);
                                match tracker.list.as_mut() {
                                    // Channels with the same readers share a list.
                                    Some(list) if list.guild() == guild && list.readers() == readers => {
                                        list.show_for(&channel);
                                    }
                                    _ => {
                                        let list = shared.take_list(&guild, &channel, &readers);
                                        if let Some(previous) = tracker.list.replace(list) {
                                            shared.keep_list(previous);
                                        }
                                    }
                                }
                                // Shown at once from the cache, until Discord's sync.
                                if tracker.list.as_ref().is_some_and(MemberList::synced) {
                                    publish_members(&tracker, shared);
                                }
                                stale = true;
                                let sent = refresh_list(&mut gateway, &mut tracker, &mut subscribed, list_stale.as_mut()).await;
                                if new && !subscribed.contains_key(&guild) {
                                    gateway.subscribe(&guild, None).await.and(sent)
                                } else {
                                    sent
                                }
                            }
                            None if new => gateway.subscribe(&guild, None).await,
                            None => Ok(()),
                        }
                    }
                    Request::MarkRead { guild, channel, sync } => {
                        if let Some(message) = tracker.unreads.mark_read(&guild, &channel) {
                            stale = true;
                            if sync {
                                if acks.is_empty() {
                                    ack_at.as_mut().reset(tokio::time::Instant::now() + ACK_DELAY);
                                }
                                acks.insert(channel, message);
                            }
                        }
                        Ok(())
                    }
                    Request::GoLive { guild, channel, reply } => {
                        if tracker.voice_channel(&guild).is_none_or(|current| current != channel) {
                            let _ = reply.send(Err(anyhow!("Join the call before going live.")));
                            Ok(())
                        } else {
                            let key = stream::key(&guild, &channel, &me);
                            stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                            let waiting = PendingStream { guild: guild.clone(), reply, opening: Opening::default() };
                            if let Some(previous) = openings.insert(key.clone(), waiting) {
                                let _ = previous.reply.send(Err(anyhow!("Going live again replaced this attempt.")));
                            }
                            // Unpaused straight away, as Discord-video-stream
                            // does: viewers of a paused stream get no video.
                            match gateway.create_stream(server(&guild), &channel).await {
                                Ok(()) => gateway.pause_stream(&key, false).await,
                                Err(error) => Err(error),
                            }
                        }
                    }
                    Request::Watch { stream_key, reply } => {
                        match stream::parse(&stream_key) {
                            Some((guild, channel, _)) if tracker.voice_channel(guild) == Some(channel) => {
                                let guild = guild.to_owned();
                                stream_deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                                let waiting = PendingStream { guild, reply, opening: Opening::default() };
                                if let Some(previous) = openings.insert(stream_key.clone(), waiting) {
                                    let _ = previous.reply.send(Err(anyhow!("Watching again replaced this attempt.")));
                                }
                                gateway.watch_stream(&stream_key).await
                            }
                            Some(_) => {
                                let _ = reply.send(Err(anyhow!("Join the call to watch its streams.")));
                                Ok(())
                            }
                            None => {
                                let _ = reply.send(Err(anyhow!("That isn't a stream Talk knows how to watch.")));
                                Ok(())
                            }
                        }
                    }
                    Request::EndStream { stream_key } => {
                        if let Some(waiting) = openings.remove(&stream_key) {
                            let _ = waiting.reply.send(Err(anyhow!("The stream was stopped before it started.")));
                        }
                        gateway.delete_stream(&stream_key).await
                    }
                    Request::PauseStream { stream_key, paused } => {
                        gateway.pause_stream(&stream_key, paused).await
                    }
                    Request::CustomStatus(custom) => {
                        // The settings update that follows says the same; this
                        // shows it without waiting.
                        tracker.custom_status = custom;
                        stale = true;
                        status_ends.as_mut().reset(tokio::time::Instant::now() + status_left(&tracker));
                        show_presence(&mut gateway, &tracker, shared).await
                    }
                    Request::Status(status) => {
                        tracker.status = status;
                        stale = true;
                        show_presence(&mut gateway, &tracker, shared).await
                    }
                    Request::Search { guild, query, reply } => {
                        search_nonce += 1;
                        let nonce = format!("talk-search-{search_nonce}");
                        // Unanswered searches are dropped, so their callers stop waiting.
                        searches.retain(|_, reply| !reply.is_closed());
                        searches.insert(nonce.clone(), reply);
                        gateway.search_members(&guild, &query, 10, &nonce).await
                    }
                    Request::Members { guild, users } => {
                        // Discord answers at most 100 users per request.
                        let mut sent = Ok(());
                        for users in users.chunks(100) {
                            sent = gateway.request_members(&guild, users).await;
                            if sent.is_err() { break; }
                        }
                        sent
                    }
                };
                if let Err(error) = sent {
                    return Ended::Failed(error);
                }
            }
            () = &mut status_ends, if tracker.custom_status.as_ref().is_some_and(|custom| custom.expires_at_ms.is_some()) => {
                tracker.custom_status = None;
                stale = true;
                if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
                    return Ended::Failed(error);
                }
            }
            () = &mut list_stale, if tracker.list.as_ref().is_some_and(|list| subscribed.get(list.guild()).is_none_or(|readers| readers != list.readers())) => {
                // The list on screen went stale without Discord sending it.
                if let Err(error) = refresh_list(&mut gateway, &mut tracker, &mut subscribed, list_stale.as_mut()).await {
                    return Ended::Failed(error);
                }
            }
            () = &mut ack_at, if !acks.is_empty() => {
                for (channel, message) in acks.drain() {
                    let api = shared.api.clone();
                    tokio::spawn(async move {
                        if let Err(error) = api.ack(&channel, &message).await {
                            tracing::warn!(%error, "could not mark a Discord channel read");
                        }
                    });
                }
            }
            () = &mut stream_deadline, if !openings.is_empty() => {
                fail_streams(&mut openings, "Discord didn't assign a stream server. Try again.");
            }
            () = &mut deadline, if pending.is_some() => {
                if let Some(pending) = pending.take() {
                    let _ = pending.reply.send(Err(anyhow!("Discord didn't assign a voice server. Try again.")));
                }
            }
            event = gateway.next() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if let Some(pending) = pending.take() {
                            let _ = pending.reply.send(Err(anyhow!("Discord disconnected while joining voice.")));
                        }
                        fail_streams(&mut openings, "Discord disconnected while opening the stream.");
                        return Ended::Failed(error);
                    }
                };
                // A server update that no join or stream waits for moves one
                // under way.
                let awaited = match event.name.as_str() {
                    "VOICE_SERVER_UPDATE" => pending.as_ref().is_some_and(|waiting| waiting.answers(&event.data)),
                    "STREAM_SERVER_UPDATE" => event.data["stream_key"].as_str().is_some_and(|key| openings.contains_key(key)),
                    _ => true,
                };
                if let Some(waiting) = pending.as_mut() {
                    match event.name.as_str() {
                        "VOICE_STATE_UPDATE" if event.data["user_id"] == me.as_str() && waiting.answers(&event.data) => {
                            waiting.session_id = event.data["session_id"].as_str().map(str::to_owned);
                        }
                        // The endpoint is null while Discord is still allocating a server.
                        "VOICE_SERVER_UPDATE" if waiting.answers(&event.data) => {
                            if let (Some(token), Some(endpoint)) = (event.data["token"].as_str(), event.data["endpoint"].as_str()) {
                                waiting.server = Some((token.into(), endpoint.into()));
                            }
                        }
                        _ => {}
                    }
                    if let (Some(session_id), Some((token, endpoint))) = (&waiting.session_id, &waiting.server) {
                        let server = VoiceServer { user_id: me.clone(), session_id: session_id.clone(), token: token.clone(), endpoint: endpoint.clone() };
                        if let Some(done) = pending.take() {
                            let _ = done.reply.send(Ok(server));
                        }
                    }
                }
                if event.name == "PRESENCE_UPDATE"
                    && tracker.list.as_mut().is_some_and(|list| list.presence(&event.data))
                {
                    publish_members(&tracker, shared);
                }
                if event.name == "GUILD_MEMBER_LIST_UPDATE" {
                    let mut names = shared.names.write().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let (mut changed, mut counted, mut synced) = (false, false, false);
                    if let Some(list) = tracker.list.as_mut() {
                        let (count, settled) = (list.count(), list.settled());
                        changed = list.apply(&event.data, &mut names);
                        counted = list.count() != count;
                        synced = !settled && list.settled();
                    }
                    // Or a list kept from before, which Discord still sends.
                    if !changed {
                        for kept in shared.lists.iter_mut().filter(|kept| kept.settled()) {
                            if kept.apply(&event.data, &mut names) {
                                break;
                            }
                        }
                    }
                    drop(names);
                    // A list Discord just sent is saved for the next start.
                    if synced {
                        cache::keep_lists(
                            shared.lists.iter().chain(tracker.list.as_ref()).cloned().collect(),
                        );
                    }
                    if changed {
                        publish_members(&tracker, shared);
                    }
                    if counted {
                        stale = true;
                    }
                }
                if event.name == "GUILD_MEMBERS_CHUNK"
                    && let Some(reply) = event.data["nonce"].as_str().and_then(|nonce| searches.remove(nonce))
                {
                    // Learn the members before answering, so their names are known.
                    tracker.apply(&event.name, &event.data);
                    let found = event.data["members"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|member| member["user"]["id"].as_str())
                        .map(str::to_owned)
                        .collect();
                    let _ = reply.send(found);
                }
                for live in live_events(&me, &shared.names, &event.name, &event.data) {
                    let _ = shared.live.send(live);
                }
                if event.name == "MESSAGE_CREATE"
                    && tracker.notifies(&event.data)
                    && let Some(message) = message::parse(&event.data)
                {
                    let guild = event.data["guild_id"].as_str().map(str::to_owned);
                    let _ = shared.live.send(Live::Notify { guild, message });
                }
                if tracker.apply(&event.name, &event.data) {
                    stale = true;
                }
                if !awaited && let Some(moved) = tracker.moved(&event.name, &event.data) {
                    let _ = shared.live.send(moved);
                }
                if event.name == "USER_SETTINGS_PROTO_UPDATE" {
                    status_ends.as_mut().reset(tokio::time::Instant::now() + status_left(&tracker));
                    if let Err(error) = show_presence(&mut gateway, &tracker, shared).await {
                        return Ended::Failed(error);
                    }
                }
                // Server events, such as presences in a server, don't change a group.
                if shared.group.is_some()
                    && event.data["guild_id"].is_null()
                    && matches!(
                        event.name.as_str(),
                        "READY_SUPPLEMENTAL" | "PRESENCE_UPDATE" | "CHANNEL_UPDATE"
                            | "CHANNEL_RECIPIENT_ADD" | "CHANNEL_RECIPIENT_REMOVE"
                    )
                {
                    publish_group(&tracker, shared);
                }
                if !openings.is_empty() && event.name.starts_with("STREAM_") {
                    open_streams(&mut openings, &tracker, &event.name, &event.data);
                }
            }
        }
    }
}

/// The members of the group DM on screen.
fn publish_group(tracker: &Tracker, shared: &Shared) {
    let Some(channel) = &shared.group else {
        return;
    };
    if let Some(members) = tracker.group_members(channel) {
        let _ = shared.live.send(Live::Members {
            channel: channel.clone(),
            members,
        });
    }
}

fn publish_members(tracker: &Tracker, shared: &Shared) {
    let Some(list) = &tracker.list else {
        return;
    };
    let server = tracker
        .servers
        .iter()
        .find(|server| server.id == list.guild())
        .map(Arc::as_ref);
    let names = shared
        .names
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _ = shared.live.send(Live::Members {
        channel: list.channel().to_owned(),
        members: list.members(server, &names),
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn answers_a_dm_join_by_its_channel() {
        let (reply, _) = oneshot::channel();
        let waiting = Pending {
            guild: PRIVATE.into(),
            channel: "20".into(),
            reply,
            session_id: None,
            server: None,
        };
        assert!(waiting.answers(&json!({"guild_id": null, "channel_id": "20"})));
        assert!(!waiting.answers(&json!({"guild_id": null, "channel_id": "21"})));
        assert!(!waiting.answers(&json!({"guild_id": "10", "channel_id": "20"})));
    }
}
