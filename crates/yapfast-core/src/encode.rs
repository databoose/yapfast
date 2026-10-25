//! Encoding for Go Live and the camera, through native/vp8.c with the libvpx
//! inside libwebrtc, and native/h264.c with x264 (vendor/x264-src). Frames
//! arrive as RGBA or I420 and leave as VP8 frames or H264 access units in
//! Annex B form.

use std::ffi::{c_int, c_long, c_longlong, c_uchar, c_uint, c_ulong};

use anyhow::{Result, bail};
use livekit::webrtc::{
    native::yuv_helper,
    video_frame::{I420Buffer, VideoBuffer},
};
use yapfast_discord::rtp::Codec;
// x264's library, which native/h264.c calls: linked only while named.
use x264_src as _;

#[repr(C)]
struct Raw {
    _private: [u8; 0],
}

type Encode = unsafe extern "C" fn(
    *mut Raw,
    *const c_uchar,
    c_int,
    *const c_uchar,
    c_int,
    *const c_uchar,
    c_int,
    c_longlong,
    c_ulong,
    c_int,
    *mut c_uchar,
    usize,
    *mut c_int,
) -> c_long;
type SetBitrate = unsafe extern "C" fn(*mut Raw, c_uint) -> c_int;
type Free = unsafe extern "C" fn(*mut Raw);

unsafe extern "C" {
    fn yapfast_vp8_new(
        width: c_uint,
        height: c_uint,
        fps: c_uint,
        kbps: c_uint,
        threads: c_uint,
    ) -> *mut Raw;
    #[allow(clippy::too_many_arguments)]
    fn yapfast_vp8_encode(
        encoder: *mut Raw,
        y: *const c_uchar,
        y_stride: c_int,
        u: *const c_uchar,
        u_stride: c_int,
        v: *const c_uchar,
        v_stride: c_int,
        pts: c_longlong,
        duration: c_ulong,
        keyframe: c_int,
        out: *mut c_uchar,
        capacity: usize,
        is_key: *mut c_int,
    ) -> c_long;
    fn yapfast_vp8_set_bitrate(encoder: *mut Raw, kbps: c_uint) -> c_int;
    fn yapfast_vp8_free(encoder: *mut Raw);
    fn yapfast_h264_new(
        width: c_uint,
        height: c_uint,
        fps: c_uint,
        kbps: c_uint,
        threads: c_uint,
        effort: c_uint,
    ) -> *mut Raw;
    #[allow(clippy::too_many_arguments)]
    fn yapfast_h264_encode(
        encoder: *mut Raw,
        y: *const c_uchar,
        y_stride: c_int,
        u: *const c_uchar,
        u_stride: c_int,
        v: *const c_uchar,
        v_stride: c_int,
        pts: c_longlong,
        duration: c_ulong,
        keyframe: c_int,
        out: *mut c_uchar,
        capacity: usize,
        is_key: *mut c_int,
    ) -> c_long;
    fn yapfast_h264_set_bitrate(encoder: *mut Raw, kbps: c_uint) -> c_int;
    fn yapfast_h264_free(encoder: *mut Raw);
}

/// RTP's video clock.
pub const CLOCK: u64 = 90_000;

/// How much work the H264 encoder puts into each frame: x264's superfast
/// preset, or its faster one, for a sharper picture at about twice the CPU.
/// VP8 takes no effort.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    #[default]
    Light,
    High,
}

pub struct Encoder {
    raw: *mut Raw,
    codec: Codec,
    encode: Encode,
    set_bitrate: SetBitrate,
    free: Free,
    /// The bitrate aimed for, in kbit/s.
    kbps: u32,
    width: u32,
    height: u32,
    fps: u32,
    /// The frame as I420, for [`Self::encode`]: a full-size luma plane, then
    /// two quarter-size chroma planes. Empty until it's first needed: the
    /// screen and the camera arrive as I420.
    planes: Vec<u8>,
    out: Vec<u8>,
}

// SAFETY: neither library keeps thread-local state; one thread uses the encoder at a time.
unsafe impl Send for Encoder {}

/// One encoded frame.
pub struct Frame<'a> {
    pub data: &'a [u8],
    pub keyframe: bool,
}

