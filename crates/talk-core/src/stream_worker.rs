//! Go Live runs in a worker process, `talk --stream-worker`, so screen
//! capture, video codecs and stream connections take memory only while
//! sharing or watching: the process exits when that stops and the memory
//! goes with it. Each worker shares this account's screen or shows one
//! other person's stream.
//!
//! Talk starts the worker with one [`Job`] as a JSON line on its standard
//! input, and reads [`Report`]s back from its standard output. Once Discord
//! has started the stream, a [`Control::Send`] line hands the worker the
//! stream's server, and it sends the video there. A `stop` line, or closing
//! its input, ends the job. Frames go to the UI through [`crate::frames`].

use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc as channel,
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use talk_discord::{
    stream::StreamServer,
    stream_connection::{self, Link, StreamReceiver, StreamSender},
};
use tokio::sync::mpsc;

use livekit::webrtc::{
    native::yuv_helper,
    video_frame::{I420Buffer, VideoBuffer},
};

use crate::{
    decode::Decoder,
    encode::{self, Encoder},
    frames::{Frames, Writer},
    stream_audio,
};

/// The argument that starts Talk as a stream worker.
pub const ARGUMENT: &str = "--stream-worker";

/// The largest frame shared. Discord's own client trials 1080p at 30 fps
/// for accounts without Nitro, up to 5 Mbit/s (its "frontier tuning"
/// experiment); larger screens are scaled down to it.
pub const SHARE_WIDTH: u32 = 1920;
pub const SHARE_HEIGHT: u32 = 1080;
/// The largest preview of the shared screen the UI shows.
pub const PREVIEW_WIDTH: u32 = 1280;
pub const PREVIEW_HEIGHT: u32 = 720;
/// The largest picture shown of someone else's stream; larger ones are
/// scaled down to it.
pub const WATCH_WIDTH: u32 = 1920;
pub const WATCH_HEIGHT: u32 = 1080;
const FPS: u32 = 30;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Job {
    /// Captures `source`, writing frames that fit `width` by `height` to the
    /// frames file at `frames`.
    Share {
        source: Source,
        frames: PathBuf,
        width: u32,
        height: u32,
        fps: u32,
    },
    /// Receives and decodes the stream at `server`, writing pictures that
    /// fit `width` by `height` to the frames file at `frames`.
    Watch {
        server: StreamServer,
        frames: PathBuf,
        width: u32,
        height: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A screen or window the person picks in the system's dialog.
    Screen,
    /// Moving colour bars, for testing without a screen to capture.
    TestPattern,
}

/// What Talk tells a running job.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    /// Send the video to the server Discord started the stream on.
    Send(StreamServer),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Report {
    /// The worker opened the frames file.
    Ready,
    /// Frames are flowing, at this size after scaling.
    Sharing { width: u32, height: u32 },
    /// Sharing: the video reaches the stream server. Watching: the first
    /// picture is on screen.
    Streaming,
    /// The job ended: without an error when it was told to stop.
    Ended { error: Option<String> },
}

/// Runs this process as a stream worker until its job ends.
pub fn run() -> Result<()> {
    // The stream connection uses TLS; nothing else here has started it.
    let _ = crate::init_tls();
    let mut output = std::io::stdout();
    serve(Box::new(BufReader::new(std::io::stdin())), &mut |report| {
        // Talk is gone if this fails; the job ends when its input closes.
        let _ = writeln!(
            output,
            "{}",
            serde_json::to_string(&report).unwrap_or_default()
        );
        let _ = output.flush();
    });
    Ok(())
}

fn serve(mut input: Box<dyn BufRead + Send>, report: &mut dyn FnMut(Report)) {
    let mut line = String::new();
    let job = match input.read_line(&mut line) {
        Ok(_) => {
            serde_json::from_str::<Job>(&line).map_err(|error| anyhow!("unknown job: {error}"))
        }
        Err(error) => Err(error.into()),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let stopper = stop.clone();
    let (controls, received) = channel::channel();
    // Reading stdin blocks, so a thread waits for controls, the stop line or the end.
    std::thread::spawn(move || {
        for line in input.lines() {
            let Ok(line) = line else { break };
            if line.trim() == "stop" {
                break;
            }
            match serde_json::from_str::<Control>(&line) {
                Ok(control) => {
                    let _ = controls.send(control);
                }
                Err(error) => tracing::warn!(%error, "unknown stream worker control"),
            }
        }
        stopper.store(true, Ordering::Relaxed);
    });
    let result = job.and_then(|job| match job {
        Job::Share {
            source,
            frames,
            width,
            height,
            fps,
        } => {
            let fps = fps.clamp(1, 60);
            let sink = Sink {
                writer: Writer::open(&frames)?,
                controls: received,
                sound: source == Source::Screen,
                video: None,
                fps,
                started: Instant::now(),
                streaming: false,
            };
            report(Report::Ready);
            match source {
                Source::TestPattern => test_pattern(sink, width, height, &stop, report),
                Source::Screen => capture_screen(sink, width, height, &stop, report),
            }
        }
        Job::Watch {
            server,
            frames,
            width,
            height,
        } => {
            let writer = Writer::open(&frames)?;
            report(Report::Ready);
            watch(server, writer, (width, height), &stop, report)
        }
    });
    report(Report::Ended {
        error: result.err().map(|error| format!("{error:#}")),
    });
}

/// Where each frame goes: the UI's preview, then Discord once the stream
/// has a server.
struct Sink {
    writer: Writer,
    controls: channel::Receiver<Control>,
    /// Whether to share other applications' sound too: not for the test
    /// pattern.
    sound: bool,
    video: Option<Video>,
    fps: u32,
    started: Instant,
    streaming: bool,
}

struct Video {
    sender: StreamSender,
    encoder: Encoder,
    /// Other applications' sound, while sharing a screen.
    _sound: Option<stream_audio::Capture>,
}

/// The kbit/s to aim for, as Discord's client does: its cap for frames of
/// `height` lines, or what its server estimates the path carries when
/// that's less, but at least the 500 kbit/s it gives a screen.
fn kbps(height: u32, estimate: Option<u64>) -> u32 {
    let cap = stream_connection::max_bitrate(height) / 1000;
    estimate.map_or(cap, |estimate| {
        (estimate / 1000).clamp(500, u64::from(cap)) as u32
    })
}

impl Sink {
    /// Takes a frame at the size it's sent: the preview shows it, scaled
    /// down to fit, and Discord gets it once the stream has a server.
    fn frame(&mut self, picture: &mut I420Buffer) -> Result<()> {
        let (width, height) = (picture.width(), picture.height());
        let preview = fit(width, height, PREVIEW_WIDTH, PREVIEW_HEIGHT);
        let smaller =
            (preview != (width, height)).then(|| picture.scale(preview.0 as i32, preview.1 as i32));
        let shown = smaller.as_ref().unwrap_or(picture);
        self.writer
            .write(preview.0, preview.1, |pixels| to_rgba(shown, pixels));
        if let Ok(Control::Send(server)) = self.controls.try_recv() {
            let sender = StreamSender::start(server, width, height, self.fps)?;
            let sound = if self.sound {
                stream_audio::Capture::start(sender.sound())
                    .inspect_err(|error| {
                        tracing::warn!(error = format!("{error:#}"), "sharing without sound")
                    })
                    .ok()
            } else {
                None
            };
            let encoder = Encoder::new(
                stream_connection::SEND_CODEC,
                width,
                height,
                self.fps,
                kbps(height, None),
            )?;
            self.video = Some(Video {
                sender,
                encoder,
                _sound: sound,
            });
        }
        let Some(video) = self.video.as_mut() else {
            return Ok(());
        };
        if video.encoder.size() != (width, height) {
            video.encoder = Encoder::new(
                stream_connection::SEND_CODEC,
                width,
                height,
                self.fps,
                kbps(height, None),
            )?;
            video.sender.resize(width, height);
        }
        let wanted = kbps(height, video.sender.bandwidth_estimate());
        let aimed = video.encoder.bitrate();
        // Not on every wobble, which would churn the rate control.
        if wanted.abs_diff(aimed) > aimed / 10 {
            match video.encoder.set_bitrate(wanted) {
                Ok(()) => tracing::debug!(kbps = wanted, "following Discord's bandwidth estimate"),
                Err(error) => {
                    tracing::warn!(error = format!("{error:#}"), "could not change the bitrate")
                }
            }
        }
        let pts = self.started.elapsed().as_micros() as u64 * encode::CLOCK / 1_000_000;
        let keyframe = video.sender.take_keyframe_request();
        if let Some(frame) = video.encoder.encode_i420(picture, pts, keyframe)? {
            video.sender.send(stream_connection::Frame {
                data: frame.data.to_vec(),
                timestamp: pts as u32,
                keyframe: frame.keyframe,
            });
        }
        Ok(())
    }

    /// Reports the stream starting, and fails once its connection ended.
    fn check(&mut self, report: &mut dyn FnMut(Report)) -> Result<()> {
        match self.video.as_ref().map(|video| video.sender.link()) {
            Some(Link::Connected) if !self.streaming => {
                self.streaming = true;
                report(Report::Streaming);
            }
            Some(Link::Ended(reason)) => bail!("Lost the stream connection: {reason}"),
            _ => {}
        }
        Ok(())
    }
}

/// Shows someone's stream: whole frames from the connection, decoded, then
/// scaled down if they don't fit.
fn watch(
    server: StreamServer,
    mut writer: Writer,
    (max_width, max_height): (u32, u32),
    stop: &AtomicBool,
    report: &mut dyn FnMut(Report),
) -> Result<()> {
    let receiver = StreamReceiver::start(server)?;
    let mut decoder: Option<Decoder> = None;
    let mut rgba = vec![];
    let mut shown = false;
    let mut failures = 0u64;
    while !stop.load(Ordering::Relaxed) {
        if let Link::Ended(reason) = receiver.link() {
            bail!("Lost the stream: {reason}");
        }
        let Some(frame) = receiver.next(Duration::from_millis(100)) else {
            continue;
        };
        if decoder
            .as_ref()
            .is_none_or(|decoder| decoder.codec() != frame.codec)
        {
            decoder = Some(Decoder::new(frame.codec)?);
        }
        let Some(decoder) = decoder.as_mut() else {
            continue;
        };
        let picture = match decoder.decode(&frame.data) {
            Ok(Some(picture)) => picture,
            Ok(None) => continue,
            Err(error) => {
                failures += 1;
                // The first of a run, and then now and again.
                if failures == 1 || failures.is_multiple_of(100) {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        failures,
                        keyframe = frame.keyframe,
                        size = frame.data.len(),
                        "a stream frame didn't decode"
                    );
                }
                receiver.request_keyframe();
                continue;
            }
        };
        let (width, height) = (picture.width, picture.height);
        rgba.resize((width * height * 4) as usize, 0);
        picture.to_rgba(&mut rgba);
        if writer.fits(width, height) && width <= max_width && height <= max_height {
            writer.write(width, height, |pixels| pixels.copy_from_slice(&rgba));
        } else {
            let size = fit(width, height, max_width, max_height);
            writer.write(size.0, size.1, |pixels| {
                scale(&rgba, (width, height, width * 4), pixels, size)
            });
        }
        if !shown {
            shown = true;
            tracing::info!(width, height, codec = ?frame.codec, "showing the stream");
            report(Report::Streaming);
        }
    }
    Ok(())
}

/// Sleeps until the next frame is due, or returns false once told to stop.
fn next_frame(started: Instant, frame: u64, fps: u32, stop: &AtomicBool) -> bool {
    let due = started + Duration::from_secs(frame) / fps;
    if let Some(wait) = due.checked_duration_since(Instant::now()) {
        std::thread::sleep(wait);
    }
    !stop.load(Ordering::Relaxed)
}

fn test_pattern(
    mut sink: Sink,
    width: u32,
    height: u32,
    stop: &AtomicBool,
    report: &mut dyn FnMut(Report),
) -> Result<()> {
    let (width, height) = (width.min(640), height.min(360));
    report(Report::Sharing { width, height });
    let started = Instant::now();
    let mut frame = 0u64;
    let mut rgba = vec![0; (width * height * 4) as usize];
    let mut picture = I420Buffer::new(width, height);
    while next_frame(started, frame, sink.fps, stop) {
        let shift = (frame * 4) as u32;
        for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
            let (x, y) = (index as u32 % width, index as u32 / width);
            let bar = ((x + shift) * 8 / width) % 8;
            let level = 255 - (y * 128 / height) as u8;
            let on = |bit: u32| if bar & bit != 0 { level } else { 32 };
            pixel.copy_from_slice(&[on(4), on(2), on(1), 255]);
        }
        let (y_stride, u_stride, v_stride) = picture.strides();
        let (y, u, v) = picture.data_mut();
        // libyuv names formats by their order in a little-endian word: its
        // ABGR is R, G, B, A in memory.
        yuv_helper::abgr_to_i420(
            &rgba,
            width * 4,
            y,
            y_stride,
            u,
            u_stride,
            v,
            v_stride,
            width as i32,
            height as i32,
        );
        sink.frame(&mut picture)?;
        sink.check(report)?;
        frame += 1;
    }
    Ok(())
}

