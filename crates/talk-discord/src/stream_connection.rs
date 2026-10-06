//! A Go Live stream's media connection, to the server Discord assigned the
//! stream: sending this account's stream, or receiving someone else's. It's
//! a voice connection of its own: the voice gateway, version 4 like
//! songbird's calls, identifying with the stream's RTC server and video;
//! UDP with Discord's transport encryption; and DAVE for the frames. Talk
//! sends VP8 and receives VP8 or H264.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc as channel,
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use songbird::model::{Event, serialize_binary_event};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::{mpsc, oneshot},
    time::{Instant, timeout},
};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

use crate::{
    dave::{Dave, Out},
    rtp::{self, Assembler, Codec, Packetizer, Transport},
    stream::{self, StreamServer},
};

/// A voice gateway version Discord's clients use, as Discord-video-stream
/// does; songbird's version 4 is enough for calls.
const VERSION: u8 = 8;
const HANDSHAKE: Duration = Duration::from_secs(15);
/// How often a viewer may ask for a keyframe while frames are lost.
const KEYFRAME_REQUESTS: Duration = Duration::from_millis(500);
/// How often a streamer sends an RTCP sender report.
const SENDER_REPORTS: Duration = Duration::from_secs(1);
/// How often a viewer logs what it received while no picture comes through.
const LOG_EVERY: Duration = Duration::from_secs(5);
/// The most packets resent for one NACK, which can name hundreds.
const RESENDS: usize = 128;
/// A frame's packets past the first `BURST` go out `PACE` a millisecond,
/// about 37 Mbit/s: a 1080p keyframe sent at once can overflow buffers on
/// the way.
const BURST: usize = 8;
const PACE: usize = 4;

/// The most bit/s a stream of `height` lines may take, as Discord's client
/// decides it: 3.5 Mbit/s up to 720p, 9 Mbit/s above. Op 12 announces it.
pub fn max_bitrate(height: u32) -> u32 {
    if height > 720 { 9_000_000 } else { 3_500_000 }
}

/// Op 5's speaking flags for a stream's sound: shared sound, not a
/// microphone, as Discord's client marks it.
const SOUNDSHARE: u8 = 2;

/// The codec Talk streams in, as every other Go Live sender known to work
/// does.
pub const SEND_CODEC: Codec = Codec::H264;

/// One encoded frame to send in [`SEND_CODEC`], with its time in the 90 kHz
/// clock.
pub struct Frame {
    pub data: Vec<u8>,
    pub timestamp: u32,
    pub keyframe: bool,
}

/// One Opus frame of the stream's sound: 20 ms of stereo at 48 kHz, with
/// `timestamp` in samples.
pub struct Sound {
    pub opus: Vec<u8>,
    pub timestamp: u32,
}

/// Queues the stream's sound, from a thread of the sender's own.
#[derive(Clone)]
pub struct SoundSender(mpsc::Sender<Sound>);

impl SoundSender {
    /// Queues a frame of sound, dropping it if the network can't keep up.
    pub fn send(&self, sound: Sound) {
        let _ = self.0.try_send(sound);
    }
}

/// One whole frame received, decrypted and ready to decode.
pub struct ReceivedFrame {
    pub codec: Codec,
    pub data: Vec<u8>,
    pub keyframe: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Connected,
    Ended(String),
}

struct Shared {
    link: Mutex<Link>,
    /// Sending: the next frame should be a keyframe. Receiving: ask the
    /// streamer for one.
    keyframe: AtomicBool,
    size: Mutex<(u32, u32)>,
    /// Sending: the bit/s Discord's server last estimated the path carries,
    /// or 0 before it said.
    estimate: AtomicU64,
}

impl Shared {
    fn new(size: (u32, u32)) -> Arc<Self> {
        Arc::new(Self {
            link: Mutex::new(Link::Connecting),
            keyframe: AtomicBool::new(true),
            size: Mutex::new(size),
            estimate: AtomicU64::new(0),
        })
    }
}

/// Runs a connection on a thread of its own until it ends, recording why.
fn spawn(
    name: &str,
    shared: Arc<Shared>,
    run: impl FnOnce(Arc<Shared>) -> std::pin::Pin<Box<dyn Future<Output = Result<()>>>>
    + Send
    + 'static,
) -> Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let result = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(anyhow::Error::from)
                .and_then(|runtime| runtime.block_on(run(shared.clone())));
            let reason = match result {
                Ok(()) => "stopped".to_owned(),
                Err(error) => {
                    tracing::warn!(error = format!("{error:#}"), "the stream connection ended");
                    format!("{error:#}")
                }
            };
            *lock(&shared.link) = Link::Ended(reason);
        })
        .context("could not start the stream connection")?;
    Ok(())
}

/// The user and channel ids a stream key names, as numbers.
fn ids(server: &StreamServer) -> Result<(u64, u64, u64)> {
    let (_, channel, streamer) =
        stream::parse(&server.stream_key).context("not a server stream")?;
    Ok((
        server.user_id.parse().context("not a user id")?,
        channel.parse().context("not a channel id")?,
        streamer.parse().context("not a user id")?,
    ))
}

/// Sends frames on a thread of its own; dropping it ends the connection.
pub struct StreamSender {
    frames: mpsc::Sender<Frame>,
    sounds: mpsc::Sender<Sound>,
    shared: Arc<Shared>,
}