impl Encoder {
    /// An encoder for `width` by `height` frames, which must be even.
    pub fn new(
        codec: Codec,
        width: u32,
        height: u32,
        fps: u32,
        kbps: u32,
        effort: Effort,
    ) -> Result<Self> {
        if width == 0 || height == 0 || !width.is_multiple_of(2) || !height.is_multiple_of(2) {
            bail!("{codec:?} frames need an even size, not {width}x{height}");
        }
        let threads =
            std::thread::available_parallelism().map_or(1, |cores| cores.get().min(4)) as u32;
        // SAFETY: plain values; null means the library refused the configuration.
        let (raw, encode, set_bitrate, free): (_, Encode, SetBitrate, Free) = match codec {
            Codec::Vp8 => (
                unsafe { yapfast_vp8_new(width, height, fps, kbps, threads) },
                yapfast_vp8_encode,
                yapfast_vp8_set_bitrate,
                yapfast_vp8_free,
            ),
            Codec::H264 => (
                unsafe { yapfast_h264_new(width, height, fps, kbps, threads, effort as c_uint) },
                yapfast_h264_encode,
                yapfast_h264_set_bitrate,
                yapfast_h264_free,
            ),
        };
        if raw.is_null() {
            bail!("could not start the {codec:?} encoder");
        }
        let pixels = (width * height) as usize;
        Ok(Self {
            raw,
            codec,
            encode,
            set_bitrate,
            free,
            kbps,
            width,
            height,
            fps,
            planes: Vec::new(),
            // Even a keyframe of noise fits.
            out: vec![0; pixels * 2 + 4096],
        })
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// The bitrate aimed for, in kbit/s.
    pub fn bitrate(&self) -> u32 {
        self.kbps
    }

    /// Aims for `kbps` from the next frame on.
    pub fn set_bitrate(&mut self, kbps: u32) -> Result<()> {
        // SAFETY: raw came from this codec's constructor and is still live.
        if unsafe { (self.set_bitrate)(self.raw, kbps) } != 0 {
            bail!("the {:?} encoder refused {kbps} kbit/s", self.codec);
        }
        self.kbps = kbps;
        Ok(())
    }

    /// Encodes RGBA pixels of the encoder's size, shown at `pts` in
    /// [`CLOCK`] units. None when the encoder skipped the frame.
    pub fn encode(&mut self, rgba: &[u8], pts: u64, keyframe: bool) -> Result<Option<Frame<'_>>> {
        let (width, height) = (self.width, self.height);
        let pixels = (width * height) as usize;
        if rgba.len() < pixels * 4 {
            bail!("a frame smaller than the encoder's");
        }
        let mut planes = std::mem::take(&mut self.planes);
        planes.resize(pixels + pixels / 2, 0);
        let (y, chroma) = planes.split_at_mut(pixels);
        let (u, v) = chroma.split_at_mut(pixels / 4);
        // libyuv names formats by their order in a little-endian word: its
        // ABGR is R, G, B, A in memory.
        yuv_helper::abgr_to_i420(
            rgba,
            width * 4,
            y,
            width,
            u,
            width / 2,
            v,
            width / 2,
            width as i32,
            height as i32,
        );
        let encoded =
            self.encode_planes([(y, width), (u, width / 2), (v, width / 2)], pts, keyframe);
        self.planes = planes;
        self.frame(encoded?)
    }

