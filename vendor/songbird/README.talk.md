# Talk DAVE receive fix

This is songbird 0.6.0 from crates.io under its original ISC license
(LICENSE.md, copied from the upstream repository).

Receiving audio in a DAVE call failed: every packet reached the Opus decoder
with DAVE's trailer still attached, and libopus rejected it as an invalid
packet. After decryption, `udp_rx` knows where the Opus frame starts and how
many bytes follow it, but it only passed those offsets to the RTP packet event.
The decoder recomputed them from the transport mode's fixed sizes, which don't
include the DAVE trailer.

Changes, marked `Talk:` in the source:

- `StoredPacket` carries the decrypted frame's offset and trailing length.
- `SsrcState::get_voice_tick` decodes decrypted packets with those offsets.
  Packets that weren't decrypted keep the upstream calculation.

Upstream pull request 314 targeted the same symptom with a larger rewrite and
was closed unmerged: https://github.com/serenity-rs/songbird/pull/314

# Talk local voice server

Talk's tests run Discord calls against testing/fake_discord.ts, which serves
its voice gateway over plain WebSocket on this machine. `generate_url` in
`driver/connection/mod.rs` accepts an endpoint written as `ws://host:port/path`
when the host is a loopback address or `localhost`; every other endpoint keeps
the upstream `wss://` form. Discord's endpoints never carry a scheme.
