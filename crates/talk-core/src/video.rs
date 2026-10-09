//! Video files, through native/video.c: MP4, MOV and WebM, with H264, VP8,
//! VP9 or AV1 pictures and AAC, Opus, MP3, Vorbis or FLAC sound. Tenor and
//! Giphy GIFs reach Discord as MP4s, which Talk decodes into an animation as
//! it does a GIF's frames; [`crate::video_player`] plays the rest.

use std::{
    ffi::{c_int, c_uchar, c_uint, c_void},
    io::{Read, Seek, SeekFrom},
    sync::Arc,
};

use anyhow::{Result, bail};

pub use crate::chat::fit;
use crate::chat::{Animation, Picture};

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct Info {
    width: c_int,
    height: c_int,
    duration_ms: i64,
    sample_rate: c_int,
    channels: c_int,
}

type ReadFn = unsafe extern "C" fn(*mut c_void, *mut u8, c_int) -> c_int;
type SeekFn = unsafe extern "C" fn(*mut c_void, i64, c_int) -> i64;

unsafe extern "C" {
    fn talk_video_open(
        source: *mut c_void,
        read: ReadFn,
        seek: SeekFn,
        threads: c_uint,
        sound: c_int,
        info: *mut Info,
    ) -> *mut Raw;
    fn talk_video_next(video: *mut Raw, time_ms: *mut i64) -> c_int;
    fn talk_video_picture(
        video: *mut Raw,
        rgba: *mut c_uchar,
        width: c_int,
        height: c_int,
    ) -> c_int;
    fn talk_video_sound(video: *const Raw, frames: *mut c_int) -> *const f32;
    fn talk_video_seek(video: *mut Raw, time_ms: i64) -> c_int;
    fn talk_video_free(video: *mut Raw);
}

/// FFmpeg's `AVSEEK_SIZE`: asks for the file's size rather than moving.
const SEEK_SIZE: c_int = 0x10000;

/// How much of a video becomes an animation; the rest is left out.
const ANIMATION_MS: u64 = 60_000;

/// Whether the bytes start as a video file does: an MP4 or MOV's `ftyp`
/// box, or WebM's EBML header.
pub fn is_video(data: &[u8]) -> bool {
    data.get(4..8) == Some(b"ftyp") || data.starts_with(&[0x1a, 0x45, 0xdf, 0xa3])
}

/// Where a video file is read from: memory, or a file still downloading.
pub trait Source: Read + Seek + Send {
    /// The file's whole size, once it's known.
    fn size(&mut self) -> Option<u64>;
}

impl<T: AsRef<[u8]> + Send> Source for std::io::Cursor<T> {
    fn size(&mut self) -> Option<u64> {
        Some(self.get_ref().as_ref().len() as u64)
    }
}

unsafe extern "C" fn read_source(source: *mut c_void, buffer: *mut u8, size: c_int) -> c_int {
    // SAFETY: the source is the Video's, alive while FFmpeg reads; the
    // buffer holds `size` bytes.
    let (source, buffer) = unsafe {
        (
            &mut *source.cast::<Box<dyn Source + '_>>(),
            std::slice::from_raw_parts_mut(buffer, size.max(0) as usize),
        )
    };
    match source.read(buffer) {
        Ok(read) => read as c_int,
        Err(error) => {
            tracing::debug!(%error, "stopped reading a video");
            -1
        }
    }
}

