//! Camera video in Discord calls, over the call's own voice connection.
//! Yapfast encodes its camera in H264 on the camera's thread and hands each
//! frame to the call. Everyone else's camera arrives as whole frames, which
//! one thread decodes into a feed made at the size its tile shows. A camera
//! whose tile is out of sight isn't asked for, and its decoder goes.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        mpsc::{Receiver, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

use anyhow::Result;
use tokio::sync::Notify;
use yapfast_discord::{
    call_video::{CameraSender, VideoFrame},
    stream_connection::{Frame, SEND_CODEC},
    voice::Call,
};

use crate::{
    camera::{self, Camera},
    decode::Decoder,
    encode::{CLOCK, Effort, Encoder},
    picture::{Feed, I420},
};

/// The camera's bitrate, in kbit/s: about what Discord's clients send 720p
/// at.
const BITRATE: u32 = 1500;
/// How often the decoding thread lets go of decoders for tiles gone from
/// sight.
const PRUNE: Duration = Duration::from_secs(1);
/// Tiles at least this tall ask for a camera's full quality, and smaller
/// ones for half, as Discord's clients do for their small tiles.
const FULL_QUALITY_HEIGHT: u32 = 360;

/// This account's camera, filming into the call until dropped or stopped.
pub(crate) struct Filming {
    /// What the self-view shows.
    pub feed: Feed,
    camera: Option<Camera>,
    sender: Arc<CameraSender>,
    /// The capture clock's start, the same across cameras.
    started: Instant,
}

impl Filming {
    /// Sends a camera to the call, once [`Self::open`] has opened one.
    pub fn start(call: &mut Call) -> Self {
        Self {
            feed: Feed::new(),
            camera: None,
            sender: Arc::new(call.film(camera::WIDTH, camera::HEIGHT, camera::FPS)),
            started: Instant::now(),
        }
    }

    /// Opens the camera `id`, or the first, off the call's loop, once the
    /// one filming has closed: one camera at a time. It films into the call
    /// as soon as it opens, and resolves with its first picture, for
    /// [`Self::opened`].
    pub fn open(&mut self, id: Option<&str>) -> impl Future<Output = Result<Camera>> + use<> {
        Camera::start(self.camera.take(), id.map(str::to_owned), self.sink())
    }

    /// Keeps the camera [`Self::open`] opened.
    pub fn opened(&mut self, camera: Camera) {
        self.camera = Some(camera);
    }

    /// Closes the camera, and tells the call it stopped.
    pub fn stop(mut self, call: &mut Call) {
        self.camera = None;
        self.feed.release();
        call.stop_filming();
    }

    /// Where a camera's pictures go: mirrored into the feed, and encoded
    /// into the call.
    fn sink(&self) -> camera::Sink {
        let mut view = self.feed.mirrored_sink();
        let (sender, started) = (self.sender.clone(), self.started);
        let mut encoder: Option<Encoder> = None;
        Box::new(move |picture: &I420<'_>| {
            view(picture);
            // The encoder takes even sizes, and starts again at a new one.
            let size = (picture.width & !1, picture.height & !1);
            let fresh = encoder
                .as_ref()
                .is_none_or(|encoder| encoder.size() != size);
            if fresh {
                encoder = Encoder::new(
                    SEND_CODEC,
                    size.0,
                    size.1,
                    camera::FPS,
                    BITRATE,
                    Effort::Light,
                    false,
                )
                .inspect_err(|error| tracing::warn!(%error, "could not encode the camera"))
                .ok();
            }
            let Some(encoder) = encoder.as_mut() else {
                return;
            };
            let keyframe = sender.take_keyframe_request() || fresh;
            let pts = started.elapsed().as_micros() as u64 * CLOCK / 1_000_000;
            match encoder.encode_picture(picture, pts, keyframe) {
                Ok(Some(frame)) => sender.send(Frame {
                    data: frame.data.to_vec(),
                    codec: SEND_CODEC,
                    timestamp: pts as u32,
                    keyframe: frame.keyframe,
                }),
                Ok(None) => {}
                Err(error) => tracing::warn!(%error, "could not encode the camera"),
            }
        })
    }
}

/// Everyone else's cameras, by Discord user id: a feed for each who films,
/// and what's been asked of the call for them.
pub(crate) struct Watching {
    feeds: Arc<Mutex<HashMap<u64, Feed>>>,
    /// Told when any of their tiles comes into sight, goes, or changes size.
    pub shown: Arc<Notify>,
    asked: HashMap<u64, (u8, u32)>,
}

