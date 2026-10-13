//! Discord voice: a call through songbird with Talk's own audio, from asking
//! Discord to join until leaving.
use std::{future::Future, path::Path, pin::Pin, sync::Arc, time::Duration};

use anyhow::{Result, bail, ensure};
use talk_discord::{
    ready::PRIVATE,
    service::{Discord, Live, Snapshot, Status, VoiceServer},
    stream::{self, StreamServer},
    voice::{Call as DiscordCall, live_microphone},
};
use tokio::sync::{mpsc, watch};

use crate::{
    discord_audio::{self, Capture, Hearing, Speakers},
    ring::{Ringer, Tone},
    stream_worker::{self, Report, Worker},
    voice::{
        Chosen, Command, Participant, Phase, SharePhase, Sharing, State, StreamQuality, Watching,
        devices,
    },
};

/// Going live: the worker capturing the screen, then the stream Discord
/// started for it.
struct Share {
    worker: Worker,
    reports: mpsc::UnboundedReceiver<Report>,
    /// The stream's key once Discord was asked to start it, so stopping ends
    /// it even before Discord answers.
    key: Option<String>,
    /// Uploading the stream's latest preview.
    preview: Option<tokio::task::JoinHandle<()>>,
}

impl Share {
    /// Starts the worker; Discord hears of the stream once there's something
    /// to show, so a cancelled dialog shares nothing.
    fn start(state: &mut State, quality: StreamQuality) -> Result<Self> {
        // Tests and screenshots share colour bars instead of a screen.
        let source = if std::env::var_os("TALK_STREAM_TEST_PATTERN").is_some() {
            stream_worker::Source::TestPattern
        } else {
            stream_worker::Source::Screen
        };
        let (worker, frames, reports) = Worker::share(source, quality)?;
        state.sharing = Some(Sharing {
            phase: SharePhase::Choosing,
            frames: Arc::new(frames),
        });
        Ok(Self {
            worker,
            reports,
            key: None,
            preview: None,
        })
    }

    /// Uploads a preview of the stream, in place of one still uploading.
    /// A failed upload is tried again a minute later, as Discord's client
    /// does, up to three times.
    fn show_preview(&mut self, discord: &Discord, thumbnail: String) {
        let Some(key) = self.key.clone() else { return };
        if let Some(older) = self.preview.take() {
            older.abort();
        }
        let discord = discord.clone();
        self.preview = Some(tokio::spawn(async move {
            for attempt in 1..=3 {
                if attempt > 1 {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                }
                match discord.stream_preview(&key, &thumbnail).await {
                    Ok(()) => {
                        tracing::debug!("uploaded the stream's preview");
                        return;
                    }
                    Err(error) => {
                        tracing::warn!(
                            error = format!("{error:#}"),
                            attempt,
                            "could not upload the stream's preview"
                        )
                    }
                }
            }
        }));
    }

    /// Ends the stream on Discord, and the worker with it.
    fn stop(self, discord: &Discord, state: &mut State) {
        if let Some(preview) = &self.preview {
            preview.abort();
        }
        if let Some(key) = &self.key {
            discord.end_stream(key);
        }
        drop(self.worker);
        state.sharing = None;
    }
}

/// The next report from the share's worker, or never without a share.
async fn next_report(share: &mut Option<Share>) -> Option<Report> {
    match share {
        Some(share) => share.reports.recv().await,
        None => std::future::pending().await,
    }
}

/// Asking Discord to start the stream, once frames flow.
type Starting = Option<Pin<Box<dyn Future<Output = Result<StreamServer>>>>>;

/// Discord's answer to going live, or never while it isn't asked.
async fn started(starting: &mut Starting) -> Result<StreamServer> {
    match starting {
        Some(starting) => starting.await,
        None => std::future::pending().await,
    }
}

/// Joining the call: Discord's voice server, then connecting to it.
type Joining<'a> = Pin<Box<dyn Future<Output = Result<(VoiceServer, DiscordCall)>> + 'a>>;

/// Connects to the voice server Discord assigned for `channel`.
fn connect<'a>(
    server: VoiceServer,
    guild: &'a str,
    channel: String,
    hearing: Arc<Hearing>,
) -> Joining<'a> {
    Box::pin(async move {
        let call = DiscordCall::connect(&server, guild, &channel, hearing).await?;
        Ok((server, call))
    })
}