/// The size `width` by `height` takes when scaled down to fit `max_width`
/// by `max_height`, keeping its shape. Codecs want even sizes.
pub fn fit(width: u32, height: u32, max_width: u32, max_height: u32) -> (u32, u32) {
    let scale = f64::min(
        1.0,
        f64::min(
            f64::from(max_width) / f64::from(width.max(1)),
            f64::from(max_height) / f64::from(height.max(1)),
        ),
    );
    let even = |value: f64| ((value as u32) & !1).max(2);
    (
        even(f64::from(width) * scale),
        even(f64::from(height) * scale),
    )
}

/// Converts an I420 picture into RGBA pixels of its size.
fn to_rgba(picture: &I420Buffer, pixels: &mut [u8]) {
    let (y, u, v) = picture.data();
    let (y_stride, u_stride, v_stride) = picture.strides();
    let (width, height) = (picture.width(), picture.height());
    yuv_helper::i420_to_abgr(
        y,
        y_stride,
        u,
        u_stride,
        v,
        v_stride,
        pixels,
        width * 4,
        width as i32,
        height as i32,
    );
}

/// Scales RGBA pixels down, averaging each 2 by 2 block it lands on.
fn scale(
    source: &[u8],
    (width, height, stride): (u32, u32, u32),
    target: &mut [u8],
    (target_width, target_height): (u32, u32),
) {
    let at = |x: u32, y: u32| (y.min(height - 1) * stride + x.min(width - 1) * 4) as usize;
    for y in 0..target_height {
        let sy = y * height / target_height;
        for x in 0..target_width {
            let sx = x * width / target_width;
            let corners = [
                at(sx, sy),
                at(sx + 1, sy),
                at(sx, sy + 1),
                at(sx + 1, sy + 1),
            ];
            let average = |channel: usize| {
                (corners
                    .iter()
                    .map(|&corner| u32::from(source.get(corner + channel).copied().unwrap_or(0)))
                    .sum::<u32>()
                    / 4) as u8
            };
            let out = ((y * target_width + x) * 4) as usize;
            target[out..out + 4].copy_from_slice(&[average(0), average(1), average(2), 255]);
        }
    }
}

