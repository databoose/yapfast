// Talk: runs the application's code on each call's captured audio after
// WebRTC's own processing, through the audio processing builder's capture
// post-processing. See README.talk.md.

use std::sync::Mutex;

#[cxx::bridge(namespace = "livekit_ffi")]
pub mod ffi {
    /// The linear gains a block starts and ends at, ramped between.
    struct CaptureRamp {
        from: f32,
        to: f32,
    }

    extern "Rust" {
        type CaptureHook;

        fn new_capture_hook() -> Box<CaptureHook>;
        fn ramp(self: &mut CaptureHook, samples: &[f32], sample_rate: i32) -> CaptureRamp;
    }
}

/// Sets the gain of captured audio, block by block.
pub trait CaptureProcessor: Send {
    /// The linear gains a block of the first channel, in 16-bit range, should
    /// start and end at. Every channel ramps between them.
    fn ramp(&mut self, samples: &[f32], sample_rate: u32) -> (f32, f32);
}

type Make = Box<dyn Fn() -> Box<dyn CaptureProcessor> + Send + Sync>;

static MAKE: Mutex<Option<Make>> = Mutex::new(None);

/// Makes a processor for each audio processing module, from now on, including
/// those already made that haven't had a processor yet.
pub fn set_capture_processor(make: impl Fn() -> Box<dyn CaptureProcessor> + Send + Sync + 'static) {
    *MAKE.lock().unwrap() = Some(Box::new(make));
}

pub struct CaptureHook {
    processor: Option<Box<dyn CaptureProcessor>>,
}

fn new_capture_hook() -> Box<CaptureHook> {
    Box::new(CaptureHook { processor: None })
}

impl CaptureHook {
    fn ramp(&mut self, samples: &[f32], sample_rate: i32) -> ffi::CaptureRamp {
        if self.processor.is_none() {
            self.processor = MAKE.lock().unwrap().as_ref().map(|make| make());
        }
        let (from, to) = match (self.processor.as_mut(), u32::try_from(sample_rate)) {
            (Some(processor), Ok(rate)) => processor.ramp(samples, rate),
            _ => (1.0, 1.0),
        };
        ffi::CaptureRamp { from, to }
    }
}
