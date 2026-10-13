//! Cameras: listing them, and capturing one for a call. Linux and Windows
//! capture with libwebrtc's capture module (`native/camera.cc`), through V4L2
//! and DirectShow, which converts each frame into the same two I420 buffers.
//!
//! `TALK_CAMERA_TEST_PATTERN=1` adds a camera that films moving colour bars,
//! for tests and screenshots on machines without a camera.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};

use crate::{picture::I420, voice::Device};

/// The largest picture captured, and how often: 720p at 30 a second, as
/// Discord's client films without Nitro and LiveKit's default camera does.
/// Tiles in a call show less than this; the encoder sends this much.
pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
pub const FPS: u32 = 30;

/// The test pattern's id and name, and its size.
const TEST_PATTERN: &str = "test-pattern";
const TEST_PATTERN_SIZE: (u32, u32) = (640, 360);

/// What a camera's pictures go to, on the camera's own thread. The planes
/// last until it returns.
pub type Sink = Box<dyn FnMut(&I420<'_>) + Send>;

fn test_pattern() -> bool {
    std::env::var_os("TALK_CAMERA_TEST_PATTERN").is_some_and(|value| value != "0")
}

/// The cameras on this system, without opening any of them.
pub fn list() -> Vec<Device> {
    let mut devices = vec![];
    if test_pattern() {
        devices.push(Device {
            id: TEST_PATTERN.into(),
            name: "Test pattern".into(),
        });
    }
    #[cfg(any(target_os = "linux", windows))]
    native::list(&mut devices);
    devices
}

/// libwebrtc's capture module, through `native/camera.cc`.
#[cfg(any(target_os = "linux", windows))]
mod native {
    use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};

    use anyhow::{Context, Result, bail};

    use super::{FPS, HEIGHT, Sink, WIDTH};
    use crate::{
        picture::{I420, RawPicture},
        voice::Device,
    };

    #[repr(C)]
    struct RawCamera {
        _private: [u8; 0],
    }

    type RawSink = extern "C" fn(user: *mut c_void, picture: *const RawPicture);

    unsafe extern "C" {
        fn talk_camera_device(
            index: c_uint,
            name: *mut c_char,
            name_length: c_uint,
            id: *mut c_char,
            id_length: c_uint,
        ) -> c_int;
        fn talk_camera_open(
            id: *const c_char,
            width: c_int,
            height: c_int,
            fps: c_int,
            frame: RawSink,
            user: *mut c_void,
        ) -> *mut RawCamera;
        fn talk_camera_close(camera: *mut RawCamera);
    }

    pub(super) fn list(devices: &mut Vec<Device>) {
        let mut index = 0;
        loop {
            let (mut name, mut id) = ([0 as c_char; 256], [0 as c_char; 1024]);
            // SAFETY: both buffers are as long as said, and come back
            // terminated.
            let count = unsafe {
                talk_camera_device(
                    index,
                    name.as_mut_ptr(),
                    name.len() as c_uint,
                    id.as_mut_ptr(),
                    id.len() as c_uint,
                )
            };
            if count < 0 {
                index += 1;
                continue;
            }
            if index >= count as c_uint {
                break;
            }
            // SAFETY: terminated above.
            let text = |chars: &[c_char]| {
                unsafe { CStr::from_ptr(chars.as_ptr()) }
                    .to_string_lossy()
                    .into_owned()
            };
            devices.push(Device {
                id: text(&id),
                name: text(&name),
            });
            index += 1;
        }
    }

    /// A camera filming through the module until dropped.
    pub(super) struct Camera {
        raw: *mut RawCamera,
        /// Where the module sends pictures; it must outlive the camera.
        _sink: Box<Sink>,
    }

    // SAFETY: the camera is only opened and closed through this handle, from
    // whichever thread holds it; pictures arrive on the module's own thread.
    unsafe impl Send for Camera {}

    extern "C" fn deliver(user: *mut c_void, picture: *const RawPicture) {
        // SAFETY: `user` is the camera's boxed sink, alive until the camera
        // closes, which waits for its thread; the planes last for this call.
        let sink = unsafe { &mut *user.cast::<Sink>() };
        if let Some(picture) = unsafe { picture.as_ref().and_then(|raw| I420::from_raw(raw)) } {
            sink(&picture);
        }
    }

    impl Camera {
        pub(super) fn open(id: &str, sink: Sink) -> Result<Self> {
            let id = CString::new(id).context("not a camera id")?;
            let mut sink = Box::new(sink);
            let user = std::ptr::from_mut::<Sink>(sink.as_mut()).cast::<c_void>();
            // SAFETY: the id is terminated, and `user` stays boxed until the
            // camera closes.
            let raw = unsafe {
                talk_camera_open(
                    id.as_ptr(),
                    WIDTH as c_int,
                    HEIGHT as c_int,
                    FPS as c_int,
                    deliver,
                    user,
                )
            };
            if raw.is_null() {
                bail!("Could not start your camera. It may be in use by another app.");
            }
            Ok(Self { raw, _sink: sink })
        }
    }

    impl Drop for Camera {
        fn drop(&mut self) {
            // SAFETY: opened by talk_camera_open and closed once; no picture
            // arrives after this returns, so the sink can go.
            unsafe { talk_camera_close(self.raw) };
        }
    }
}