unsafe extern "C" fn seek_source(source: *mut c_void, offset: i64, whence: c_int) -> i64 {
    // SAFETY: as in read_source.
    let source = unsafe { &mut *source.cast::<Box<dyn Source + '_>>() };
    let to = match whence {
        SEEK_SIZE => return source.size().map_or(-1, |size| size as i64),
        0 => SeekFrom::Start(offset.max(0) as u64),
        1 => SeekFrom::Current(offset),
        2 => match source.size() {
            Some(size) => SeekFrom::Start((size as i64 + offset).max(0) as u64),
            None => return -1,
        },
        _ => return -1,
    };
    source.seek(to).map_or(-1, |at| at as i64)
}

/// What [`Video::decode`] decoded.
#[derive(Debug, PartialEq, Eq)]
pub enum Next {
    /// A picture, which [`Video::picture`] writes out, showing at this time
    /// in milliseconds, when the file says.
    Picture(Option<u64>),
    /// Sound, which [`Video::sound`] gives, starting at this time.
    Sound(Option<u64>),
    End,
}

/// A video file being decoded.
pub struct Video<'a> {
    raw: *mut Raw,
    /// Boxed twice so FFmpeg holds a thin pointer that stays put.
    _source: Box<Box<dyn Source + 'a>>,
    pub width: u32,
    pub height: u32,
    /// How long it plays, when the file says.
    pub duration_ms: Option<u64>,
    /// Its sound's sample rate and channels, when it has sound and Talk was
    /// asked for it.
    pub sound: Option<(u32, usize)>,
}

// SAFETY: the decoders keep no thread-local state; one thread uses a video at
// a time, and its source is Send.
unsafe impl Send for Video<'_> {}

impl<'a> Video<'a> {
    /// Opens the file's first video, when it has one Talk can decode, and
    /// with `sound`, its sound.
    pub fn open(source: impl Source + 'a, sound: bool) -> Option<Self> {
        let threads = std::thread::available_parallelism().map_or(1, |cores| cores.get().min(4));
        Self::open_with(source, sound, threads)
    }

    /// As [`Self::open`], decoding on up to `threads` threads.
    fn open_with(source: impl Source + 'a, sound: bool, threads: usize) -> Option<Self> {
        let mut source: Box<Box<dyn Source + 'a>> = Box::new(Box::new(source));
        let mut info = Info::default();
        // SAFETY: the source stays boxed in the video for as long as FFmpeg reads it.
        let raw = unsafe {
            talk_video_open(
                (&mut *source as *mut Box<dyn Source + 'a>).cast(),
                read_source,
                seek_source,
                threads as c_uint,
                c_int::from(sound),
                &mut info,
            )
        };
        if raw.is_null() {
            return None;
        }
        Some(Self {
            raw,
            _source: source,
            width: info.width as u32,
            height: info.height as u32,
            duration_ms: u64::try_from(info.duration_ms).ok(),
            sound: (info.sample_rate > 0 && info.channels > 0)
                .then_some((info.sample_rate as u32, info.channels as usize)),
        })
    }

    /// Decodes up to the next picture or sound, in the order the file has them.
    pub fn decode(&mut self) -> Result<Next> {
        let mut time_ms = -1;
        // SAFETY: raw is a live video.
        let got = unsafe { talk_video_next(self.raw, &mut time_ms) };
        let time = u64::try_from(time_ms).ok();
        match got {
            1 => Ok(Next::Picture(time)),
            2 => Ok(Next::Sound(time)),
            0 => Ok(Next::End),
            -2 => bail!("the video's pictures aren't 8-bit 4:2:0"),
            _ => bail!("could not decode the video ({got})"),
        }
    }

    /// Writes the picture [`Self::decode`] gave last into `rgba` at `width` by
    /// `height`.
    pub fn picture(&mut self, rgba: &mut [u8], width: u32, height: u32) -> Result<()> {
        if rgba.len() < width as usize * height as usize * 4 || width == 0 || height == 0 {
            bail!(
                "a {width}×{height} picture doesn't fit {} bytes",
                rgba.len()
            );
        }
        // SAFETY: rgba holds `height` rows of `width * 4` bytes.
        let done = unsafe {
            talk_video_picture(self.raw, rgba.as_mut_ptr(), width as c_int, height as c_int)
        };
        if done < 0 {
            bail!("could not convert the video's picture");
        }
        Ok(())
    }

    /// The sound [`Self::decode`] gave last: interleaved samples, one a
    /// channel, at [`Self::sound`]'s rate.
    pub fn sound(&self) -> &[f32] {
        let mut frames = 0;
        // SAFETY: the samples stay in the video until the next call on it,
        // which borrowing self forbids meanwhile.
        unsafe {
            let samples = talk_video_sound(self.raw, &mut frames);
            let channels = self.sound.map_or(0, |(_, channels)| channels);
            if samples.is_null() || frames <= 0 || channels == 0 {
                return &[];
            }
            std::slice::from_raw_parts(samples, frames as usize * channels)
        }
    }

    /// Moves to the last keyframe at or before `time_ms`; what comes before
    /// that time is for the caller to skip.
    pub fn seek(&mut self, time_ms: u64) -> Result<()> {
        // SAFETY: raw is a live video.
        if unsafe { talk_video_seek(self.raw, time_ms as i64) } < 0 {
            bail!("could not move to {time_ms} ms in the video");
        }
        Ok(())
    }
}

impl Drop for Video<'_> {
    fn drop(&mut self) {
        // SAFETY: raw came from talk_video_open and is freed once, before its source.
        unsafe { talk_video_free(self.raw) };
    }
}