impl StreamSender {
    /// Connects to `server` and announces `width` by `height` video.
    pub fn start(server: StreamServer, width: u32, height: u32, fps: u32) -> Result<Self> {
        let (user, channel, _) = ids(&server)?;
        let shared = Shared::new((width, height));
        // A few frames of slack; beyond that the network can't keep up.
        let (frames, receiver) = mpsc::channel(4);
        // 160 ms of sound.
        let (sounds, heard) = mpsc::channel(8);
        spawn("talk-stream-send", shared.clone(), move |shared| {
            Box::pin(
                async move { send(&server, user, channel, fps, &shared, receiver, heard).await },
            )
        })?;
        Ok(Self {
            frames,
            sounds,
            shared,
        })
    }

    /// Where the stream's sound goes.
    pub fn sound(&self) -> SoundSender {
        SoundSender(self.sounds.clone())
    }

    /// Queues a frame. A full queue drops it and asks for a keyframe, so
    /// viewers recover once the network catches up.
    pub fn send(&self, frame: Frame) {
        if self.frames.try_send(frame).is_err() {
            self.shared.keyframe.store(true, Ordering::Relaxed);
        }
    }

    /// Announces a new frame size with the next frame.
    pub fn resize(&self, width: u32, height: u32) {
        *lock(&self.shared.size) = (width, height);
    }

    /// Whether the next frame should be a keyframe: at the start, after
    /// dropped frames, or when a viewer asked for one.
    pub fn take_keyframe_request(&self) -> bool {
        self.shared.keyframe.swap(false, Ordering::Relaxed)
    }

    /// The bit/s Discord's server last estimated the path carries, once it
    /// said.
    pub fn bandwidth_estimate(&self) -> Option<u64> {
        Some(self.shared.estimate.load(Ordering::Relaxed)).filter(|&estimate| estimate > 0)
    }

    pub fn link(&self) -> Link {
        lock(&self.shared.link).clone()
    }
}

/// Receives someone's stream on a thread of its own; dropping it ends the
/// connection.
pub struct StreamReceiver {
    frames: channel::Receiver<ReceivedFrame>,
    sounds: Option<channel::Receiver<Vec<u8>>>,
    shared: Arc<Shared>,
    _stop: oneshot::Sender<()>,
}

impl StreamReceiver {
    pub fn start(server: StreamServer) -> Result<Self> {
        let (user, channel, streamer) = ids(&server)?;
        let shared = Shared::new((0, 0));
        // Frames waiting to be decoded; more means decoding can't keep up.
        let (frames, receiver) = channel::sync_channel(4);
        // Opus frames waiting to be played: 320 ms.
        let (sounds, heard) = channel::sync_channel(16);
        let (stop, stopped) = oneshot::channel();
        spawn("talk-stream-receive", shared.clone(), move |shared| {
            Box::pin(async move {
                receive_stream(
                    &server,
                    user,
                    channel,
                    streamer,
                    &shared,
                    (frames, sounds),
                    stopped,
                )
                .await
            })
        })?;
        Ok(Self {
            frames: receiver,
            sounds: Some(heard),
            shared,
            _stop: stop,
        })
    }

    /// The next whole frame, waiting at most `wait`.
    pub fn next(&self, wait: Duration) -> Option<ReceivedFrame> {
        self.frames.recv_timeout(wait).ok()
    }

    /// The stream's sound, decrypted Opus frames, for a player of its own.
    /// It ends with the connection.
    pub fn take_sounds(&mut self) -> Option<channel::Receiver<Vec<u8>>> {
        self.sounds.take()
    }

    /// Asks the streamer for a keyframe, as after a frame that wouldn't decode.
    pub fn request_keyframe(&self) {
        self.shared.keyframe.store(true, Ordering::Relaxed);
    }

