//! Sound through WASAPI in shared mode. The audio engine converts between a
//! stream's format and the device's, and signals an event as it plays or
//! records; reading and writing wait for it on the caller's thread, and copy
//! straight between the caller's samples and the engine's buffer.
//!
//! WebRTC's Windows device module names each device by its endpoint id, as
//! `IMMDevice::GetId` gives it, so a device id opens the endpoint directly.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, ensure};
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        Media::{
            Audio::{
                AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, EDataFlow,
                IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice,
                IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
                WAVEFORMATEXTENSIBLE_0, eCapture, eConsole, eRender,
            },
            KernelStreaming::{
                KSDATAFORMAT_SUBTYPE_PCM, SPEAKER_FRONT_CENTER, SPEAKER_FRONT_LEFT,
                SPEAKER_FRONT_RIGHT, WAVE_FORMAT_EXTENSIBLE,
            },
            Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        },
        System::{
            Com::{
                CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
            },
            Threading::{CreateEventW, WaitForSingleObject},
        },
    },
    core::HSTRING,
};

use super::Format;
use crate::voice::NAMED;

/// What a stream without a queue of its own keeps queued.
const DEFAULT_QUEUE: Duration = Duration::from_millis(200);
/// The least a microphone's buffer holds, so a reader that's a little late
/// loses nothing.
const RECORDING: Duration = Duration::from_millis(200);
/// The engine signals every few milliseconds while a stream runs; this long
/// without a signal means the device went away.
const SILENCE_MS: u32 = 2000;

/// Each thread that opens or uses a stream joins COM's multithreaded
/// apartment, in which the engine's objects work from any of its threads.
struct Apartment(bool);

impl Apartment {
    fn join() -> Self {
        // SAFETY: balanced by CoUninitialize when the thread ends; a thread
        // already in a single-threaded apartment stays there.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: this thread's CoInitializeEx succeeded.
            unsafe { CoUninitialize() };
        }
    }
}

thread_local! {
    static APARTMENT: Apartment = Apartment::join();
}

fn com() {
    APARTMENT.with(|_| {});
}

/// The endpoint behind a device id, or the default one for `None`.
fn device(id: Option<&str>, flow: EDataFlow) -> Result<IMMDevice> {
    com();
    // SAFETY: plain COM calls on this thread's apartment.
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("no audio device service")?;
        // Ids made from a name come from PulseAudio, not from here.
        match id.filter(|id| !id.starts_with(NAMED)) {
            None => enumerator
                .GetDefaultAudioEndpoint(flow, eConsole)
                .context("no default device"),
            Some(id) => {
                let device = enumerator
                    .GetDevice(&HSTRING::from(id))
                    .context("device not found")?;
                ensure!(
                    device.GetState()? == DEVICE_STATE_ACTIVE,
                    "device not connected"
                );
                Ok(device)
            }
        }
    }
}

fn wave_format(format: Format) -> WAVEFORMATEXTENSIBLE {
    let bits = if format.float { 32 } else { 16 };
    let block = format.frame_bytes() as u16;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: format.channels,
            nSamplesPerSec: format.rate,
            nAvgBytesPerSec: format.rate * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: bits,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: bits,
        },
        dwChannelMask: if format.channels == 1 {
            SPEAKER_FRONT_CENTER
        } else {
            SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT
        },
        SubFormat: if format.float {
            KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            KSDATAFORMAT_SUBTYPE_PCM
        },
    }
}

/// In WASAPI's 100 ns units.
fn hundred_nanoseconds(duration: Duration) -> i64 {
    (duration.as_nanos() / 100) as i64
}

/// An initialized shared-mode stream and the event its engine signals.
struct Stream {
    client: IAudioClient,
    event: HANDLE,
    format: Format,
    /// The engine's buffer, in frames.
    buffer: u32,
    running: bool,
}

// SAFETY: the engine's objects live in the multithreaded apartment, which
// every thread using them joins first (`com`).
unsafe impl Send for Stream {}