// libwebrtc asks the screen sharing portal over D-Bus through GIO, but
// webrtc-sys only compiles against its headers.
#[cfg(target_os = "linux")]
#[link(name = "gio-2.0")]
#[link(name = "gobject-2.0")]
#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn g_main_context_iteration(
        context: *mut std::ffi::c_void,
        may_block: std::ffi::c_int,
    ) -> std::ffi::c_int;
}

/// Runs what's due on GLib's main context. The portal's D-Bus replies arrive
/// there, and nothing else runs it in this process: without this the
/// dialog never opens.
fn run_glib() {
    #[cfg(target_os = "linux")]
    // SAFETY: the default context; only the capture thread iterates it.
    while unsafe { g_main_context_iteration(std::ptr::null_mut(), 0) } != 0 {}
}

#[derive(Default)]
struct Captured {
    size: Option<(u32, u32)>,
    /// Capture stopped for good, or a frame couldn't be sent.
    failed: Option<String>,
}

fn capture_screen(
    sink: Sink,
    max_width: u32,
    max_height: u32,
    stop: &AtomicBool,
    report: &mut dyn FnMut(Report),
) -> Result<()> {
    use livekit::webrtc::desktop_capturer::{
        CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
    };
    // On Linux the system's screen sharing dialog picks a screen or window.
    #[cfg(target_os = "linux")]
    let kind = DesktopCaptureSourceType::Generic;
    #[cfg(not(target_os = "linux"))]
    let kind = DesktopCaptureSourceType::Screen;
    let mut options = DesktopCapturerOptions::new(kind);
    options.set_include_cursor(true);
    let mut capturer =
        DesktopCapturer::new(options).context("Screen sharing isn't available on this system.")?;
    let fps = sink.fps;
    let sink = Arc::new(Mutex::new(sink));
    let captured = Arc::new(Mutex::new(Captured::default()));
    let (into, seen) = (sink.clone(), captured.clone());
    #[cfg(target_os = "linux")]
    let source = None;
    #[cfg(not(target_os = "linux"))]
    let source = capturer.get_source_list().into_iter().next();
    capturer.start_capture(source, move |result| {
        let mut seen = seen.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(frame) => {
                let (width, height) = (frame.width().max(1) as u32, frame.height().max(1) as u32);
                let mut picture = I420Buffer::new(width, height);
                let (y_stride, u_stride, v_stride) = picture.strides();
                let (y, u, v) = picture.data_mut();
                // libyuv's ARGB is B, G, R, A in memory, as the capturer's frames are.
                yuv_helper::argb_to_i420(
                    frame.data(),
                    frame.stride(),
                    y,
                    y_stride,
                    u,
                    u_stride,
                    v,
                    v_stride,
                    width as i32,
                    height as i32,
                );
                let size = fit(width, height, max_width, max_height);
                if size != (width, height) {
                    picture = picture.scale(size.0 as i32, size.1 as i32);
                }
                let taken = into
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .frame(&mut picture);
                match taken {
                    Ok(()) => seen.size = Some(size),
                    Err(error) => seen.failed = Some(format!("{error:#}")),
                }
            }
            // While the dialog is open, or between frames.
            Err(CaptureError::Temporary) => {}
            Err(CaptureError::Permanent) => {
                seen.failed
                    .get_or_insert_with(|| "Screen sharing stopped.".into());
            }
        }
    });
    let started = Instant::now();
    let mut frame = 0u64;
    let mut reported = None;
    while next_frame(started, frame, fps, stop) {
        run_glib();
        capturer.capture_frame();
        frame += 1;
        let seen = captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(failure) = &seen.failed {
            bail!(if reported.is_none() {
                "Screen sharing was cancelled, or this desktop has no screen sharing dialog."
                    .to_owned()
            } else {
                failure.clone()
            });
        }
        if seen.size != reported
            && let Some((width, height)) = seen.size
        {
            reported = seen.size;
            report(Report::Sharing { width, height });
        }
        drop(seen);
        sink.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .check(report)?;
    }
    Ok(())
}