/// Ringing the others for a DM call this account started: whether Discord
/// let it ring them.
type Ringing = Option<Pin<Box<dyn Future<Output = Result<bool>> + Send>>>;

/// Whether ringing worked, or never while nothing is being rung.
async fn rung(ringing: &mut Ringing) -> Result<bool> {
    match ringing {
        Some(ringing) => ringing.await,
        None => std::future::pending().await,
    }
}

/// Watching someone's stream: Discord's answer, then a worker showing it.
struct Watch {
    key: String,
    /// Whose stream it is, by Discord user id.
    user: String,
    opening: Starting,
    worker: Option<(Worker, mpsc::UnboundedReceiver<Report>)>,
}

impl Watch {
    /// Plays the stream's sound, or not, as being deafened changes.
    fn hear(watch: &mut Option<Self>, hearing: bool) {
        if let Some((worker, _)) = watch.as_mut().and_then(|watch| watch.worker.as_mut())
            && let Err(error) = worker.hear(hearing)
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "could not tell the stream worker"
            );
        }
    }

    /// Plays the stream's sound at the volume chosen for it.
    fn volume(watch: &mut Option<Self>) {
        if let Some(watching) = watch.as_mut()
            && let Some((worker, _)) = watching.worker.as_mut()
            && let Err(error) = worker.volume(crate::volume::get(
                crate::volume::Kind::Stream,
                &watching.user,
            ))
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "could not tell the stream worker"
            );
        }
    }

    fn start(discord: &Discord, guild: &str, channel: &str, user: &str, state: &mut State) -> Self {
        let key = stream::key(guild, channel, user);
        let name = state
            .participants
            .iter()
            .find(|person| person.id == user)
            .map_or_else(|| "Someone".into(), |person| person.user.clone());
        state.watching = Some(Box::new(Watching {
            user: user.to_owned(),
            name,
            opening: true,
            frames: None,
        }));
        let (discord, asked) = (discord.clone(), key.clone());
        Self {
            key,
            user: user.to_owned(),
            opening: Some(Box::pin(async move { discord.watch(&asked).await })),
            worker: None,
        }
    }

    /// Shows the stream from `server`, in place of a worker showing it from
    /// the server it was on before.
    fn open(&mut self, server: &StreamServer, state: &mut State) -> Result<()> {
        self.opening = None;
        let volume = crate::volume::get(crate::volume::Kind::Stream, &self.user);
        let (worker, frames, reports) =
            Worker::watch(server, state.speaker.clone(), state.deafened, volume)?;
        self.worker = Some((worker, reports));
        if let Some(shown) = state.watching.as_mut() {
            shown.frames = Some(Arc::new(frames));
        }
        Ok(())
    }

    /// Stops watching on Discord, and the worker with it.
    fn stop(self, discord: &Discord, state: &mut State) {
        discord.end_stream(&self.key);
        drop(self.worker);
        state.watching = None;
    }
}

enum WatchEvent {
    Opened(Result<StreamServer>),
    Report(Option<Report>),
}

/// Discord's answer, then the worker's reports; never without a watch.
async fn watch_event(watch: &mut Option<Watch>) -> WatchEvent {
    let Some(watch) = watch else {
        return std::future::pending().await;
    };
    if let Some(opening) = watch.opening.as_mut() {
        return WatchEvent::Opened(opening.await);
    }
    match watch.worker.as_mut() {
        Some((_, reports)) => WatchEvent::Report(reports.recv().await),
        None => std::future::pending().await,
    }
}

