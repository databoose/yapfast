# Talk patches

This is webrtc-sys 0.3.35 from crates.io under its original Apache-2.0 license.
Three changes differ from it.

## Windows link fix

include/livekit/jsep.h differs: PeerContext is forward-declared as struct,
matching the generated cxx bridge. The mismatched class declaration produced
undefined observer constructors and Box<PeerContext>::drop on Windows MSVC.

This backports the one-line fix documented in LiveKit issue 1154:
https://github.com/livekit/rust-sdks/issues/1154

## Capture leveling

src/capture.rs and the `CaptureLeveling` class in
src/peer_connection_factory.cpp hand every call's captured audio, after WebRTC's
processing, to a processor the application sets with
`capture::set_capture_processor`. Talk levels the voice there. build.rs lists
the new bridge.

## Remote track volume

LiveKit's Rust API can't set how loud a remote track plays. src/audio_track.cpp
keeps each audio track wrapper in a registry by track id, and the new
`set_audio_track_volume` bridge function passes a volume to the matching
tracks' sources on the signaling thread, as WebRTC's `SetVolume` expects.

Otherwise the Rust API, media code, dependency versions and libwebrtc archive
stay pinned.
