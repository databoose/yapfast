//! A call's media connection: this account's microphone out as Opus, and
//! everyone else's in, decoded per speaker for Talk to mix; and cameras
//! both ways (`call_video.rs`). It runs on a thread of its own with a small
//! runtime, as a Go Live stream does, over the same voice connection
//! (`voice_connection.rs`).

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc as channel,
    },
    time::Duration,
};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, MissedTickBehavior},
};

use crate::{
    call_stats::{CallStats, PING_EVERY, Reported},
    call_video::{self, Camera, Receiving, VideoFrame},
    dave::Dave,
    opus, rtp,
    voice::{FRAME, Listener},
    voice_connection::{Closed, Connection, Identity, lock, receive, send_json},
};

/// The microphone's bit rate and mode: stereo at 128 kbit/s in Opus's music
/// mode, as songbird sent it when it ran calls, and as Talk's voice
/// processing was tuned with.
const BITRATE: i32 = 128_000;
/// 20 ms of Opus silence, which Discord's clients send five of when they
/// stop talking.
const SILENCE: [u8; 3] = [0xf8, 0xff, 0xfe];
const SILENT_FRAMES: u8 = 5;
/// Op 5's flag for a microphone.
const MICROPHONE: u8 = 1;
/// How often UDP carries a keepalive, so a quiet connection stays open
/// through NAT, as songbird, which ran calls before, sent one.
const KEEPALIVE: Duration = Duration::from_secs(5);
/// A speaker's audio starts playing once this many packets wait: 60 ms.
const START: usize = 3;
/// At most this many packets wait, 400 ms; beyond, the oldest go, so a
/// stall never turns into lasting delay.
const MOST: usize = 20;
/// A speaker missing this many packets in a row has stopped: 100 ms.
const GONE: u8 = 5;
/// A speaker heard from no more in this long gives up their decoder.
const FORGET: Duration = Duration::from_secs(10);
/// A tick this late means the call stalled rather than woke late: its
/// schedule starts again from now instead of rushing out what it missed.
const STALLED: Duration = Duration::from_millis(100);
/// While a speaker's audio is dropped for one reason, it's logged again
/// after this many more packets: 10 s.
const STILL_DROPPING: u32 = 500;
/// Gateway drops in a row that resuming or reconnecting may follow.
const RETRIES: u32 = 3;

/// What the microphone sends: written by Talk, read every 20 ms.
pub(crate) struct Sending {
    pub buffer: Arc<Mutex<VecDeque<f32>>>,
    pub playing: Arc<AtomicBool>,
}

/// Orders for a running call.
pub(crate) enum Order {
    /// Connect to another voice server, as for another channel.
    Move(Identity, oneshot::Sender<Result<()>>),
    /// Send the camera's frames, or stop.
    Film(Option<Camera>),
    /// Ask for these people's cameras, at a quality from 0 to 100 and the
    /// pixels they show at; no others.
    Want(HashMap<u64, (u8, u32)>),
    Leave,
}

/// Starts a call on a thread of its own. The receiver says whether the
/// first connection succeeded.
pub(crate) fn start(
    identity: Identity,
    listener: Arc<dyn Listener>,
    sending: Arc<Mutex<Option<Sending>>>,
    video: channel::SyncSender<VideoFrame>,
    stats: Arc<CallStats>,
) -> Result<(mpsc::UnboundedSender<Order>, oneshot::Receiver<Result<()>>)> {
    let (orders, ordered) = mpsc::unbounded_channel();
    let (ready, connected) = oneshot::channel();
    std::thread::Builder::new()
        .name("talk-discord-call".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready.send(Err(error.into()));
                    return;
                }
            };
            runtime.block_on(run(
                identity, listener, sending, video, stats, ordered, ready,
            ));
        })?;
    Ok((orders, connected))
}

/// Why the media loop stopped.
enum Ended {
    Left,
    Moved(Identity, oneshot::Sender<Result<()>>),
    Dropped(anyhow::Error),
}