/// A Discord voice call, from asking Discord to join until leaving. Commands
/// stay responsive throughout, as in a Matrix call.
pub(crate) async fn call(
    discord: &Discord,
    guild: &str,
    channel: &str,
    state: &mut State,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    snapshots: &watch::Sender<State>,
    remembered: Option<&Path>,
) -> Result<()> {
    ensure!(
        discord_audio::AVAILABLE,
        "Discord voice has no sound on this system yet."
    );
    let mut discord_states = discord.snapshots();
    let me = discord_states.borrow().user_id.clone().unwrap_or_default();
    state.name = call_name(&discord_states.borrow(), guild, channel)
        .unwrap_or(if guild == PRIVATE {
            "Discord call"
        } else {
            "Discord voice"
        })
        .to_owned();
    snapshots.send_replace(state.clone());

    let (ended, mut endings) = mpsc::unbounded_channel();
    let hearing = Arc::new(Hearing::new(ended));
    // From before joining, so that a move while connecting is heard.
    let mut live = discord.live();
    let mut joining: Joining = Box::pin(async {
        let server = discord.join(guild, channel).await?;
        connect(server, guild, channel.to_owned(), hearing.clone()).await
    });
    // Where this account is, which someone such as a bot may move it from.
    let mut channel = channel.to_owned();
    let deadline = tokio::time::sleep(Duration::from_secs(35));
    tokio::pin!(deadline);
    let joined = loop {
        tokio::select! {
            biased;
            command = commands.recv() => match command {
                None | Some(Command::Leave) => {
                    discord.leave(guild);
                    return Ok(());
                }
                // Used once the call connects.
                Some(Command::Microphone(id)) => state.microphone = Some(id),
                Some(Command::Speaker(id)) => state.speaker = Some(id),
                Some(Command::Camera(id)) => state.camera = Some(id),
                Some(Command::Devices) => { let _ = devices(state); }
                _ => {}
            },
            () = &mut deadline => {
                discord.leave(guild);
                bail!("Discord voice took too long to connect. Try again.");
            }
            result = &mut joining => break result,
            Ok(event) = live.recv() => match event {
                Live::VoiceChannel { guild: moved, channel: to } if moved == guild && to != channel => {
                    channel = to;
                    moved_to(state, &discord_states.borrow(), guild, &channel);
                    snapshots.send_replace(state.clone());
                }
                // Someone moved this account while it connected, as a bot
                // does from a channel that makes rooms. The connection under
                // way would end in Discord's "session is no longer valid".
                Live::VoiceMoved { guild: moved, server } if moved == guild => {
                    tracing::info!("Discord moved the call while it connected");
                    joining = connect(server, guild, channel.clone(), hearing.clone());
                }
                _ => {}
            },
        }
    };
    let (mut server, mut call) = match joined {
        Ok(joined) => joined,
        Err(error) => {
            discord.leave(guild);
            return Err(error);
        }
    };
    let (played, reverse) = std::sync::mpsc::sync_channel(8);
    let reverse = std::sync::Arc::new(std::sync::Mutex::new(reverse));
    let (microphone, input) = live_microphone();
    let track = call.play(input);
    let _ = track.pause();
    let mut capture: Option<Capture> = None;
    let mut share: Option<Share> = None;
    let mut starting: Starting = None;
    let mut watch: Option<Watch> = None;
    // Ringing the others while they haven't come, with the ringback heard meanwhile.
    let mut ringing: Ringing = None;
    let mut ringback: Option<Ringer> = None;
    // Discord wouldn't ring them, which the call says until someone comes.
    let mut unrung = false;

    let result = async {
        let mut speakers = Some(open_discord_speakers(state, &played, &hearing)?);
        Chosen::write(remembered, state);
        state.phase = Phase::Connected;
        if guild == PRIVATE && alone(&discord_states.borrow(), &channel, &me) {
            // Starting a DM call, rather than joining one, rings the others,
            // once Discord has the call: it rings only in one.
            let (discord, channel) = (discord.clone(), channel.to_owned());
            ringing = Some(Box::pin(async move {
                let mut states = discord.snapshots();
                let started = states.wait_for(|snapshot| {
                    snapshot.conversations.iter().any(|found| found.id == channel && found.call)
                });
                if tokio::time::timeout(Duration::from_secs(5), started).await.is_err() {
                    tracing::warn!("Discord hasn't reported the DM call; ringing anyway");
                }
                discord.ring(&channel).await
            }));
        }
        let mut ticks = tokio::time::interval(Duration::from_millis(150));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut volumes = crate::volume::changes();
        // The voice connection dropped, and the call ends when `lost` comes
        // unless Discord says it moved the call meanwhile.
        let mut dropped = false;
        let lost = tokio::time::sleep(MOVE_GRACE);
        tokio::pin!(lost);
        loop {
            let talking = capture.as_ref().is_some_and(Capture::talking);
            let participants = discord_participants(
                &discord_states.borrow(),
                &channel,
                &me,
                state,
                &hearing.speaking(),
                talking,
            );
            if participants != state.participants {
                state.participants = participants;
                snapshots.send_replace(state.clone());
            }
            if unrung && !alone(&discord_states.borrow(), &channel, &me) {
                unrung = false;
                state.warning = None;
                snapshots.send_replace(state.clone());
            }
            let calling = guild == PRIVATE
                && !state.deafened
                && alone(&discord_states.borrow(), &channel, &me)
                && state.participants.iter().any(|person| !person.connected);
            if calling != ringback.is_some() {
                ringback = calling
                    .then(|| Ringer::start(Tone::Outgoing, state.speaker.as_deref()))
                    .and_then(|started| started.inspect_err(|error| tracing::warn!(%error, "could not play the ringback")).ok());
            }
            tokio::select! {
                command = commands.recv() => {
                    match command {
                        None | Some(Command::Leave) => break,
                        Some(Command::Join(_)) => state.error = Some("Leave your current voice channel before joining another.".into()),
                        Some(Command::Mute(true)) => {
                            capture = None;
                            state.muted = true;
                        }
                        Some(Command::Mute(false)) => {
                            // Unmuting while deafened means wanting to talk, which needs hearing too.
                            if state.deafened {
                                state.deafened = false;
                                hearing.set_deafened(false);
                                Watch::hear(&mut watch, true);
                            }
                            match open_discord_microphone(state, &reverse, &microphone, &track) {
                                Ok(opened) => {
                                    capture = Some(opened);
                                    state.muted = false;
                                    state.error = None;
                                }
                                Err(error) => {
                                    state.muted = true;
                                    state.error = Some(error.to_string());
                                }
                            }
                        }
                        Some(Command::Deafen(deafened)) => {
                            if deafened {
                                capture = None;
                                state.muted = true;
                            }
                            state.deafened = deafened;
                            hearing.set_deafened(deafened);
                            Watch::hear(&mut watch, !deafened);
                        }
                        Some(Command::Devices) => { let _ = devices(state); }
                        Some(Command::Microphone(id)) => {
                            state.microphone = Some(id);
                            Chosen::write(remembered, state);
                            if capture.take().is_some() {
                                match open_discord_microphone(state, &reverse, &microphone, &track) {
                                    Ok(opened) => capture = Some(opened),
                                    Err(_) => {
                                        state.muted = true;
                                        state.error = Some("Could not switch microphone. Refresh devices and choose another.".into());
                                    }
                                }
                            }
                        }
                        Some(Command::Speaker(id)) => {
                            state.speaker = Some(id);
                            Chosen::write(remembered, state);
                            speakers = None;
                            match open_discord_speakers(state, &played, &hearing) {
                                Ok(opened) => speakers = Some(opened),
                                Err(_) => state.error = Some("Could not switch speakers. Refresh devices and choose another.".into()),
                            }
                        }
                        Some(Command::Camera(id)) => {
                            state.camera = Some(id);
                            Chosen::write(remembered, state);
                        }
                        Some(Command::GoLive(quality)) => {
                            if share.is_none() {
                                state.quality = quality;
                                Chosen::write(remembered, state);
                                match Share::start(state, quality) {
                                    Ok(started) => {
                                        share = Some(started);
                                        state.error = None;
                                    }
                                    Err(error) => state.error = Some(format!("{error:#}")),
                                }
                            }
                        }
                        Some(Command::StopLive) => {
                            if let Some(stopped) = share.take() {
                                stopped.stop(discord, state);
                            }
                            starting = None;
                        }
                        Some(Command::Watch(user)) => {
                            let watching = state.watching.as_ref().is_some_and(|watching| watching.user == user);
                            if user != me && !watching {
                                if let Some(stopped) = watch.take() {
                                    stopped.stop(discord, state);
                                }
                                state.error = None;
                                watch = Some(Watch::start(discord, guild, &channel, &user, state));
                            }
                        }
                        Some(Command::StopWatching) => {
                            if let Some(stopped) = watch.take() {
                                stopped.stop(discord, state);
                            }
                        }
                        // Camera video in Discord calls comes with their own voice connection.
                        Some(Command::LeaveStale | Command::Video(_)) => {}
                    }
                    discord.set_self(guild, &channel, state.muted, state.deafened);
                    snapshots.send_replace(state.clone());
                }
                report = next_report(&mut share) => {
                    match report {
                        Some(Report::Ready) => {
                            if let Some(sharing) = &state.sharing {
                                sharing.frames.unlink();
                            }
                        }
                        Some(Report::Sharing { .. }) => {
                            if let Some(sharing) = state.sharing.as_mut()
                                && sharing.phase == SharePhase::Choosing
                            {
                                sharing.phase = SharePhase::Starting;
                                if let Some(going) = share.as_mut() {
                                    going.key = Some(stream::key(guild, &channel, &me));
                                }
                                let (discord, guild, channel) = (discord.clone(), guild.to_owned(), channel.to_owned());
                                starting = Some(Box::pin(async move { discord.go_live(&guild, &channel).await }));
                            }
                        }
                        Some(Report::Streaming) => {
                            if let Some(sharing) = state.sharing.as_mut() {
                                sharing.phase = SharePhase::Live;
                            }
                        }
                        Some(Report::Preview { thumbnail }) => {
                            if let Some(going) = share.as_mut() {
                                going.show_preview(discord, thumbnail);
                            }
                        }
                        // Only a video's worker reports these.
                        Some(Report::Opened { .. } | Report::Finished) => {}
                        Some(Report::Ended { error }) => {
                            if let Some(stopped) = share.take() {
                                stopped.stop(discord, state);
                            }
                            starting = None;
                            state.error = error;
                        }
                        // The reports end with Ended, so this is a worker gone quietly.
                        None => {
                            if let Some(stopped) = share.take() {
                                stopped.stop(discord, state);
                            }
                            starting = None;
                        }
                    }
                    snapshots.send_replace(state.clone());
                }
                result = started(&mut starting) => {
                    starting = None;
                    match result {
                        // The worker sends the video there, then reports it streaming.
                        Ok(server) => {
                            if let Some(going) = share.as_mut()
                                && let Err(error) = going.worker.send(&server)
                                && let Some(stopped) = share.take()
                            {
                                stopped.stop(discord, state);
                                state.error = Some(error.to_string());
                            }
                        }
                        Err(error) => {
                            if let Some(stopped) = share.take() {
                                stopped.stop(discord, state);
                            }
                            state.error = Some(error.to_string());
                        }
                    }
                    snapshots.send_replace(state.clone());
                }
                result = rung(&mut ringing) => {
                    ringing = None;
                    let warning = match result {
                        Ok(true) => None,
                        Ok(false) => Some(format!(
                            "Discord doesn't let you ring {}. They'll see the call when they open the DM.",
                            state.name
                        )),
                        Err(error) => {
                            tracing::warn!(error = format!("{error:#}"), "could not ring the DM call");
                            Some(format!("Discord didn't ring them: {error}"))
                        }
                    };
                    if warning.is_some() && alone(&discord_states.borrow(), &channel, &me) {
                        unrung = true;
                        state.warning = warning;
                        snapshots.send_replace(state.clone());
                    }
                }
                Ok(()) = volumes.changed() => Watch::volume(&mut watch),
                event = watch_event(&mut watch) => {
                    let mut stop = None;
                    match event {
                        WatchEvent::Opened(Ok(server)) => {
                            if let Some(watching) = watch.as_mut()
                                && let Err(error) = watching.open(&server, state)
                            {
                                stop = Some(Some(format!("{error:#}")));
                            }
                        }
                        WatchEvent::Opened(Err(error)) => stop = Some(Some(error.to_string())),
                        WatchEvent::Report(Some(Report::Ready)) => {
                            if let Some(frames) = state.watching.as_ref().and_then(|watching| watching.frames.as_ref()) {
                                frames.unlink();
                            }
                        }
                        WatchEvent::Report(Some(Report::Streaming)) => {
                            if let Some(watching) = state.watching.as_mut() {
                                watching.opening = false;
                            }
                        }
                        WatchEvent::Report(Some(Report::Ended { error })) => stop = Some(error),
                        WatchEvent::Report(Some(
                            Report::Sharing { .. }
                            | Report::Preview { .. }
                            | Report::Opened { .. }
                            | Report::Finished,
                        )) => {}
                        WatchEvent::Report(None) => stop = Some(None),
                    }
                    if let Some(error) = stop
                        && let Some(stopped) = watch.take()
                    {
                        stopped.stop(discord, state);
                        state.error = error;
                    }
                    snapshots.send_replace(state.clone());
                }
                Ok(event) = live.recv() => match event {
                    Live::StreamEnded { stream_key, reason } => {
                        // The streamer stopped, or Discord ended watching.
                        if watch.as_ref().is_some_and(|watch| watch.key == stream_key)
                            && let Some(stopped) = watch.take()
                        {
                            stopped.stop(discord, state);
                            snapshots.send_replace(state.clone());
                        }
                        if share.as_ref().is_some_and(|share| share.key.as_ref() == Some(&stream_key))
                            && let Some(stopped) = share.take()
                        {
                            stopped.stop(discord, state);
                            if reason.as_deref() != Some("user_requested") {
                                state.error = Some("Discord ended your stream.".into());
                            }
                            snapshots.send_replace(state.clone());
                        }
                    }
                    // Someone moved this account to another channel of the
                    // server; Discord names its voice server next.
                    Live::VoiceChannel { guild: moved, channel: to } if moved == guild && to != channel => {
                        channel = to;
                        moved_to(state, &discord_states.borrow(), guild, &channel);
                        snapshots.send_replace(state.clone());
                    }
                    // Discord moved the call to another voice server: the
                    // call carries on there.
                    Live::VoiceMoved { guild: moved, server: next } if moved == guild => {
                        if dropped || next.endpoint != server.endpoint || next.token != server.token {
                            tracing::info!("Discord moved the call to another voice server");
                            match tokio::time::timeout(MOVE_TIMEOUT, call.move_to(&next, &channel)).await {
                                Ok(Ok(())) => {
                                    server = next;
                                    dropped = false;
                                }
                                Ok(Err(error)) => {
                                    tracing::warn!(error = format!("{error:#}"), "could not follow the call to its new voice server");
                                    bail!("Discord moved the call to another voice server, and Talk couldn't connect to it. Join again.");
                                }
                                Err(_) => bail!("Discord moved the call to another voice server, which took too long to answer. Join again."),
                            }
                        }
                    }
                    // Or a stream this account shares or watches.
                    Live::StreamMoved(next) => {
                        if starting.is_none()
                            && let Some(going) = share.as_mut().filter(|going| going.key.as_ref() == Some(&next.stream_key))
                            && let Err(error) = going.worker.send(&next)
                            && let Some(stopped) = share.take()
                        {
                            stopped.stop(discord, state);
                            state.error = Some(error.to_string());
                        }
                        if let Some(watching) = watch.as_mut().filter(|watching| watching.key == next.stream_key && watching.opening.is_none()) {
                            if let Some(shown) = state.watching.as_mut() {
                                shown.opening = true;
                            }
                            if let Err(error) = watching.open(&next, state)
                                && let Some(stopped) = watch.take()
                            {
                                stopped.stop(discord, state);
                                state.error = Some(format!("{error:#}"));
                            }
                        }
                        snapshots.send_replace(state.clone());
                    }
                    _ => {}
                },
                Some(reason) = endings.recv() => {
                    // Discord ends the connection when it moves the call,
                    // perhaps before it says where to.
                    tracing::warn!(%reason, "Discord voice disconnected");
                    dropped = true;
                    lost.as_mut().reset(tokio::time::Instant::now() + MOVE_GRACE);
                }
                () = &mut lost, if dropped => {
                    bail!("Discord voice disconnected. Join again to reconnect.");
                }
                changed = discord_states.changed() => {
                    if changed.is_err() || discord_states.borrow().status != Status::Online {
                        bail!("Lost the connection to Discord. Join again when it's back.");
                    }
                    // A channel made for the call may be named only now.
                    if let Some(name) = call_name(&discord_states.borrow(), guild, &channel)
                        && name != state.name
                    {
                        state.name = name.to_owned();
                        snapshots.send_replace(state.clone());
                    }
                }
                _ = ticks.tick() => {}
            }
        }
        drop(speakers);
        Ok(())
    }
    .await;
    if let Some(stopped) = share.take() {
        stopped.stop(discord, state);
    }
    if let Some(stopped) = watch.take() {
        stopped.stop(discord, state);
    }
    state.phase = Phase::Leaving;
    snapshots.send_replace(state.clone());
    drop(ringback);
    drop(capture);
    hearing.set_output(None);
    call.leave();
    discord.leave(guild);
    result
}