    /// Encodes an I420 picture of the encoder's size, like [`Self::encode`].
    pub fn encode_i420(
        &mut self,
        picture: &I420Buffer,
        pts: u64,
        keyframe: bool,
    ) -> Result<Option<Frame<'_>>> {
        if (picture.width(), picture.height()) != (self.width, self.height) {
            bail!("a picture of another size than the encoder's");
        }
        let (y, u, v) = picture.data();
        let (y_stride, u_stride, v_stride) = picture.strides();
        let encoded =
            self.encode_planes([(y, y_stride), (u, u_stride), (v, v_stride)], pts, keyframe);
        self.frame(encoded?)
    }

    /// Encodes a camera's picture, at least the encoder's size: its top left
    /// corner, when it's larger, as a picture an odd size loses a row or a
    /// column.
    pub fn encode_picture(
        &mut self,
        picture: &crate::picture::I420<'_>,
        pts: u64,
        keyframe: bool,
    ) -> Result<Option<Frame<'_>>> {
        if picture.width < self.width || picture.height < self.height {
            bail!("a picture smaller than the encoder's");
        }
        let [y_stride, u_stride, v_stride] = picture.strides;
        let encoded = self.encode_planes(
            [
                (picture.y, y_stride),
                (picture.u, u_stride),
                (picture.v, v_stride),
            ],
            pts,
            keyframe,
        );
        self.frame(encoded?)
    }

    /// Encodes three planes, each with its stride, into `out`: the encoded
    /// size and whether it's a keyframe, or None when the frame was skipped.
    fn encode_planes(
        &mut self,
        [(y, y_stride), (u, u_stride), (v, v_stride)]: [(&[u8], u32); 3],
        pts: u64,
        keyframe: bool,
    ) -> Result<Option<(usize, bool)>> {
        let mut is_key: c_int = 0;
        // SAFETY: the planes hold a picture of this encoder's size at these
        // strides, and the output buffer is sized for its frames; all stay
        // put during the call.
        let written = unsafe {
            (self.encode)(
                self.raw,
                y.as_ptr(),
                y_stride as c_int,
                u.as_ptr(),
                u_stride as c_int,
                v.as_ptr(),
                v_stride as c_int,
                pts as c_longlong,
                (CLOCK / u64::from(self.fps)) as c_ulong,
                c_int::from(keyframe),
                self.out.as_mut_ptr(),
                self.out.len(),
                &mut is_key,
            )
        };
        match written {
            ..0 => bail!("{:?} encoding failed ({written})", self.codec),
            0 => Ok(None),
            _ => Ok(Some((written as usize, is_key != 0))),
        }
    }

    fn frame(&self, encoded: Option<(usize, bool)>) -> Result<Option<Frame<'_>>> {
        Ok(encoded.map(|(size, keyframe)| Frame {
            data: &self.out[..size],
            keyframe,
        }))
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: raw came from this codec's constructor and is freed once.
        unsafe { (self.free)(self.raw) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Decoder;

    /// Four frames of moving colour, the third a forced keyframe.
    fn encode_four(codec: Codec, effort: Effort) -> Vec<(bool, Vec<u8>)> {
        let (width, height) = (64, 36);
        let mut encoder = Encoder::new(codec, width, height, 30, 500, effort).unwrap();
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let mut frames = vec![];
        for frame in 0..4u64 {
            for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
                pixel.copy_from_slice(&[(index as u64 + frame * 8) as u8, 90, 200, 255]);
            }
            if frame == 1 {
                encoder.set_bitrate(250).unwrap();
                assert_eq!(encoder.bitrate(), 250);
            }
            let encoded = encoder
                .encode(&rgba, frame * 3000, frame == 2)
                .unwrap()
                .unwrap();
            frames.push((encoded.keyframe, encoded.data.to_vec()));
        }
        assert!(Encoder::new(codec, 63, 36, 30, 500, effort).is_err());
        frames
    }

    #[test]
    fn encodes_vp8_keyframes_and_deltas() {
        let frames = encode_four(Codec::Vp8, Effort::Light);
        for (keyframe, data) in &frames {
            // A keyframe's tag has bit 0 clear, and its size follows the start code.
            assert_eq!(*keyframe, data[0] & 1 == 0);
            if *keyframe {
                assert_eq!(&data[3..6], &[0x9d, 0x01, 0x2a]);
                let size = |at: usize| u16::from_le_bytes([data[at], data[at + 1]]) & 0x3fff;
                assert_eq!((size(6), size(8)), (64, 36));
            }
        }
        let keys: Vec<bool> = frames.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys[..3], [true, false, true]);
    }

    #[test]
    fn encodes_main_profile_h264_that_decodes() {
        for effort in [Effort::Light, Effort::High] {
            let frames = encode_four(Codec::H264, effort);
            let keys: Vec<bool> = frames.iter().map(|(key, _)| *key).collect();
            assert_eq!(keys[..3], [true, false, true]);
            let units = |data| yapfast_discord::rtp::nal_units(data);
            let types =
                |data| -> Vec<u8> { units(data).iter().map(|unit| unit[0] & 0x1f).collect() };
            let first = types(&frames[0].1);
            assert!(
                first.starts_with(&[7, 8]) && first.contains(&5),
                "parameter sets, then an IDR slice: {first:?}"
            );
            // The sequence parameter set's profile_idc: Main, as Discord's
            // client sends.
            assert_eq!(units(&frames[0].1)[0][1], 77);
            assert!(!types(&frames[1].1).contains(&5), "a delta frame");
            let mut decoder = Decoder::new(Codec::H264).unwrap();
            let mut shown = 0;
            for (_, data) in &frames {
                if let Some(picture) = decoder.decode(data).unwrap() {
                    assert_eq!((picture.width, picture.height), (64, 36));
                    shown += 1;
                }
            }
            assert!(shown >= 3, "{shown} pictures at {effort:?} effort");
        }
    }
}