/// A video as an animation that fits `width` by `height`, as a GIF is:
/// silent, looping, at most a minute of it. Each picture shows until the
/// next one's time; the last, and any without a time, as long as the one
/// before. A video that fails partway shows what came before.
pub fn decode_animation(data: &[u8], width: u32, height: u32) -> Option<Picture> {
    let mut video = Video::open(std::io::Cursor::new(data), false)?;
    let (width, height) = fit(video.width, video.height, width, height);
    let mut first = None;
    let mut delays = Vec::new();
    let (mut start, mut delay) = (None::<u64>, 100);
    loop {
        let time = match video.decode() {
            Ok(Next::Picture(time)) => time,
            Ok(Next::Sound(_)) => continue,
            Ok(Next::End) => break,
            Err(error) => {
                tracing::debug!(%error, "could not decode a video's picture");
                break;
            }
        };
        if first.is_none() {
            let mut rgba = vec![0; width as usize * height as usize * 4];
            video.picture(&mut rgba, width, height).ok()?;
            first = Some(rgba);
        }
        let shows = match start {
            None => time.unwrap_or(0),
            Some(before) => {
                let time = time.unwrap_or(before + u64::from(delay));
                if time > before {
                    delay = (time - before).min(10_000) as u32;
                }
                delays.push(delay);
                time.max(before)
            }
        };
        start = Some(shows);
        if shows >= ANIMATION_MS {
            break;
        }
    }
    start?;
    delays.push(delay);
    let rgba = first?;
    let animation = (delays.len() > 1).then(|| Animation::video(Arc::from(data), delays));
    Some(Picture {
        width,
        height,
        rgba,
        animation,
    })
}