    pub fn link(&self) -> Link {
        lock(&self.shared.link).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The voice gateway's address. A test server on this machine may give a
/// plain WebSocket address, as songbird accepts for calls.
fn gateway_url(endpoint: &str) -> Result<reqwest::Url> {
    if let Some(local) = endpoint.strip_prefix("ws://") {
        let url = reqwest::Url::parse(&format!("ws://{local}/?v={VERSION}"))?;
        return match url.host() {
            Some(url::Host::Ipv4(ip)) if ip.is_loopback() => Ok(url),
            Some(url::Host::Domain("localhost")) => Ok(url),
            _ => bail!("Discord gave a stream server Talk won't connect to"),
        };
    }
    Ok(reqwest::Url::parse(&format!(
        "wss://{endpoint}/?v={VERSION}"
    ))?)
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn send_json(ws: &mut Socket, op: u8, data: Value) -> Result<()> {
    ws.send(Message::text(json!({"op": op, "d": data}).to_string()))
        .await?;
    Ok(())
}

async fn send_out(ws: &mut Socket, out: Vec<Out>) -> Result<()> {
    for out in out {
        let message = match out {
            Out::Json(event) => Message::text(serde_json::to_string(&event)?),
            Out::Binary(event) => {
                Message::binary(serialize_binary_event(&event).map_err(|error| anyhow!("{error}"))?)
            }
        };
        ws.send(message).await?;
    }
    Ok(())
}

/// A message from the voice gateway: its op and data, or a DAVE event.
enum Incoming {
    Json(u64, Value),
    Dave(Event),
}

/// The next message, noting its sequence number for heartbeats to
/// acknowledge: version 8 numbers JSON messages in `seq`, and binary ones in
/// their first two bytes.
async fn receive(ws: &mut Socket, seq: &mut Option<u64>) -> Result<Incoming> {
    loop {
        match ws
            .next()
            .await
            .context("Discord closed the stream connection")??
        {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(&text)?;
                if let Some(number) = value["seq"].as_u64() {
                    *seq = Some(number);
                }
                let op = value["op"].as_u64().unwrap_or(u64::MAX);
                // The voice model knows DAVE's JSON ops and who's connected.
                if matches!(op, 11 | 13 | 21 | 22 | 24)
                    && let Ok(event) = serde_json::from_value::<Event>(value.clone())
                {
                    return Ok(Incoming::Dave(event));
                }
                return Ok(Incoming::Json(op, value["d"].clone()));
            }
            Message::Binary(bytes) => match binary(&bytes) {
                Ok((number, event)) => {
                    *seq = Some(number);
                    return Ok(Incoming::Dave(event));
                }
                Err(error) => tracing::debug!(%error, "unknown binary voice message"),
            },
            Message::Close(frame) => {
                bail!(
                    "Discord closed the stream connection{}",
                    frame
                        .map(|frame| format!(" ({}: {})", frame.code, frame.reason))
                        .unwrap_or_default()
                )
            }
            _ => {}
        }
    }
}

/// A binary DAVE message and its sequence number. Version 8 puts the number
/// before the opcode; the voice model reads from the opcode on, as version 4
/// sends it.
fn binary(bytes: &[u8]) -> Result<(u64, Event)> {
    let Some((number, message)) = bytes.split_first_chunk::<2>() else {
        bail!("a binary voice message too short for its sequence number");
    };
    let event =
        songbird::model::deserialize_binary_event(message).map_err(|error| anyhow!("{error}"))?;
    Ok((u64::from(u16::from_be_bytes(*number)), event))
}

/// Discord's address discovery: the server tells the client its public
/// address and port, which it then selects the protocol with.
async fn discover(udp: &UdpSocket, ssrc: u32) -> Result<(IpAddr, u16)> {
    let mut request = [0u8; 74];
    request[..2].copy_from_slice(&1u16.to_be_bytes());
    request[2..4].copy_from_slice(&70u16.to_be_bytes());
    request[4..8].copy_from_slice(&ssrc.to_be_bytes());
    udp.send(&request).await?;
    let mut answer = [0u8; 74];
    let length = timeout(Duration::from_secs(5), udp.recv(&mut answer))
        .await
        .context("the stream server didn't answer address discovery")??;
    if length < 74 || u16::from_be_bytes([answer[0], answer[1]]) != 2 {
        bail!("the stream server's address discovery answer was malformed");
    }
    let address = &answer[8..72];
    let end = address
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(address.len());
    let ip = std::str::from_utf8(&address[..end])?.parse()?;
    Ok((ip, u16::from_be_bytes([answer[72], answer[73]])))
}

fn video_attributes(
    ssrc: u32,
    video: u32,
    rtx: u32,
    (width, height): (u32, u32),
    fps: u32,
) -> Value {
    json!({
        "audio_ssrc": ssrc,
        "video_ssrc": video,
        "rtx_ssrc": rtx,
        "streams": [{
            "type": "video",
            "rid": rtp::STREAM_RID,
            "ssrc": video,
            "active": true,
            "quality": 100,
            "rtx_ssrc": rtx,
            "max_bitrate": max_bitrate(height),
            "max_framerate": fps,
            "max_resolution": {"type": "fixed", "width": width, "height": height},
        }],
    })
}

/// Which way the video goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Send,
    Receive,
}

/// A connection after its handshake.
struct Connection {
    ws: Socket,
    udp: UdpSocket,
    transport: Transport,
    dave: Dave,
    ssrc: u32,
    video: u32,
    rtx: u32,
    interval: Duration,
    /// The video codec Discord chose, if it said.
    codec: Option<Codec>,
    /// Gateway messages that arrived during the handshake.
    early: Vec<(u64, Value)>,
    /// The last sequence number the server sent, for heartbeats.
    seq: Option<u64>,
}

impl Connection {
    async fn open(server: &StreamServer, user: u64, channel: u64, role: Role) -> Result<Self> {
        let url = gateway_url(&server.endpoint)?;
        let (mut ws, _) = timeout(HANDSHAKE, tokio_tungstenite::connect_async(url.as_str()))
            .await
            .context("the stream server didn't answer")??;
        send_json(
            &mut ws,
            0,
            json!({
                "server_id": server.rtc_server_id,
                "user_id": server.user_id,
                "session_id": server.session_id,
                "token": server.token,
                "video": true,
                "streams": [{"type": "screen", "rid": rtp::STREAM_RID, "quality": 100}],
                "max_dave_protocol_version": davey::DAVE_PROTOCOL_VERSION,
            }),
        )
        .await?;
        // Talk encodes in SEND_CODEC, and decodes VP8 or H264.
        let (payload_type, rtx_payload_type) = SEND_CODEC.payload_types();
        let codecs = match role {
            Role::Send => json!([
                {"name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::OPUS},
                {"name": SEND_CODEC.name(), "type": "video", "priority": 1000,
                 "payload_type": payload_type, "rtx_payload_type": rtx_payload_type,
                 "encode": true, "decode": true},
            ]),
            Role::Receive => json!([
                {"name": "opus", "type": "audio", "priority": 1000, "payload_type": rtp::OPUS},
                {"name": "H264", "type": "video", "priority": 1000, "payload_type": rtp::H264,
                 "rtx_payload_type": rtp::H264_RTX, "encode": false, "decode": true},
                {"name": "VP8", "type": "video", "priority": 2000, "payload_type": rtp::VP8,
                 "rtx_payload_type": rtp::VP8_RTX, "encode": false, "decode": true},
            ]),
        };

        let mut dave_early = vec![];
        let mut early = vec![];
        let mut seq = None;
        let handshake = async {
            let (mut interval, mut ready) = (None, None);
            while interval.is_none() || ready.is_none() {
                match receive(&mut ws, &mut seq).await? {
                    Incoming::Json(8, hello) => interval = hello["heartbeat_interval"].as_f64(),
                    Incoming::Json(2, data) => ready = Some(data),
                    Incoming::Json(op, data) => early.push((op, data)),
                    Incoming::Dave(event) => dave_early.push(event),
                }
            }
            let (interval, ready) = (interval.unwrap_or(41_250.0), ready.unwrap_or_default());
            tracing::debug!(%ready, "the stream server is ready");
            let ssrc = ready["ssrc"]
                .as_u64()
                .context("the stream server sent no SSRC")? as u32;
            let ip: IpAddr = ready["ip"]
                .as_str()
                .context("the stream server sent no address")?
                .parse()?;
            let port = ready["port"]
                .as_u64()
                .context("the stream server sent no port")? as u16;
            let modes: Vec<&str> = ready["modes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let mode = Transport::MODES
                .into_iter()
                .find(|mode| modes.contains(mode))
                .context("the stream server offers no encryption Talk knows")?;
            // Discord numbers the video SSRCs after the audio one.
            let stream = &ready["streams"][0];
            let video = stream["ssrc"]
                .as_u64()
                .map_or(ssrc.wrapping_add(1), |ssrc| ssrc as u32);
            let rtx = stream["rtx_ssrc"]
                .as_u64()
                .map_or(ssrc.wrapping_add(2), |ssrc| ssrc as u32);

            let udp = UdpSocket::bind(if ip.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).await?;
            udp.connect((ip, port)).await?;
            let (address, port) = discover(&udp, ssrc).await?;
            send_json(
                &mut ws,
                1,
                json!({
                    "protocol": "udp",
                    "codecs": codecs,
                    "data": {"address": address.to_string(), "port": port, "mode": mode},
                }),
            )
            .await?;
            let session = loop {
                match receive(&mut ws, &mut seq).await? {
                    Incoming::Json(4, session) => break session,
                    Incoming::Json(op, data) => early.push((op, data)),
                    Incoming::Dave(event) => dave_early.push(event),
                }
            };
            let mut described = session.clone();
            if let Some(fields) = described.as_object_mut() {
                fields.remove("secret_key");
            }
            tracing::debug!(session = %described, "the stream server described the session");
            let key: Vec<u8> = session["secret_key"]
                .as_array()
                .context("the stream server sent no key")?
                .iter()
                .filter_map(|byte| byte.as_u64().map(|byte| byte as u8))
                .collect();
            let transport = Transport::new(session["mode"].as_str().unwrap_or(mode), &key)?;
            let version = session["dave_protocol_version"].as_u64().unwrap_or(0) as u16;
            let codec = session["video_codec"].as_str().and_then(Codec::from_name);
            anyhow::Ok((interval, ssrc, video, rtx, udp, transport, version, codec))
        };
        let (interval, ssrc, video, rtx, udp, transport, version, codec) =
            timeout(HANDSHAKE, handshake)
                .await
                .context("the stream server took too long to connect")??;

        // Discord made a stream's DAVE group id just before its RTC server
        // id, in the one stream seen so far; Dave moves to the id the voice
        // server uses if this guess is wrong.
        let group = server
            .rtc_server_id
            .parse::<u64>()
            .map_or(channel, |id| id.saturating_sub(1));
        let mut dave = Dave::new(user, group);
        if let Some(streamer) =
            stream::parse(&server.stream_key).and_then(|(_, _, streamer)| streamer.parse().ok())
        {
            dave.recognise(streamer);
        }
        for (op, data) in &early {
            let mut text = data.to_string();
            text.truncate(400);
            tracing::debug!(op, data = %text, "the stream server said while connecting");
            for user in announced(*op, data) {
                dave.recognise(user);
            }
        }
        send_out(&mut ws, dave.start(version)).await?;
        for event in dave_early {
            send_out(&mut ws, dave.handle(event)).await?;
        }
        tracing::info!(
            dave = version,
            ?codec,
            rtc_server = %server.rtc_server_id,
            channel,
            "connected to a Discord stream"
        );
        Ok(Self {
            ws,
            udp,
            transport,
            dave,
            ssrc,
            video,
            rtx,
            interval: Duration::from_secs_f64(interval / 1000.0),
            codec,
            early,
            seq,
        })
    }

    async fn heartbeat(&mut self) -> Result<()> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as u64);
        send_json(
            &mut self.ws,
            3,
            json!({"t": nonce, "seq_ack": self.seq.map_or(-1, |seq| seq as i64)}),
        )
        .await
    }