impl Stream {
    fn open(device: &IMMDevice, format: Format, buffer: Duration) -> Result<Self> {
        // SAFETY: plain COM calls; the format outlives Initialize.
        unsafe {
            let client: IAudioClient = device
                .Activate(CLSCTX_ALL, None)
                .context("the device refused a stream")?;
            let wave = wave_format(format);
            client
                .Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    hundred_nanoseconds(buffer),
                    0,
                    (&raw const wave).cast::<WAVEFORMATEX>(),
                    None,
                )
                .context("the device refused the stream's format")?;
            let event = CreateEventW(None, false, false, None)?;
            // Built first, so dropping it closes the event on an error.
            let mut stream = Self {
                buffer: 0,
                client,
                event,
                format,
                running: false,
            };
            stream.client.SetEventHandle(stream.event)?;
            stream.buffer = stream.client.GetBufferSize()?;
            Ok(stream)
        }
    }

    fn start(&mut self) -> Result<()> {
        if !self.running {
            // SAFETY: an initialized client.
            unsafe { self.client.Start() }.context("the device wouldn't start")?;
            self.running = true;
        }
        Ok(())
    }

    fn stop(&mut self) {
        if self.running {
            // SAFETY: as above.
            let _ = unsafe { self.client.Stop() };
            self.running = false;
        }
    }

    /// Frames waiting in the engine's buffer.
    fn padding(&self) -> Result<u32> {
        // SAFETY: as above.
        unsafe { self.client.GetCurrentPadding() }.context("the device went away")
    }

    /// Waits for the engine to play or record more.
    fn wait(&self) -> Result<()> {
        // SAFETY: the event lives as long as the stream.
        let waited = unsafe { WaitForSingleObject(self.event, SILENCE_MS) };
        ensure!(waited == WAIT_OBJECT_0, "the device stopped");
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        com();
        self.stop();
        // SAFETY: the stream owns its event, and the engine stopped using it.
        let _ = unsafe { CloseHandle(self.event) };
    }
}

pub struct Output {
    stream: Stream,
    render: IAudioRenderClient,
    /// The most kept queued, in frames: what was asked, though the engine's
    /// buffer may be larger.
    queue: u32,
}

