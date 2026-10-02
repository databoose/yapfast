# Talk Windows link fix

This is webrtc-sys 0.3.35 from crates.io under its original Apache-2.0 license.
Only include/livekit/jsep.h differs: PeerContext is forward-declared as struct,
matching the generated cxx bridge. The mismatched class declaration produced
undefined observer constructors and Box<PeerContext>::drop on Windows MSVC.

This backports the one-line fix documented in LiveKit issue 1154:
https://github.com/livekit/rust-sdks/issues/1154

The Rust API, media code, dependency versions and libwebrtc archive stay pinned.