    /// Answers DAVE; other messages go back to the caller.
    async fn handle(&mut self, message: Incoming) -> Result<Option<(u64, Value)>> {
        match message {
            Incoming::Dave(event) => {
                let out = self.dave.handle(event);
                send_out(&mut self.ws, out).await?;
                Ok(None)
            }
            Incoming::Json(op, data) => {
                for user in announced(op, &data) {
                    self.dave.recognise(user);
                }
                Ok(Some((op, data)))
            }
        }
    }
}

/// Who a voice gateway message says is in the stream, for DAVE's check of
/// the people proposed into its group: op 11 lists them, while speaking
/// (5) and video attributes (12) carry one.
fn announced(op: u64, data: &Value) -> Vec<u64> {
    let id = |value: &Value| value.as_str().and_then(|id| id.parse().ok());
    match op {
        11 => data["user_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(id)
            .collect(),
        5 | 12 => id(&data["user_id"]).into_iter().collect(),
        _ => vec![],
    }
}

async fn send(
    server: &StreamServer,
    user: u64,
    channel: u64,
    fps: u32,
    shared: &Shared,
    mut frames: mpsc::Receiver<Frame>,
    mut sounds: mpsc::Receiver<Sound>,
) -> Result<()> {
    let mut connection = Connection::open(server, user, channel, Role::Send).await?;
    let (ssrc, video, rtx) = (connection.ssrc, connection.video, connection.rtx);
    let mut size = *lock(&shared.size);
    send_json(
        &mut connection.ws,
        12,
        video_attributes(ssrc, video, rtx, size, fps),
    )
    .await?;
    send_json(
        &mut connection.ws,
        5,
        json!({"speaking": SOUNDSHARE, "delay": 0, "ssrc": ssrc}),
    )
    .await?;
    *lock(&shared.link) = Link::Connected;

    if let Some(codec) = connection.codec
        && codec != SEND_CODEC
    {
        bail!("Discord asked for {codec:?} video, and Talk only sends {SEND_CODEC:?}");
    }
    let mut packetizer = Packetizer::new(video, SEND_CODEC);
    let mut history = rtp::History::default();
    let interval = connection.interval;
    let mut heartbeat = tokio::time::interval_at(Instant::now() + interval, interval);
    let mut datagram = vec![0u8; 1500];
    // For RTCP sender reports: packets and payload bytes sent so far.
    let (mut packets, mut octets) = (0u32, 0u32);
    let mut reported = Instant::now() - SENDER_REPORTS;
    // For the log: whether frames went out encrypted, and what came back.
    let mut encrypted = None;
    let mut sent = Sent::default();
    let mut log = tokio::time::interval_at(Instant::now() + LOG_EVERY, LOG_EVERY);
    // The sound's RTP numbering and sender report counts, on the audio SSRC.
    let mut sound_sequence: u16 = rand::random();
    let (mut sound_packets, mut sound_octets) = (0u32, 0u32);
    let mut sound_reported = Instant::now() - SENDER_REPORTS;
    loop {
        tokio::select! {
            frame = frames.recv() => {
                let Some(frame) = frame else {
                    let _ = connection.ws.close(None).await;
                    return Ok(());
                };
                let wanted = *lock(&shared.size);
                if wanted != size {
                    size = wanted;
                    send_json(&mut connection.ws, 12, video_attributes(ssrc, video, rtx, size, fps)).await?;
                }
                let encrypting = connection.dave.encrypting();
                if encrypted != Some(encrypting) {
                    encrypted = Some(encrypting);
                    tracing::info!(
                        end_to_end = encrypting,
                        dave = connection.dave.status(),
                        "sending stream frames"
                    );
                }
                let payload = connection.dave.encrypt(SEND_CODEC, &frame.data);
                sent.frames += 1;
                sent.keyframes += u32::from(frame.keyframe);
                for (index, mut packet) in packetizer.packets(&payload, frame.timestamp).into_iter().enumerate() {
                    if index >= BURST && index % PACE == 0 {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    history.remember(&packet);
                    let clear = rtp::rtp_clear_len(&packet).unwrap_or(packet.len());
                    packets = packets.wrapping_add(1);
                    octets = octets.wrapping_add((packet.len() - clear) as u32);
                    connection.transport.seal(&mut packet, clear)?;
                    connection.udp.send(&packet).await?;
                }
                if reported.elapsed() >= SENDER_REPORTS {
                    reported = Instant::now();
                    let mut report = rtp::sender_report(
                        video,
                        std::time::SystemTime::now(),
                        frame.timestamp,
                        packets,
                        octets,
                    );
                    connection.transport.seal(&mut report, rtp::RTCP_CLEAR)?;
                    connection.udp.send(&report).await?;
                }
            }
            _ = heartbeat.tick() => connection.heartbeat().await?,
            message = receive(&mut connection.ws, &mut connection.seq) => {
                // Not heartbeat acks, nor the keys in the ready message and
                // session description.
                if let Some((op, data)) = connection.handle(message?).await?
                    && !matches!(op, 2 | 4 | 6)
                {
                    let mut text = data.to_string();
                    text.truncate(400);
                    tracing::debug!(op, data = %text, "the stream server said");
                    // Tell whoever connected about the video again, in case
                    // the server doesn't pass it on to late viewers.
                    if op == 11 {
                        send_json(&mut connection.ws, 12, video_attributes(ssrc, video, rtx, size, fps)).await?;
                        send_json(&mut connection.ws, 5, json!({"speaking": SOUNDSHARE, "delay": 0, "ssrc": ssrc})).await?;
                    }
                }
            }
            Some(sound) = sounds.recv() => {
                let payload = connection.dave.encrypt_opus(&sound.opus);
                let mut packet =
                    rtp::opus_packet(ssrc, sound_sequence, sound.timestamp, sound_packets == 0, &payload);
                sound_sequence = sound_sequence.wrapping_add(1);
                sound_packets = sound_packets.wrapping_add(1);
                sound_octets = sound_octets.wrapping_add(payload.len() as u32);
                let clear = rtp::rtp_clear_len(&packet).unwrap_or(packet.len());
                connection.transport.seal(&mut packet, clear)?;
                connection.udp.send(&packet).await?;
                sent.sounds += 1;
                if sound_reported.elapsed() >= SENDER_REPORTS {
                    sound_reported = Instant::now();
                    let mut report = rtp::sender_report(
                        ssrc,
                        std::time::SystemTime::now(),
                        sound.timestamp,
                        sound_packets,
                        sound_octets,
                    );
                    connection.transport.seal(&mut report, rtp::RTCP_CLEAR)?;
                    connection.udp.send(&report).await?;
                }
            }
            received = connection.udp.recv(&mut datagram) => {
                let packet = &datagram[..received?];
                if !rtp::is_rtcp(packet) {
                    sent.rtp_back += 1;
                    continue;
                }
                let Some(rtcp) = connection.transport.open(packet, rtp::RTCP_CLEAR) else {
                    sent.unopened += 1;
                    continue;
                };
                for (kind, said) in rtp::rtcp_kinds(&rtcp) {
                    *sent.rtcp.entry(kind.clone()).or_default() += 1;
                    if !said.is_empty() {
                        sent.said.insert(kind, said);
                    }
                }
                if rtp::wants_keyframe(&rtcp) {
                    sent.requests += 1;
                    shared.keyframe.store(true, Ordering::Relaxed);
                }
                if let Some(estimate) = rtp::bandwidth_estimate(&rtcp, video) {
                    shared.estimate.store(estimate, Ordering::Relaxed);
                }
                for lost in rtp::nacked(&rtcp, video).into_iter().take(RESENDS) {
                    let Some(mut again) = history.get(lost).and_then(|original| packetizer.retransmit(original, rtx)) else {
                        sent.too_late += 1;
                        continue;
                    };
                    let clear = rtp::rtp_clear_len(&again).unwrap_or(again.len());
                    connection.transport.seal(&mut again, clear)?;
                    connection.udp.send(&again).await?;
                    sent.resent += 1;
                }
            }
            _ = log.tick() => {
                tracing::debug!(?sent, packets, octets, "stream sent");
                sent = Sent::default();
            }
        }
    }
}

/// What a streamer sent and heard back since it last logged.
#[derive(Debug, Default)]
struct Sent {
    frames: u32,
    keyframes: u32,
    /// Opus frames of the stream's sound.
    sounds: u32,
    /// RTCP by packet type and feedback format, as `type/format`.
    rtcp: std::collections::BTreeMap<String, u32>,
    /// What the last of each kind said.
    said: std::collections::BTreeMap<String, String>,
    /// Keyframe requests among them.
    requests: u32,
    /// Packets resent when a viewer reported them lost, and those already
    /// gone from the history.
    resent: u32,
    too_late: u32,
    /// RTCP that wouldn't decrypt.
    unopened: u32,
    /// RTP, which a streamer isn't sent.
    rtp_back: u32,
}

/// What the server said about the streamer's video.
#[derive(Default)]
struct Streamer {
    video: Option<u32>,
    rtx: Option<u32>,
    codec: Option<Codec>,
}

impl Streamer {
    fn learn(&mut self, op: u64, data: &Value) {
        match op {
            // Video: the streamer's SSRCs.
            12 => {
                let ssrc = |value: &Value| {
                    value
                        .as_u64()
                        .filter(|&ssrc| ssrc != 0)
                        .map(|ssrc| ssrc as u32)
                };
                self.video = ssrc(&data["video_ssrc"]).or(self.video);
                self.rtx = ssrc(&data["rtx_ssrc"]).or(self.rtx);
            }
            // Session update: the codec may change.
            14 => {
                if let Some(codec) = data["video_codec"].as_str().and_then(Codec::from_name) {
                    self.codec = Some(codec);
                }
            }
            _ => {}
        }
    }

    /// The codec of a packet, by its payload type or, from the streamer's
    /// video SSRC, by what Discord chose.
    fn codec(&self, packet: &rtp::Packet) -> Option<Codec> {
        if Some(packet.ssrc) == self.rtx {
            return None;
        }
        Codec::from_payload_type(packet.payload_type).or_else(|| {
            (Some(packet.ssrc) == self.video)
                .then_some(self.codec)
                .flatten()
        })
    }
}

/// What a viewer's connection has received, logged while no picture
/// reaches the decoder so a stuck stream shows where it stops.
#[derive(Debug, Default)]
struct Received {
    packets: u64,
    rtcp: u64,
    unopened: u64,
    unparsed: u64,
    /// Packets of a payload type that isn't the streamer's video.
    other: u64,
    video: u64,
    frames: u64,
    unreadable: u64,
    decoded: u64,
    /// Opus frames of the stream's sound, and those that wouldn't decrypt.
    sounds: u64,
    unheard: u64,
    /// Packets on the RTX SSRC: resent video, and padding.
    resent: u64,
    padding: u64,
    /// Video packets that came out of order, those that came after they
    /// were asked for again, those that came too late or twice, and those
    /// never seen.
    reordered: u64,
    recovered: u64,
    late: u64,
    given_up: u64,
    /// Packets asked for again (NACK), and keyframes asked for (PLI).
    asked: u64,
    keyframe_requests: u64,
    /// Frames dropped because the decoder was behind.
    behind: u64,
    payload_types: std::collections::BTreeSet<u8>,
    ssrcs: std::collections::BTreeSet<u32>,
}

/// One SSRC's video on its way to frames.
struct Video {
    reorder: rtp::Reorder,
    assembler: Assembler,
    /// For receiver reports: the counts at the last one.
    reported: (u32, u32),
}

async fn receive_stream(
    server: &StreamServer,
    user: u64,
    channel: u64,
    streamer_id: u64,
    shared: &Shared,
    // Where whole video frames and the sound's Opus frames go.
    (frames, sounds): (
        channel::SyncSender<ReceivedFrame>,
        channel::SyncSender<Vec<u8>>,
    ),
    mut stopped: oneshot::Receiver<()>,
) -> Result<()> {
    let mut connection = Connection::open(server, user, channel, Role::Receive).await?;
    let mut streamer = Streamer {
        codec: connection.codec,
        ..Streamer::default()
    };
    for (op, data) in std::mem::take(&mut connection.early) {
        streamer.learn(op, &data);
    }
    // Media sink wants: the server forwards video at the quality a client
    // asks for, here everything at full quality.
    send_json(&mut connection.ws, 15, json!({"any": 100})).await?;
    *lock(&shared.link) = Link::Connected;

    let mut incoming: HashMap<u32, Video> = HashMap::new();
    let mut asked = Instant::now() - KEYFRAME_REQUESTS;
    let interval = connection.interval;
    let mut heartbeat = tokio::time::interval_at(Instant::now() + interval, interval);
    let mut keyframes = tokio::time::interval(KEYFRAME_REQUESTS);
    // Gives up on missing packets and asks for them again even while
    // nothing arrives.
    let mut repairs = tokio::time::interval(Duration::from_millis(20));
    let mut reports = tokio::time::interval(Duration::from_secs(1));
    // The streamer's last sender report, as its NTP time's middle and when
    // it came, which receiver reports echo for the round trip.
    let mut last_report: Option<(u32, std::time::Instant)> = None;
    let mut datagram = vec![0u8; 2048];
    let mut received = Received::default();
    let mut log = tokio::time::interval_at(Instant::now() + LOG_EVERY, LOG_EVERY);
    // Frames handed to the decoder when the log last looked.
    let mut flowing = 0;
    loop {
        let mut lost = false;
        tokio::select! {
            _ = &mut stopped => {
                let _ = connection.ws.close(None).await;
                return Ok(());
            }
            _ = heartbeat.tick() => connection.heartbeat().await?,
            message = receive(&mut connection.ws, &mut connection.seq) => {
                if let Some((op, data)) = connection.handle(message?).await? {
                    if !matches!(op, 2 | 4 | 6) {
                        let mut text = data.to_string();
                        text.truncate(400);
                        tracing::debug!(op, data = %text, "the stream server said");
                    }
                    streamer.learn(op, &data);
                }
            }
            _ = keyframes.tick() => {}
            _ = repairs.tick() => {}
            _ = reports.tick() => {
                let Some(media) = streamer.video else { continue };
                let Some(video) = incoming.get_mut(&media) else { continue };
                let Some((highest, lost)) = video.reorder.counts() else { continue };
                let (expected, missed) = (
                    highest.wrapping_sub(video.reported.0),
                    lost.saturating_sub(video.reported.1),
                );
                let fraction = (u64::from(missed) * 256 / u64::from(expected.max(1))).min(255) as u8;
                video.reported = (highest, lost);
                let (middle, delay) = last_report.map_or((0, 0), |(middle, heard)| {
                    (middle, (heard.elapsed().as_secs_f64() * 65536.0) as u32)
                });
                let mut report = rtp::receiver_report(connection.ssrc, media, (highest, lost, fraction), (middle, delay));
                connection.transport.seal(&mut report, rtp::RTCP_CLEAR)?;
                connection.udp.send(&report).await?;
            }
            _ = log.tick() => {
                tracing::debug!(?received, "stream received");
                if received.decoded == flowing {
                    tracing::info!(
                        ?received,
                        dave = connection.dave.status(),
                        unreadable = connection.dave.unreadable.as_deref(),
                        streamer_video = ?streamer.video,
                        streamer_codec = ?streamer.codec,
                        "no stream frames reached the decoder lately"
                    );
                } else if flowing == 0 {
                    tracing::info!(frames = received.decoded, "stream frames are reaching the decoder");
                }
                flowing = received.decoded;
            }
            length = connection.udp.recv(&mut datagram) => {
                let packet = &datagram[..length?];
                received.packets += 1;
                if rtp::is_rtcp(packet) {
                    received.rtcp += 1;
                    let rtcp = connection.transport.open(packet, rtp::RTCP_CLEAR).unwrap_or_default();
                    for (sender, middle) in rtp::sender_reports(&rtcp) {
                        if Some(sender) == streamer.video {
                            last_report = Some((middle, std::time::Instant::now()));
                        }
                    }
                    continue;
                }
                let Some(clear) = rtp::rtp_clear_len(packet) else {
                    received.unparsed += 1;
                    continue;
                };
                let Some(plain) = connection.transport.open(packet, clear) else {
                    received.unopened += 1;
                    continue;
                };
                let Some(packet) = rtp::parse(&plain) else {
                    received.unparsed += 1;
                    continue;
                };
                received.payload_types.insert(packet.payload_type);
                received.ssrcs.insert(packet.ssrc);
                if packet.payload_type == rtp::OPUS {
                    received.sounds += 1;
                    match connection.dave.decrypt_opus(streamer_id, packet.payload.to_vec()) {
                        // A full queue drops sound the player can't keep up with.
                        Some(sound) => drop(sounds.try_send(sound)),
                        None => received.unheard += 1,
                    }
                    continue;
                }
                let held = if streamer.rtx.is_some() && Some(packet.ssrc) == streamer.rtx {
                    received.resent += 1;
                    match streamer.video.and_then(|video| rtp::Held::unwrap_rtx(&packet, video)) {
                        Some(held) => held,
                        None => {
                            received.padding += 1;
                            continue;
                        }
                    }
                } else {
                    rtp::Held::new(&packet)
                };
                let Some(codec) = streamer.codec(&held.packet()) else {
                    received.other += 1;
                    continue;
                };
                received.video += 1;
                streamer.video.get_or_insert(held.ssrc);
                let video = incoming.entry(held.ssrc).or_insert_with(|| Video {
                    reorder: rtp::Reorder::default(),
                    assembler: Assembler::new(codec),
                    reported: (0, 0),
                });
                if video.assembler.codec() != codec {
                    video.assembler = Assembler::new(codec);
                }
                let resent = Some(packet.ssrc) == streamer.rtx;
                match video.reorder.insert(held, std::time::Instant::now()) {
                    rtp::Arrival::New if !resent => {}
                    rtp::Arrival::New | rtp::Arrival::Recovered if resent => received.recovered += 1,
                    rtp::Arrival::New | rtp::Arrival::Recovered => received.reordered += 1,
                    rtp::Arrival::Late => received.late += 1,
                }
            }
        }
        let now = std::time::Instant::now();
        for (&ssrc, video) in &mut incoming {
            let codec = video.assembler.codec();
            let (ready, skipped) = video.reorder.ready(now);
            received.given_up += skipped;
            for held in ready {
                let pushed = video.assembler.push(&held.packet());
                lost |= pushed.lost;
                let Some(frame) = pushed.frame else { continue };
                received.frames += 1;
                let Some(data) = connection.dave.decrypt(streamer_id, frame.data) else {
                    received.unreadable += 1;
                    continue;
                };
                received.decoded += 1;
                let frame = ReceivedFrame {
                    codec,
                    data,
                    keyframe: frame.keyframe,
                };
                if let Err(channel::TrySendError::Full(_)) = frames.try_send(frame) {
                    // The decoder is behind; the frames after this one
                    // need it, so wait for a keyframe.
                    received.behind += 1;
                    video.assembler.wait_for_keyframe();
                    lost = true;
                }
            }
            let missing = video.reorder.to_ask(now);
            if !missing.is_empty() {
                received.asked += missing.len() as u64;
                let mut request = rtp::nack(connection.ssrc, ssrc, &missing);
                connection.transport.seal(&mut request, rtp::RTCP_CLEAR)?;
                connection.udp.send(&request).await?;
            }
        }
        // Ask for a keyframe after a loss, or when the decoder asked.
        if (lost || shared.keyframe.load(Ordering::Relaxed))
            && asked.elapsed() >= KEYFRAME_REQUESTS
            && let Some(media) = streamer.video
        {
            shared.keyframe.store(false, Ordering::Relaxed);
            asked = Instant::now();
            received.keyframe_requests += 1;
            let mut request = rtp::picture_loss(connection.ssrc, media);
            connection.transport.seal(&mut request, rtp::RTCP_CLEAR)?;
            connection.udp.send(&request).await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_websockets_only_on_this_machine() {
        assert_eq!(
            gateway_url("c-ams.discord.media:443").unwrap().as_str(),
            "wss://c-ams.discord.media/?v=8"
        );
        assert_eq!(
            gateway_url("ws://127.0.0.1:9/voice").unwrap().as_str(),
            "ws://127.0.0.1:9/voice/?v=8"
        );
        assert!(gateway_url("ws://example.com/voice").is_err());
    }

    #[test]
    fn reads_binary_messages_after_their_sequence_number() {
        // Sequence 5, op 30 (a welcome) for transition 4.
        let (seq, event) = binary(&[0, 5, 30, 0, 4, 1, 2, 3]).unwrap();
        assert_eq!(seq, 5);
        assert!(
            matches!(&event, Event::DaveMlsWelcome(welcome) if welcome.transition_id == 4 && welcome.welcome == [1, 2, 3]),
            "{event:?}"
        );
        assert!(binary(&[0]).is_err());
    }

    #[test]
    fn learns_the_streamers_video() {
        let mut streamer = Streamer::default();
        streamer.learn(
            12,
            &json!({"user_id": "2", "audio_ssrc": 10, "video_ssrc": 11, "rtx_ssrc": 12}),
        );
        streamer.learn(14, &json!({"video_codec": "H264"}));
        let packet = |ssrc: u32, payload_type: u8| rtp::Packet {
            payload_type,
            marker: false,
            sequence: 0,
            timestamp: 0,
            ssrc,
            payload: &[],
        };
        assert_eq!(
            streamer.codec(&packet(11, rtp::VP8)),
            Some(Codec::Vp8),
            "by payload type"
        );
        assert_eq!(
            streamer.codec(&packet(11, 96)),
            Some(Codec::H264),
            "by Discord's choice"
        );
        assert_eq!(
            streamer.codec(&packet(12, rtp::VP8)),
            None,
            "retransmissions aren't frames"
        );
        assert_eq!(streamer.codec(&packet(30, 96)), None);
    }
}