/// A camera filming until dropped.
#[derive(Default)]
pub struct Camera {
    #[cfg(target_os = "linux")]
    _native: Option<native::Camera>,
    /// The thread that films, told to stop through the flag: the test
    /// pattern's, or on Windows the one holding the native camera.
    thread: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
}

impl Camera {
    /// Starts filming with the camera `id`, or the first one, sending its
    /// pictures to `sink`.
    pub fn open(id: Option<&str>, sink: Sink) -> Result<Self> {
        let devices = list();
        let device = match id {
            Some(id) => devices.iter().find(|device| device.id == id),
            None => devices.first(),
        };
        let Some(device) = device.or_else(|| devices.first()) else {
            bail!("No camera found. Connect one and try again.");
        };
        if device.id == TEST_PATTERN {
            return Ok(Self::pattern(sink));
        }
        Self::native(&device.id, sink)
    }

    #[cfg(target_os = "linux")]
    fn native(id: &str, sink: Sink) -> Result<Self> {
        Ok(Self {
            _native: Some(native::Camera::open(id, sink)?),
            thread: None,
        })
    }

    /// DirectShow's objects live in a COM apartment that the module joins
    /// on the thread opening the camera, and leaves on the one closing it.
    /// A thread of the camera's own does both, so no other thread's COM
    /// apartment is joined or left by mistake.
    #[cfg(windows)]
    fn native(id: &str, sink: Sink) -> Result<Self> {
        let id = id.to_owned();
        let (opened, result) = std::sync::mpsc::sync_channel(1);
        let camera = Self::spawn(move |stop| {
            let camera = match native::Camera::open(&id, sink) {
                Ok(camera) => camera,
                Err(error) => {
                    let _ = opened.send(Err(error));
                    return;
                }
            };
            let _ = opened.send(Ok(()));
            while !stop.load(Ordering::Acquire) {
                std::thread::park();
            }
            drop(camera);
        })?;
        match result.recv() {
            Ok(Ok(())) => Ok(camera),
            Ok(Err(error)) => Err(error),
            Err(_) => bail!("Could not start your camera."),
        }
    }

    #[cfg(not(any(target_os = "linux", windows)))]
    fn native(_: &str, _: Sink) -> Result<Self> {
        bail!("Cameras work on Linux and Windows for now.")
    }