/// How long a dropped voice connection waits for Discord to say it moved
/// the call, before the call ends.
const MOVE_GRACE: Duration = Duration::from_secs(5);
/// How long connecting to the voice server a call moved to may take.
const MOVE_TIMEOUT: Duration = Duration::from_secs(15);

/// Opens the chosen speakers, or the system default if they're gone.
fn open_discord_speakers(
    state: &State,
    played: &std::sync::mpsc::SyncSender<Vec<i16>>,
    hearing: &Hearing,
) -> Result<Speakers> {
    let (speakers, output) = match discord_audio::speakers(state.speaker.as_deref(), played.clone())
    {
        Ok(opened) => opened,
        Err(error) if state.speaker.is_some() => {
            tracing::warn!(%error, "using the default speakers");
            discord_audio::speakers(None, played.clone())?
        }
        Err(error) => return Err(error),
    };
    hearing.set_output(Some(output));
    Ok(speakers)
}

/// Opens the chosen microphone, or the system default if it's gone.
fn open_discord_microphone(
    state: &State,
    reverse: &discord_audio::Reverse,
    microphone: &talk_discord::voice::Microphone,
    track: &talk_discord::voice::Track,
) -> Result<Capture> {
    let open = |id: Option<&str>| {
        discord_audio::capture(id, reverse.clone(), microphone.clone(), track.clone())
    };
    match open(state.microphone.as_deref()) {
        Ok(capture) => Ok(capture),
        Err(error) if state.microphone.is_some() => {
            tracing::warn!(%error, "using the default microphone");
            open(None)
        }
        Err(error) => Err(error),
    }
}