async fn run(
    mut identity: Identity,
    listener: Arc<dyn Listener>,
    sending: Arc<Mutex<Option<Sending>>>,
    video: channel::SyncSender<VideoFrame>,
    stats: Arc<CallStats>,
    mut orders: mpsc::UnboundedReceiver<Order>,
    ready: oneshot::Sender<Result<()>>,
) {
    let mut connection = match Connection::open(&identity, codecs(), &[]).await {
        Ok(connection) => {
            let _ = ready.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let mut media = match Media::new(video, stats) {
        Ok(media) => media,
        Err(error) => {
            listener.disconnected(&format!("{error:#}"));
            return;
        }
    };
    let mut drops = 0;
    loop {
        match media
            .run(&mut connection, &listener, &sending, &mut orders)
            .await
        {
            Ended::Left => {
                let _ = connection.ws.close(None).await;
                return;
            }
            Ended::Moved(to, reply) => {
                let _ = connection.ws.close(None).await;
                match Connection::open(&to, codecs(), &[]).await {
                    Ok(moved) => {
                        connection = moved;
                        identity = to;
                        media.reset();
                        drops = 0;
                        let _ = reply.send(Ok(()));
                    }
                    Err(error) => {
                        let reason = format!("{error:#}");
                        let _ = reply.send(Err(error));
                        listener.disconnected(&reason);
                        return;
                    }
                }
            }
            Ended::Dropped(error) => {
                drops += 1;
                let code = error
                    .downcast_ref::<Closed>()
                    .and_then(|closed| closed.code);
                // Disconnected from the call, or the session ended: final.
                if matches!(code, Some(4014 | 4006)) || drops > RETRIES {
                    listener.disconnected(&format!("{error:#}"));
                    return;
                }
                tracing::info!(
                    error = format!("{error:#}"),
                    "the call's voice connection dropped"
                );
                if let Err(resume) = connection.resume(&identity).await {
                    tracing::info!(
                        error = format!("{resume:#}"),
                        "couldn't resume the call; connecting again"
                    );
                    match Connection::open(&identity, codecs(), &[]).await {
                        Ok(again) => {
                            connection = again;
                            media.reset();
                        }
                        Err(again) => {
                            listener.disconnected(&format!("{error:#}; then {again:#}"));
                            return;
                        }
                    }
                }
            }
        }
    }
}

/// What a call offers: Opus, H264 for the camera both ways, and VP8 to
/// receive.
fn codecs() -> Value {
    json!([
        {"name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::OPUS},
        {"name": "H264", "type": "video", "priority": 1000, "payload_type": rtp::H264,
         "rtx_payload_type": rtp::H264_RTX, "encode": true, "decode": true},
        {"name": "VP8", "type": "video", "priority": 2000, "payload_type": rtp::VP8,
         "rtx_payload_type": rtp::VP8_RTX, "encode": false, "decode": true},
    ])
}

/// The call's audio and video, kept across moves and reconnections.
struct Media {
    encoder: opus::Encoder,
    /// One frame of the microphone, in both channels, and its Opus packet.
    pcm: Vec<f32>,
    packet: Vec<u8>,
    sequence: u16,
    timestamp: u32,
    /// Whether op 5 says this account is speaking, and the silent frames
    /// still to send since it stopped.
    speaking: bool,
    silent: u8,
    /// Everyone else, by their audio SSRC, and whose each SSRC is.
    speakers: HashMap<u32, Speaker>,
    users: HashMap<u32, u64>,
    /// Why each SSRC's audio is being dropped and how many packets have
    /// gone, and whether the microphone was last sent end-to-end encrypted,
    /// as logged.
    dropped: HashMap<u32, (String, u32)>,
    encrypting: Option<bool>,
    /// This account's camera, while it films, and everyone else's.
    filming: Option<call_video::Sending>,
    watching: Receiving,
    /// How the connection is doing, for Talk to show, and what Discord's
    /// last receiver reports said about this account's packets.
    stats: Arc<CallStats>,
    reported: Reported,
}

impl Media {
    fn new(video: channel::SyncSender<VideoFrame>, stats: Arc<CallStats>) -> Result<Self> {
        Ok(Self {
            encoder: opus::Encoder::new(2, opus::Application::Audio, BITRATE)?,
            pcm: vec![0.0; FRAME * 2],
            packet: vec![0; 1500],
            sequence: rand::random(),
            timestamp: rand::random(),
            speaking: false,
            silent: 0,
            speakers: HashMap::new(),
            users: HashMap::new(),
            dropped: HashMap::new(),
            encrypting: None,
            filming: None,
            watching: Receiving::new(video),
            stats,
            reported: Reported::default(),
        })
    }

    /// A new connection: speakers, cameras and their SSRCs start over.
    fn reset(&mut self) {
        self.speakers.clear();
        self.users.clear();
        self.dropped.clear();
        self.encrypting = None;
        self.speaking = false;
        self.silent = 0;
        self.watching.reset();
        self.reported.reset();
    }

    async fn run(
        &mut self,
        connection: &mut Connection,
        listener: &Arc<dyn Listener>,
        sending: &Mutex<Option<Sending>>,
        orders: &mut mpsc::UnboundedReceiver<Order>,
    ) -> Ended {
        for (op, data) in std::mem::take(&mut connection.early) {
            self.learn(op, &data);
            self.watching.learn(op, &data);
        }
        // Each heartbeat measures the ping, which Discord's web client
        // measures this often.
        let interval = connection.interval.min(PING_EVERY);
        let mut heartbeat = tokio::time::interval_at(Instant::now() + interval, interval);
        let mut ticks = tokio::time::interval(Duration::from_millis(20));
        // Windows wakes timers on its 15.6 ms clock, so a 20 ms tick often
        // comes at 31 ms. Catching up on the ticks missed keeps 50 frames a
        // second each way; delaying them sent and played two thirds of that.
        ticks.set_missed_tick_behavior(MissedTickBehavior::Burst);
        let mut keepalive = tokio::time::interval(KEEPALIVE);
        let mut datagram = vec![0u8; 2048];
        let result: Result<Ended> = async {
            // A new connection hears about the camera again, and what's
            // wanted of others'.
            if let Some(filming) = self.filming.as_mut() {
                filming.announce(connection).await?;
            }
            self.watching.ask(connection).await?;
            loop {
                tokio::select! {
                    order = orders.recv() => match order {
                        None | Some(Order::Leave) => return Ok(Ended::Left),
                        Some(Order::Move(to, reply)) => return Ok(Ended::Moved(to, reply)),
                        Some(Order::Film(Some(camera))) => {
                            let mut filming = call_video::Sending::new(camera, connection);
                            filming.announce(connection).await?;
                            self.filming = Some(filming);
                        }
                        Some(Order::Film(None)) => {
                            if self.filming.take().is_some() {
                                call_video::Sending::clear(connection).await?;
                            }
                        }
                        Some(Order::Want(wants)) => {
                            self.watching.want(wants);
                            self.watching.ask(connection).await?;
                        }
                    },
                    frame = async {
                        match self.filming.as_mut() {
                            Some(filming) => filming.next().await,
                            None => std::future::pending().await,
                        }
                    } => match (frame, self.filming.as_mut()) {
                        (Some(frame), Some(filming)) => filming.send(connection, frame).await?,
                        // The camera closed.
                        _ => {
                            self.filming = None;
                            call_video::Sending::clear(connection).await?;
                        }
                    },
                    due = ticks.tick() => {
                        if due.elapsed() > STALLED {
                            ticks.reset();
                        }
                        self.send(connection, sending).await?;
                        self.hear(listener);
                        self.watching.repair(connection).await?;
                    }
                    _ = heartbeat.tick() => {
                        connection.heartbeat().await?;
                        self.secure(connection);
                    }
                    _ = keepalive.tick() => {
                        let mut packet = [0u8; 8];
                        packet[..4].copy_from_slice(&connection.ssrc.to_be_bytes());
                        connection.udp.send(&packet).await?;
                    }
                    message = receive(&mut connection.ws, &mut connection.seq) => {
                        let handled = connection.handle(message?).await?;
                        // DAVE's messages change the group, and its code.
                        self.secure(connection);
                        if let Some((op, data)) = handled {
                            if op == 6 && let Some(round_trip) = Connection::acked(&data) {
                                self.stats.ping(round_trip);
                            }
                            self.learn(op, &data);
                            if self.watching.learn(op, &data) {
                                self.watching.ask(connection).await?;
                            }
                        }
                    }
                    length = connection.udp.recv(&mut datagram) => {
                        self.receive(connection, &datagram[..length?]).await?;
                    }
                }
            }
        }
        .await;
        result.unwrap_or_else(Ended::Dropped)
    }

    /// Notes whether frames leave end-to-end encrypted, and the group's
    /// privacy code.
    fn secure(&self, connection: &Connection) {
        self.stats
            .encryption(connection.dave.encrypting(), connection.dave.privacy_code());
    }

    /// Learns whose audio each SSRC carries from their speaking messages
    /// and their video ones. Someone already talking when Talk joins may
    /// send no speaking message again; their video message, sent for
    /// everyone in the call, names their audio too.
    fn learn(&mut self, op: u64, data: &Value) {
        let ssrc = match op {
            5 => &data["ssrc"],
            12 => &data["audio_ssrc"],
            _ => return,
        };
        if let (Some(user), Some(ssrc)) = (
            data["user_id"].as_str().and_then(|id| id.parse().ok()),
            ssrc.as_u64().filter(|&ssrc| ssrc != 0),
        ) {
            self.users.insert(ssrc as u32, user);
        }
    }

    /// Logs why a speaker's audio is dropped when the reason changes, and
    /// every 10 s while it lasts, since it otherwise vanishes unheard.
    fn report_drop(&mut self, ssrc: u32, reason: &str, dave: &Dave) {
        match self.dropped.get_mut(&ssrc) {
            Some((last, count)) if last == reason => {
                *count += 1;
                if *count % STILL_DROPPING == 0 {
                    tracing::info!(
                        ssrc,
                        reason,
                        dropped = *count,
                        dave = dave.status(),
                        "still dropping a speaker's audio in a call"
                    );
                }
            }
            _ => {
                tracing::info!(
                    ssrc,
                    reason,
                    dave = dave.status(),
                    "dropping a speaker's audio in a call"
                );
                self.dropped.insert(ssrc, (reason.to_owned(), 1));
            }
        }
    }

    /// Sends 20 ms of the microphone while it plays, then five silent
    /// frames when it stops.
    async fn send(
        &mut self,
        connection: &mut Connection,
        sending: &Mutex<Option<Sending>>,
    ) -> Result<()> {
        let playing = {
            let sending = lock(sending);
            match sending.as_ref() {
                Some(sending) if sending.playing.load(Ordering::Relaxed) => {
                    let mut buffer = lock(&sending.buffer);
                    let count = buffer.len().min(FRAME);
                    for (out, sample) in self.pcm.chunks_exact_mut(2).zip(buffer.drain(..count)) {
                        out.fill(sample);
                    }
                    self.pcm[count * 2..].fill(0.0);
                    true
                }
                _ => false,
            }
        };
        let marker = playing && !self.speaking;
        if marker {
            send_json(
                &mut connection.ws,
                5,
                json!({"speaking": MICROPHONE, "delay": 0, "ssrc": connection.ssrc}),
            )
            .await?;
            self.speaking = true;
        }
        if playing && self.encrypting != Some(connection.dave.encrypting()) {
            self.encrypting = Some(connection.dave.encrypting());
            tracing::info!(
                encrypting = connection.dave.encrypting(),
                dave = connection.dave.status(),
                "the call's microphone is sending"
            );
        }
        if playing {
            let length = self.encoder.encode_float(&self.pcm, &mut self.packet)?;
            let opus = &self.packet[..length];
            let payload = connection.dave.encrypt_opus(opus);
            self.transmit(connection, &payload, marker).await?;
            self.silent = SILENT_FRAMES;
        } else if self.silent > 0 {
            let payload = connection.dave.encrypt_opus(&SILENCE);
            self.transmit(connection, &payload, false).await?;
            self.silent -= 1;
            if self.silent == 0 {
                self.speaking = false;
                send_json(
                    &mut connection.ws,
                    5,
                    json!({"speaking": 0, "delay": 0, "ssrc": connection.ssrc}),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn transmit(
        &mut self,
        connection: &mut Connection,
        payload: &[u8],
        marker: bool,
    ) -> Result<()> {
        let mut packet = rtp::opus_packet(
            connection.ssrc,
            self.sequence,
            self.timestamp,
            marker,
            payload,
        );
        self.sequence = self.sequence.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(FRAME as u32);
        let clear = rtp::rtp_clear_len(&packet).unwrap_or(packet.len());
        connection.transport.seal(&mut packet, clear)?;
        connection.udp.send(&packet).await?;
        Ok(())
    }

    /// Queues a received Opus packet for its speaker, hands a camera's
    /// packet on, and reads RTCP about this account's packets: their loss,
    /// and what the camera's viewers ask of it.
    async fn receive(&mut self, connection: &mut Connection, datagram: &[u8]) -> Result<()> {
        if rtp::is_rtcp(datagram) {
            if let Some(rtcp) = connection.transport.open(datagram, rtp::RTCP_CLEAR) {
                let ours = [connection.ssrc, connection.video];
                self.reported.learn(&self.stats, &rtcp, &ours);
                if let Some(filming) = self.filming.as_mut() {
                    filming.feedback(connection, &rtcp).await?;
                }
            }
            return Ok(());
        }
        let Some(clear) = rtp::rtp_clear_len(datagram) else {
            return Ok(());
        };
        let Some(plain) = connection.transport.open(datagram, clear) else {
            return Ok(());
        };
        let Some(packet) = rtp::parse(&plain) else {
            return Ok(());
        };
        if packet.payload_type != rtp::OPUS {
            self.watching.receive(connection, &packet).await?;
            return Ok(());
        }
        // DAVE keys each sender's frames by who they are, which only the
        // voice gateway tells.
        let Some(&user) = self.users.get(&packet.ssrc) else {
            self.report_drop(packet.ssrc, "whose SSRC it is is unknown", &connection.dave);
            return Ok(());
        };
        // Silence passes in the clear even while sealed audio fails.
        let sealed = packet.payload.ends_with(&[0xfa, 0xfa]);
        let Some(opus) = connection.dave.decrypt_opus(user, packet.payload.to_vec()) else {
            let reason = connection.dave.unreadable.take().unwrap_or_default();
            self.report_drop(packet.ssrc, &reason, &connection.dave);
            return Ok(());
        };
        if sealed && let Some((reason, dropped)) = self.dropped.remove(&packet.ssrc) {
            tracing::info!(
                ssrc = packet.ssrc,
                user,
                reason,
                dropped,
                "hearing a speaker again in a call"
            );
        }
        let speaker = match self.speakers.entry(packet.ssrc) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => match Speaker::new(user) {
                Ok(speaker) => entry.insert(speaker),
                Err(error) => {
                    tracing::warn!(%error, "couldn't decode a speaker");
                    return Ok(());
                }
            },
        };
        speaker.user = user;
        speaker.push(packet.sequence, opus);
        Ok(())
    }

    /// Plays everyone's next 20 ms, and forgets those long gone.
    fn hear(&mut self, listener: &Arc<dyn Listener>) {
        let now = std::time::Instant::now();
        self.speakers
            .retain(|_, speaker| now.duration_since(speaker.heard) < FORGET);
        let voices: Vec<(u64, &[i16])> = self
            .speakers
            .values_mut()
            .filter_map(|speaker| Some((speaker.user, speaker.next()?)))
            .collect();
        listener.tick(&voices);
    }
}

/// One speaker's audio on its way to the mix: packets in order, waiting a
/// little for those that come late, and decoded one 20 ms frame a tick.
struct Speaker {
    user: u64,
    decoder: opus::Decoder,
    /// Packets by sequence number, oldest first.
    waiting: VecDeque<(u16, Vec<u8>)>,
    /// The sequence number to play next, while playing.
    next: Option<u16>,
    /// Packets missing in a row.
    missing: u8,
    /// The last frame decoded, interleaved stereo.
    pcm: Vec<i16>,
    heard: std::time::Instant,
}

/// Whether sequence number `a` comes before `b`, across wrapping.
fn before(a: u16, b: u16) -> bool {
    a != b && b.wrapping_sub(a) < 0x8000
}

impl Speaker {
    fn new(user: u64) -> Result<Self> {
        Ok(Self {
            user,
            decoder: opus::Decoder::new(crate::voice::CHANNELS)?,
            waiting: VecDeque::with_capacity(MOST + 1),
            next: None,
            missing: 0,
            pcm: vec![0; FRAME * usize::from(crate::voice::CHANNELS)],
            heard: std::time::Instant::now(),
        })
    }

    fn push(&mut self, sequence: u16, opus: Vec<u8>) {
        self.heard = std::time::Instant::now();
        if self.next.is_some_and(|next| before(sequence, next)) {
            return;
        }
        let at = self
            .waiting
            .iter()
            .rposition(|(waiting, _)| !before(sequence, *waiting))
            .map_or(0, |at| at + 1);
        if at > 0 && self.waiting[at - 1].0 == sequence {
            return;
        }
        self.waiting.insert(at, (sequence, opus));
        while self.waiting.len() > MOST {
            self.waiting.pop_front();
            self.next = self.waiting.front().map(|(sequence, _)| *sequence);
        }
    }

    /// The next 20 ms, or nothing while waiting to start or once stopped.
    fn next(&mut self) -> Option<&[i16]> {
        let next = match self.next {
            Some(next) => next,
            None if self.waiting.len() >= START => self.waiting.front()?.0,
            None => return None,
        };
        while self
            .waiting
            .front()
            .is_some_and(|(sequence, _)| before(*sequence, next))
        {
            self.waiting.pop_front();
        }
        self.next = Some(next.wrapping_add(1));
        let decoded = match self.waiting.front() {
            Some((sequence, _)) if *sequence == next => {
                let (_, opus) = self.waiting.pop_front()?;
                self.missing = 0;
                self.decoder.decode(&opus, &mut self.pcm, false)
            }
            waiting => {
                self.missing += 1;
                if self.missing >= GONE && waiting.is_none() {
                    // Stopped talking: wait for them to start again.
                    self.next = None;
                    self.missing = 0;
                    return None;
                }
                match waiting {
                    // The next packet carries this one's forward error
                    // correction.
                    Some((sequence, opus)) if *sequence == next.wrapping_add(1) => {
                        self.decoder.decode(opus, &mut self.pcm, true)
                    }
                    _ => self.decoder.conceal(&mut self.pcm),
                }
            }
        };
        match decoded {
            Ok(samples) => Some(&self.pcm[..samples * usize::from(crate::voice::CHANNELS)]),
            Err(error) => {
                tracing::debug!(%error, "a call's audio packet didn't decode");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(encoder: &mut opus::Encoder, frame: usize) -> Vec<u8> {
        let pcm: Vec<f32> = (0..FRAME)
            .map(|at| {
                ((frame * FRAME + at) as f32 * 330.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5
            })
            .collect();
        let mut packet = vec![0; 1500];
        let length = encoder.encode_float(&pcm, &mut packet).unwrap();
        packet.truncate(length);
        packet
    }

    #[test]
    fn a_speaker_waits_reorders_conceals_and_stops() {
        let mut encoder = opus::Encoder::new(1, opus::Application::Voice, BITRATE).unwrap();
        let mut speaker = Speaker::new(7).unwrap();
        let packets: Vec<Vec<u8>> = (0..8).map(|frame| tone(&mut encoder, frame)).collect();
        // Two packets aren't enough to start.
        speaker.push(65_534, packets[0].clone());
        speaker.push(65_535, packets[1].clone());
        assert!(speaker.next().is_none(), "waits for 60 ms");
        // The fourth arrives before the third, across the wrap.
        speaker.push(1, packets[3].clone());
        speaker.push(0, packets[2].clone());
        for _ in 0..4 {
            assert_eq!(speaker.next().map(<[i16]>::len), Some(FRAME * 2));
        }
        assert!(speaker.waiting.is_empty());
        // A packet that comes too late is dropped, not played.
        speaker.push(65_535, packets[1].clone());
        assert!(speaker.waiting.is_empty());
        // Packet 2 is lost: it's concealed from packet 3's error correction.
        speaker.push(3, packets[5].clone());
        assert!(speaker.next().is_some(), "packet 2, rebuilt");
        assert!(speaker.next().is_some(), "packet 3");
        // Then silence: concealed briefly, then the speaker stops.
        let played = std::iter::from_fn(|| speaker.next().map(|_| ())).count();
        assert_eq!(played, usize::from(GONE) - 1);
        assert!(speaker.next.is_none());
    }

    #[test]
    fn learns_whose_audio_from_speaking_and_video_messages() {
        let (frames, _) = channel::sync_channel(1);
        let mut media = Media::new(frames, Arc::default()).unwrap();
        media.learn(5, &json!({"user_id": "7", "ssrc": 100, "speaking": 1}));
        // Already talking when Talk joined: only op 12 names their audio.
        media.learn(
            12,
            &json!({"user_id": "8", "audio_ssrc": 200, "video_ssrc": 0, "streams": []}),
        );
        media.learn(
            12,
            &json!({"user_id": "9", "audio_ssrc": 0, "video_ssrc": 0}),
        );
        assert_eq!(media.users, HashMap::from([(100, 7), (200, 8)]));
    }

    #[test]
    fn a_long_stall_keeps_only_the_newest() {
        let mut encoder = opus::Encoder::new(1, opus::Application::Voice, BITRATE).unwrap();
        let mut speaker = Speaker::new(7).unwrap();
        let packet = tone(&mut encoder, 0);
        for sequence in 0..(MOST as u16 + 10) {
            speaker.push(sequence, packet.clone());
        }
        assert_eq!(speaker.waiting.len(), MOST);
        assert_eq!(
            speaker.waiting.front().map(|(sequence, _)| *sequence),
            Some(10)
        );
    }
}