    /// Runs `film` on the camera's thread until it returns; it's told to
    /// stop through the flag, and woken if parked.
    fn spawn(film: impl FnOnce(&AtomicBool) + Send + 'static) -> std::io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::Builder::new()
            .name("talk-camera".into())
            .spawn(move || film(&stopped))?;
        Ok(Self {
            #[cfg(target_os = "linux")]
            _native: None,
            thread: Some((stop, thread)),
        })
    }

    /// Films moving colour bars at 30 frames a second.
    fn pattern(mut sink: Sink) -> Self {
        let film = move |stopped: &AtomicBool| {
            let (width, height) = TEST_PATTERN_SIZE;
            let chroma = (width / 2 * height / 2) as usize;
            let (mut y, u, v) = (
                vec![0u8; (width * height) as usize],
                vec![128u8; chroma],
                vec![128u8; chroma],
            );
            let started = Instant::now();
            let mut frame = 0u32;
            while !stopped.load(Ordering::Relaxed) {
                let due = started + Duration::from_secs(u64::from(frame)) / FPS;
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                // Grey bars sliding across, with a bright block bouncing
                // over them, so a still picture shows as one.
                for (row, line) in y.chunks_exact_mut(width as usize).enumerate() {
                    for (column, luma) in line.iter_mut().enumerate() {
                        *luma = 40 + (((column as u32 + frame * 4) / 80) % 6 * 30) as u8;
                        let block = (frame * 6) % (width - 80);
                        if (column as u32).wrapping_sub(block) < 80 && (row as u32) % 120 < 80 {
                            *luma = 235;
                        }
                    }
                }
                let picture = I420 {
                    width,
                    height,
                    y: &y,
                    u: &u,
                    v: &v,
                    strides: [width, width / 2, width / 2],
                };
                sink(&picture);
                frame += 1;
            }
        };
        Self::spawn(film).unwrap_or_default()
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        if let Some((stop, thread)) = self.thread.take() {
            stop.store(true, Ordering::Release);
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_pattern_films_until_dropped() {
        // SAFETY: tests that read this variable all set it the same way.
        unsafe { std::env::set_var("TALK_CAMERA_TEST_PATTERN", "1") };
        assert!(list().iter().any(|device| device.id == TEST_PATTERN));
        let (frames, received) = std::sync::mpsc::channel();
        let camera = Camera::open(
            Some(TEST_PATTERN),
            Box::new(move |picture| {
                let _ = frames.send((picture.width, picture.height, picture.y[0]));
            }),
        )
        .unwrap();
        let first = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((first.0, first.1), TEST_PATTERN_SIZE);
        drop(camera);
        while received.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_millis(100));
        assert!(received.try_recv().is_err(), "stopped");
    }

    /// Films the first real camera for a second, or the first whose name
    /// holds `TALK_TEST_CAMERA`: run with a camera, or a V4L2 loopback
    /// device something writes to.
    #[test]
    #[ignore = "needs a camera"]
    fn films_a_real_camera() {
        let devices: Vec<Device> = list()
            .into_iter()
            .filter(|device| device.id != TEST_PATTERN)
            .collect();
        println!("cameras: {devices:?}");
        let named = std::env::var("TALK_TEST_CAMERA").unwrap_or_default();
        let device = devices.iter().find(|device| device.name.contains(&named));
        let (frames, received) = std::sync::mpsc::channel();
        let camera = Camera::open(
            device.map(|device| device.id.as_str()),
            Box::new(move |picture| {
                let _ = frames.send((picture.width, picture.height));
            }),
        )
        .unwrap();
        let started = Instant::now();
        let mut count = 0;
        while started.elapsed() < Duration::from_secs(1) {
            if let Ok(size) = received.recv_timeout(Duration::from_secs(2)) {
                count += 1;
                assert!(size.0 <= WIDTH && size.1 <= HEIGHT, "{size:?}");
            }
        }
        drop(camera);
        println!("{count} frames in a second");
        assert!(count >= 10);
    }
}