/// Everyone in the Discord voice channel, this account first.
fn discord_participants(
    snapshot: &talk_discord::service::Snapshot,
    channel: &str,
    me: &str,
    state: &State,
    speaking: &[u64],
    talking: bool,
) -> Vec<Participant> {
    let mut participants: Vec<Participant> = snapshot
        .voice
        .get(channel)
        .into_iter()
        .flatten()
        .map(|member| {
            let local = member.user_id == me;
            Participant {
                id: member.user_id.clone(),
                user: member.name.clone(),
                person: member.user_id.clone(),
                local,
                muted: if local { state.muted } else { member.muted },
                deafened: if local {
                    state.deafened
                } else {
                    member.deafened
                },
                speaking: if local {
                    talking && !state.muted
                } else {
                    member
                        .user_id
                        .parse()
                        .is_ok_and(|user: u64| speaking.contains(&user))
                },
                connected: true,
                problem: None,
                avatar: Some(member.avatar.clone()),
                streaming: member.streaming,
                video: None,
            }
        })
        .collect();
    // Those a DM call rings, until they answer or it stops ringing them.
    let ringing = snapshot
        .conversations
        .iter()
        .find(|conversation| conversation.id == channel)
        .map_or(&[][..], |conversation| conversation.ringing.as_slice());
    for member in ringing {
        if member.user_id != me
            && !participants
                .iter()
                .any(|person| person.id == member.user_id)
        {
            participants.push(Participant {
                id: member.user_id.clone(),
                user: member.name.clone(),
                person: member.user_id.clone(),
                local: false,
                muted: false,
                deafened: false,
                speaking: false,
                connected: false,
                problem: Some("Calling...".into()),
                avatar: Some(member.avatar.clone()),
                streaming: false,
                video: None,
            });
        }
    }
    participants.sort_by_key(|person| std::cmp::Reverse(person.local));
    participants
}

/// Shows the call in `channel`, where someone moved this account.
fn moved_to(state: &mut State, snapshot: &Snapshot, guild: &str, channel: &str) {
    tracing::info!("moved to another voice channel");
    state.room = Some(crate::discord::room_id(guild, channel));
    if let Some(name) = call_name(snapshot, guild, channel) {
        state.name = name.to_owned();
    }
}

/// The name of the voice channel or DM call, once Discord said it.
fn call_name<'a>(snapshot: &'a Snapshot, guild: &str, channel: &str) -> Option<&'a str> {
    if guild == PRIVATE {
        snapshot
            .conversations
            .iter()
            .find(|found| found.id == channel)
            .map(|found| found.name.as_str())
    } else {
        snapshot
            .servers
            .iter()
            .flat_map(|server| &server.voice_channels)
            .find(|found| found.id == channel)
            .map(|found| found.name.as_str())
    }
}

/// Whether nobody but this account is in a call.
fn alone(snapshot: &talk_discord::service::Snapshot, channel: &str, me: &str) -> bool {
    snapshot
        .voice
        .get(channel)
        .into_iter()
        .flatten()
        .all(|member| member.user_id == me)
}