impl Watching {
    /// Decodes the call's camera frames on a thread of their own, until the
    /// call ends.
    pub fn start(frames: Option<Receiver<VideoFrame>>) -> Self {
        let feeds = Arc::new(Mutex::new(HashMap::new()));
        if let Some(frames) = frames {
            let decoding = feeds.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("yapfast-discord-video".into())
                .spawn(move || decode(&frames, &decoding))
            {
                tracing::warn!(%error, "could not start decoding cameras");
            }
        }
        Self {
            feeds,
            shown: Arc::new(Notify::new()),
            asked: HashMap::new(),
        }
    }

    /// Keeps a feed for each of `filming`, and lets go of the others'.
    pub fn film(&mut self, filming: &[u64]) {
        let mut feeds = lock(&self.feeds);
        feeds.retain(|user, feed| {
            let kept = filming.contains(user);
            if !kept {
                feed.release();
            }
            kept
        });
        for &user in filming {
            feeds
                .entry(user)
                .or_insert_with(|| Feed::watched(self.shown.clone()));
        }
    }

    /// The feed of `user`'s camera, while they film.
    pub fn feed(&self, user: u64) -> Option<Feed> {
        lock(&self.feeds).get(&user).cloned()
    }

    /// Asks the call for each camera at the size its tile shows, and for
    /// none out of sight, when that changed.
    pub fn ask(&mut self, call: &mut Call) {
        let wants: HashMap<u64, (u8, u32)> = lock(&self.feeds)
            .iter()
            .filter_map(|(&user, feed)| {
                let (width, height) = feed.shown_at()?;
                let quality = if height >= FULL_QUALITY_HEIGHT {
                    100
                } else {
                    50
                };
                Some((user, (quality, width * height)))
            })
            .collect();
        if wants != self.asked {
            self.asked = wants.clone();
            call.want_video(wants);
        }
    }
}

/// Decodes each camera's frames into its feed, at the size it shows. A
/// camera gets a decoder when its tile is in sight, from a keyframe, and
/// loses it when the tile goes.
fn decode(frames: &Receiver<VideoFrame>, feeds: &Mutex<HashMap<u64, Feed>>) {
    // Each camera's decoder, the feed it writes, and the sink writing it.
    type Sink = Box<dyn FnMut(&I420<'_>) + Send>;
    let mut decoders: HashMap<u64, (Decoder, Feed, Sink)> = HashMap::new();
    let mut pruned = Instant::now();
    loop {
        match frames.recv_timeout(PRUNE) {
            Ok(frame) => {
                let feed = lock(feeds)
                    .get(&frame.user)
                    .filter(|feed| feed.shown_at().is_some())
                    .cloned();
                let Some(feed) = feed else {
                    decoders.remove(&frame.user);
                    continue;
                };
                let current = decoders
                    .get(&frame.user)
                    .is_some_and(|(decoder, shown, _)| {
                        decoder.codec() == frame.codec && *shown == feed
                    });
                if !current {
                    decoders.remove(&frame.user);
                    // A decoder starts from a keyframe.
                    if !frame.keyframe {
                        continue;
                    }
                    // One thread each: a tile is small, and every thread
                    // keeps frames of its own.
                    match Decoder::with_threads(frame.codec, 1) {
                        Ok(decoder) => {
                            let sink = Box::new(feed.sink());
                            decoders.insert(frame.user, (decoder, feed, sink));
                        }
                        Err(error) => {
                            tracing::warn!(%error, "could not decode a camera");
                            continue;
                        }
                    }
                }
                let Some((decoder, _, sink)) = decoders.get_mut(&frame.user) else {
                    continue;
                };
                match decoder.decode(&frame.data) {
                    Ok(Some(picture)) => sink(&picture.i420()),
                    Ok(None) => {}
                    Err(error) => {
                        tracing::debug!(%error, "dropped a camera's decoder");
                        decoders.remove(&frame.user);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if pruned.elapsed() >= PRUNE {
            pruned = Instant::now();
            let feeds = lock(feeds);
            decoders.retain(|user, (_, shown, _)| {
                feeds
                    .get(user)
                    .is_some_and(|feed| feed == shown && feed.shown_at().is_some())
            });
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