impl Output {
    pub fn open(
        speaker: Option<&str>,
        _name: &'static str,
        format: Format,
        queue: Option<Duration>,
    ) -> Result<Self> {
        let device = device(speaker, eRender).context("these speakers are no longer connected")?;
        let queue = queue.unwrap_or(DEFAULT_QUEUE);
        let stream = Stream::open(&device, format, queue)?;
        // SAFETY: an initialized client.
        let render = unsafe { stream.client.GetService::<IAudioRenderClient>() }?;
        let queue = ((format.bytes(queue) / format.frame_bytes()) as u32).clamp(1, stream.buffer);
        Ok(Self {
            stream,
            render,
            queue,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
        com();
        let frame_bytes = self.stream.format.frame_bytes();
        let mut rest = bytes;
        while rest.len() >= frame_bytes {
            let padding = self.stream.padding()?;
            let room = self.queue.saturating_sub(padding);
            if room == 0 {
                // Full: it plays once it has all it may queue.
                self.stream.start()?;
                self.stream.wait()?;
                continue;
            }
            let count = room.min((rest.len() / frame_bytes) as u32);
            let length = count as usize * frame_bytes;
            // SAFETY: the engine lends `count` frames of room, which we fill
            // from the caller's samples before giving them back.
            unsafe {
                let buffer = self.render.GetBuffer(count)?;
                std::ptr::copy_nonoverlapping(rest.as_ptr(), buffer, length);
                self.render.ReleaseBuffer(count, 0)?;
            }
            rest = &rest[length..];
        }
        Ok(())
    }

    pub fn drain(&mut self) -> Result<()> {
        com();
        if self.stream.padding()? == 0 {
            return Ok(());
        }
        self.stream.start()?;
        let deadline = Instant::now()
            + Duration::from_secs_f64(f64::from(self.queue) / f64::from(self.stream.format.rate))
            + Duration::from_secs(1);
        while self.stream.padding()? > 0 {
            ensure!(Instant::now() < deadline, "the speakers stopped playing");
            self.stream.wait()?;
        }
        self.stream.stop();
        Ok(())
    }

    pub fn flush(&mut self) {
        com();
        self.stream.stop();
        // SAFETY: a stopped client.
        let _ = unsafe { self.stream.client.Reset() };
    }

    pub fn latency(&self) -> Duration {
        com();
        let queued = self.stream.padding().unwrap_or(0);
        // SAFETY: an initialized client.
        let engine = unsafe { self.stream.client.GetStreamLatency() }.unwrap_or(0);
        Duration::from_secs_f64(f64::from(queued) / f64::from(self.stream.format.rate))
            + Duration::from_nanos(engine.max(0) as u64 * 100)
    }
}

// SAFETY: as for `Stream`.
unsafe impl Send for Output {}

pub struct Input {
    stream: Stream,
    capture: IAudioCaptureClient,
    /// The end of the last packet, which the last read had no room for.
    left: Vec<u8>,
    taken: usize,
}

// SAFETY: as for `Stream`.
unsafe impl Send for Input {}

impl Input {
    pub fn open(
        microphone: Option<&str>,
        _name: &'static str,
        format: Format,
        fragment: Duration,
    ) -> Result<Self> {
        let device =
            device(microphone, eCapture).context("this microphone is no longer connected")?;
        let mut stream = Stream::open(&device, format, (fragment * 4).max(RECORDING))?;
        // SAFETY: an initialized client.
        let capture = unsafe { stream.client.GetService::<IAudioCaptureClient>() }?;
        let left = Vec::with_capacity(stream.buffer as usize * format.frame_bytes());
        stream.start()?;
        Ok(Self {
            stream,
            capture,
            left,
            taken: 0,
        })
    }

    pub fn read(&mut self, bytes: &mut [u8]) -> Result<()> {
        com();
        let left = &self.left[self.taken..];
        let mut filled = left.len().min(bytes.len());
        bytes[..filled].copy_from_slice(&left[..filled]);
        self.taken += filled;
        while filled < bytes.len() {
            // SAFETY: an initialized, running client.
            let packet =
                unsafe { self.capture.GetNextPacketSize() }.context("the microphone went away")?;
            if packet == 0 {
                self.stream.wait()?;
                continue;
            }
            let (mut data, mut count, mut flags) = (std::ptr::null_mut(), 0, 0);
            // SAFETY: the engine lends `count` frames at `data` until
            // ReleaseBuffer; they're copied out before that.
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut count, &mut flags, None, None)
                    .context("the microphone went away")?;
                let length = count as usize * self.stream.format.frame_bytes();
                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null();
                let packet: &[u8] = if silent {
                    &[]
                } else {
                    std::slice::from_raw_parts(data, length)
                };
                let wanted = (bytes.len() - filled).min(length);
                let into = &mut bytes[filled..filled + wanted];
                match packet.get(..wanted) {
                    Some(samples) => into.copy_from_slice(samples),
                    None => into.fill(0),
                }
                filled += wanted;
                self.left.clear();
                self.taken = 0;
                if wanted < length {
                    match packet.get(wanted..) {
                        Some(rest) => self.left.extend_from_slice(rest),
                        None => self.left.resize(length - wanted, 0),
                    }
                }
                self.capture
                    .ReleaseBuffer(count)
                    .map_err(|error| anyhow!("{error}"))?;
            }
        }
        Ok(())
    }
}

#[cfg(feature = "discord")]
pub fn others(
    _: fn(&str) -> bool,
    _: super::Hear,
    _: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<std::thread::JoinHandle<()>> {
    anyhow::bail!("Talk shares sound on Linux only for now")
}