/// A video animation's pictures from the first, at `width` by `height`, each
/// decoded as it's taken. One thread decodes them: they're small, and come
/// one at a time.
pub(crate) fn frames(
    file: Arc<[u8]>,
    width: u32,
    height: u32,
) -> Box<dyn Iterator<Item = Vec<u8>>> {
    let Some(mut video) = Video::open_with(std::io::Cursor::new(file), false, 1) else {
        return Box::new(std::iter::empty());
    };
    Box::new(std::iter::from_fn(move || {
        loop {
            match video.decode().ok()? {
                Next::Picture(_) => {
                    let mut rgba = vec![0; width as usize * height as usize * 4];
                    video.picture(&mut rgba, width, height).ok()?;
                    return Some(rgba);
                }
                Next::Sound(_) => {}
                Next::End => return None,
            }
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../testing/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn recognises_video_files() {
        assert!(is_video(b"\0\0\0\x20ftypisom"));
        assert!(is_video(&[0x1a, 0x45, 0xdf, 0xa3, 1]));
        assert!(!is_video(b"GIF89a"));
        assert!(!is_video(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn fits_without_enlarging() {
        assert_eq!(fit(498, 280, 360, 270), (360, 202));
        assert_eq!(fit(100, 50, 360, 270), (100, 50));
    }

    #[test]
    fn decodes_videos_into_animations() {
        // An animated AVIF, as vxtwitter makes of a GIF, is an AV1 video too.
        for name in [
            "gif-h264.mp4",
            "gif-vp9.webm",
            "gif-av1.mp4",
            "gif-animated.avif",
        ] {
            let data = fixture(name);
            let picture =
                crate::chat::decode_picture(&data, 32, 32).unwrap_or_else(|| panic!("{name}"));
            assert_eq!((picture.width, picture.height), (32, 18), "{name}");
            assert_eq!(
                picture.animation.as_ref().unwrap().delays,
                [100; 10],
                "{name}"
            );
            let frames: Vec<_> = picture.frames().collect();
            assert_eq!(frames.len(), 10, "{name}");
            assert_eq!(frames[0], picture.rgba, "{name}");
            // The test pattern moves, so the pictures differ.
            assert_ne!(frames[0], frames[5], "{name}");
            assert!(picture.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }
        assert!(decode_animation(b"\0\0\0\x20ftypisom not really", 32, 32).is_none());
    }

    /// Each picture's and sound's time, to the end, and how many samples
    /// of each channel the sound held.
    fn timeline(video: &mut Video) -> (Vec<u64>, Vec<u64>, usize) {
        let (mut pictures, mut sounds, mut samples) = (vec![], vec![], 0);
        let channels = video.sound.map_or(1, |(_, channels)| channels);
        loop {
            match video.decode().unwrap() {
                Next::Picture(time) => {
                    let mut rgba = vec![0; 16 * 9 * 4];
                    video.picture(&mut rgba, 16, 9).unwrap();
                    pictures.push(time.unwrap());
                }
                Next::Sound(time) => {
                    sounds.push(time.unwrap());
                    samples += video.sound().len() / channels;
                }
                Next::End => return (pictures, sounds, samples),
            }
        }
    }

    #[test]
    fn plays_pictures_and_sound_and_moves_back() {
        for (name, rate, channels) in [("video-aac.mp4", 44_100, 2), ("video-opus.webm", 48_000, 1)]
        {
            let data = fixture(name);
            let mut video = Video::open(std::io::Cursor::new(&data[..]), true).unwrap();
            assert_eq!((video.width, video.height), (160, 90), "{name}");
            assert_eq!(video.sound, Some((rate, channels)), "{name}");
            assert!(
                video
                    .duration_ms
                    .is_some_and(|ms| (1900..=2100).contains(&ms)),
                "{name}"
            );
            let (pictures, sounds, samples) = timeline(&mut video);
            assert_eq!(pictures.len(), 50, "{name}: 2 s at 25 fps");
            assert!(
                pictures.windows(2).all(|pair| pair[1] - pair[0] == 40),
                "{name}: in order"
            );
            assert!(sounds.windows(2).all(|pair| pair[0] < pair[1]), "{name}");
            let seconds = samples as f64 / f64::from(rate);
            assert!(
                (1.9..2.2).contains(&seconds),
                "{name}: {seconds} s of sound"
            );
            // Back to a second in, from its keyframe, the first.
            video.seek(1000).unwrap();
            let (again, _, _) = timeline(&mut video);
            assert_eq!(again.first(), Some(&0), "{name}: from the keyframe before");
            assert_eq!(again.last(), pictures.last(), "{name}");
        }
        // Without sound asked for, there's none.
        let data = fixture("video-aac.mp4");
        let mut silent = Video::open(std::io::Cursor::new(&data[..]), false).unwrap();
        assert_eq!(silent.sound, None);
        assert_eq!(timeline(&mut silent).1, Vec::<u64>::new());
    }
}