/// Talk's handle on a running worker. Dropping it stops the worker.
pub struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Worker {
    /// Starts sharing `source`, with frames for the UI in a new [`Frames`].
    pub fn share(source: Source) -> Result<(Self, Frames, mpsc::UnboundedReceiver<Report>)> {
        let frames = Frames::create(PREVIEW_WIDTH, PREVIEW_HEIGHT)?;
        let job = Job::Share {
            source,
            frames: frames.path().context("the video frames file has no name")?,
            width: SHARE_WIDTH,
            height: SHARE_HEIGHT,
            fps: FPS,
        };
        let (worker, reports) = Self::start(&job)?;
        Ok((worker, frames, reports))
    }

    /// Starts watching the stream at `server`, with pictures for the UI in a
    /// new [`Frames`].
    pub fn watch(server: &StreamServer) -> Result<(Self, Frames, mpsc::UnboundedReceiver<Report>)> {
        let frames = Frames::create(WATCH_WIDTH, WATCH_HEIGHT)?;
        let job = Job::Watch {
            server: server.clone(),
            frames: frames.path().context("the video frames file has no name")?,
            width: WATCH_WIDTH,
            height: WATCH_HEIGHT,
        };
        let (worker, reports) = Self::start(&job)?;
        Ok((worker, frames, reports))
    }

    /// Hands the worker the server Discord started the stream on.
    pub fn send(&mut self, server: &StreamServer) -> Result<()> {
        let line = serde_json::to_string(&Control::Send(server.clone()))?;
        let stdin = self
            .stdin
            .as_mut()
            .context("the stream worker has stopped")?;
        writeln!(stdin, "{line}").context("could not reach the stream worker")?;
        Ok(())
    }

    fn start(job: &Job) -> Result<(Self, mpsc::UnboundedReceiver<Report>)> {
        // Tests run from their own binary, so they name Talk's.
        let program = match std::env::var_os("TALK_STREAM_WORKER") {
            Some(program) => PathBuf::from(program),
            None => std::env::current_exe().context("could not find Talk to start streaming")?,
        };
        let mut child = Command::new(program)
            .arg(ARGUMENT)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("could not start the stream worker")?;
        let mut stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let line = serde_json::to_string(job)?;
        if let Some(stdin) = stdin.as_mut() {
            writeln!(stdin, "{line}").context("could not reach the stream worker")?;
        }
        let (reports, receiver) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("talk-stream-reports".into())
            .spawn(move || {
                let mut ended = false;
                for line in stdout
                    .into_iter()
                    .flat_map(|stdout| BufReader::new(stdout).lines().map_while(Result::ok))
                {
                    match serde_json::from_str::<Report>(&line) {
                        Ok(report) => {
                            ended |= matches!(report, Report::Ended { .. });
                            if reports.send(report).is_err() {
                                return;
                            }
                        }
                        Err(error) => tracing::warn!(%error, "unknown stream worker report"),
                    }
                }
                if !ended {
                    let _ = reports.send(Report::Ended {
                        error: Some("Screen sharing stopped unexpectedly.".into()),
                    });
                }
            })
            .context("could not start the stream worker")?;
        Ok((Self { child, stdin }, receiver))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "stop");
        }
        // The worker checks for the stop between frames.
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        tracing::warn!("the stream worker didn't stop; ending it");
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_frames_with_even_sizes() {
        assert_eq!(fit(1920, 1080, 1280, 720), (1280, 720));
        assert_eq!(fit(3440, 1440, 1280, 720), (1280, 534));
        assert_eq!(fit(801, 601, 1280, 720), (800, 600), "never larger");
    }

    #[test]
    fn aims_for_discords_bitrates() {
        assert_eq!(kbps(720, None), 3500);
        assert_eq!(kbps(1080, None), 9000);
        assert_eq!(kbps(1080, Some(4_000_000)), 4000, "what the path carries");
        assert_eq!(kbps(720, Some(9_000_000)), 3500, "never past the cap");
        assert_eq!(kbps(1080, Some(100_000)), 500, "nor below a screen's floor");
    }

    #[test]
    fn scales_rgba_down() {
        // Two by two red-green-blue-black pixels make one average.
        let source = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 255];
        let mut target = [0; 4];
        scale(&source, (2, 2, 8), &mut target, (1, 1));
        assert_eq!(target, [63, 63, 63, 255]);
    }

    #[test]
    fn previews_what_it_sends_at_a_smaller_size() {
        let frames = Frames::create(PREVIEW_WIDTH, PREVIEW_HEIGHT).unwrap();
        let (_controls, received) = channel::channel();
        let mut sink = Sink {
            writer: Writer::open(&frames.path().unwrap()).unwrap(),
            controls: received,
            sound: false,
            video: None,
            fps: 30,
            started: Instant::now(),
            streaming: false,
        };
        // A 1080p picture, mid-grey: luma and chroma halfway.
        let mut picture = I420Buffer::new(SHARE_WIDTH, SHARE_HEIGHT);
        let (y, u, v) = picture.data_mut();
        y.fill(126);
        u.fill(128);
        v.fill(128);
        sink.frame(&mut picture).unwrap();
        let (width, height, first) = frames
            .take(|width, height, pixels| (width, height, pixels[..4].to_vec()))
            .unwrap();
        assert_eq!((width, height), (PREVIEW_WIDTH, PREVIEW_HEIGHT));
        assert!(
            first[..3].iter().all(|&level| level.abs_diff(128) < 4),
            "{first:?}"
        );
        assert_eq!(first[3], 255);
    }

    #[test]
    fn serves_a_test_pattern_until_stopped() {
        let frames = Frames::create(PREVIEW_WIDTH, PREVIEW_HEIGHT).unwrap();
        let job = Job::Share {
            source: Source::TestPattern,
            frames: frames.path().unwrap(),
            width: PREVIEW_WIDTH,
            height: PREVIEW_HEIGHT,
            fps: 60,
        };
        let (input, mut feed) = std::io::pipe().unwrap();
        writeln!(feed, "{}", serde_json::to_string(&job).unwrap()).unwrap();
        let (sender, reports) = std::sync::mpsc::channel();
        let serving = std::thread::spawn(move || {
            serve(Box::new(BufReader::new(input)), &mut |report| {
                sender.send(report).unwrap();
            });
        });
        assert_eq!(reports.recv().unwrap(), Report::Ready);
        assert_eq!(
            reports.recv().unwrap(),
            Report::Sharing {
                width: 640,
                height: 360
            }
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let size = loop {
            if let Some(size) = frames.take(|width, height, pixels| (width, height, pixels.len())) {
                break size;
            }
            assert!(Instant::now() < deadline, "no frame arrived");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(size, (640, 360, 640 * 360 * 4));
        writeln!(feed, "stop").unwrap();
        serving.join().unwrap();
        assert_eq!(reports.recv().unwrap(), Report::Ended { error: None });
    }
}
