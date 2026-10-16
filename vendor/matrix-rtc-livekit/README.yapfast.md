# Yapfast's native audio and join cancellation patch

Source: `element-hq/matrix-rust-rtc` at
`5831af899f1e1744c21332613cdc9578a98c604f`, crate `matrix-rtc-livekit`.
The upstream license files and source headers are retained. Workspace dependencies
in Cargo.toml are expanded to the same versions and git revisions; examples and
upstream test targets are omitted.

The local changes support native device tracks and cancellable joins:

- `LiveKitTransportConnection::reassert_local_key_index` becomes `pub(crate)`.
- `Call::publish_audio_track` publishes an existing LiveKit native audio track,
  then reapplies the connection's active sender key index, just as the existing
  managed publish method does.
- `Call::join_cancellable` retains the signalling manager while joining. On
  cancellation it drops connection work, removes any published Matrix membership,
  and waits for bounded cleanup before returning. The existing join API remains
  available.
- `Call::reassert_local_key_index` exposes the connection method above. Yapfast
  calls it on LiveKit's `LocalTrackRepublished` event.

Without this, unmuting for the first time after a key rotation publishes a cryptor
at index zero. A late joiner holds only the newer index and hears silence.
`scripts/test-voice` reproduces the failure by joining muted, waiting 30 seconds,
adding a second account, then publishing native microphone audio. Both directions,
mute, deafen and normal leave run in separate processes using virtual devices.
The same test holds an authorization request open, cancels joining, verifies that
Leave completes within five seconds and that the other account loses the pending
participant, then joins again successfully.

A full LiveKit reconnect also republishes local tracks with new frame cryptors
at index zero, so remote participants hear silence once the key has rotated.
`testing/test_voice_reliability.py` (republish scenario) rotates Alice's key,
interrupts Bob's network for 30 seconds, and checks that Alice still receives
Bob's tone after the reconnect.

`connect_e2ee` also requests LiveKit's single peer connection mode. With separate
publisher and subscriber connections, the subscriber reached `Connected` about one
second after the publisher on every join, which made up most of the join time.
`testing/join_voice.py` measures joins with one process per participant.

Keep the remaining MatrixRTC crates and Matrix SDK revision pinned. Remove this
copy when an upstream native publication API provides the same key-index handling and partial-join cleanup.
