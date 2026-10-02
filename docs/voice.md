# Persistent voice channels

Talk can join one encrypted voice channel at a time, wait alone, and keep the
connection while browsing text channels, other servers and DMs. Each join starts
muted. The sidebar footer exposes mute, deafen, audio devices and Leave. Deafen
also mutes the microphone; undeafening keeps it muted. The voice view shows the
participants and their mute/speaking state. Switching voice channels asks before
leaving the current one.

The native LiveKit audio engine captures and plays audio through system devices.
The gear button opens a popover to select microphone and speakers by device ID,
without hiding the channel list in small windows. Choices last while
Talk is open. Refresh devices after plugging in hardware. No microphone track is
created until Unmute. Leaving releases the audio handle, and restarting Talk
requires joining again. Preview controls only simulate these states.

## Run the local demo

For testing with a friend on another machine, use
[the shared beta deployment](../deploy/coolify/README.md) at `matrix.org`.
It includes **Talk beta**, encrypted **general** and the persistent **Lounge**
voice channel.

From the project folder, enter the development shell and start the services:

```sh
nix develop path:./nix
scripts/test-stack up
cargo run --locked -- --voice-demo
```

The demo creates two disposable cross-signed accounts, a Matrix Space, an encrypted
text channel and an encrypted voice channel. It prints two commands with separate
data directories. Run each in a separate terminal. Select **Talk voice test** in
the server rail, then **Lounge → Join voice**. Both accounts join muted. Select
devices using the gear button, then unmute one account. Use separate machines
or headphones for a conversation; two windows on one computer share its audio.

List native devices without publishing a microphone:

```sh
cargo run --locked -- --audio-devices
```

Normal accounts require password sign-in, a verified device, an encrypted joined
room of type `org.matrix.msc3417.call`, an `m.rtc.slot` at `m.call#ROOM` with
`m.per_member` encryption, a homeserver supporting the pinned SDK's sticky events,
and a reachable MatrixRTC LiveKit service. Talk refuses missing or unencrypted
slots. Room creation is currently provided by the demo or another client.
There is no fallback to unencrypted media.

The local stack has an automatic authorization fallback at localhost:16080.
Other homeservers use SDK discovery. To configure a fallback in fish:

```fish
set -x TALK_RTC_AUTHORIZATION_URL https://your-calling-service.example
cargo run --locked
```

The URL requires HTTPS except for loopback development addresses. A remote tester
needs a TLS deployment with reachable LiveKit media ports; the local Compose
stack binds to loopback. This slice does not establish a production SFU admission
policy.

## Validation and remaining checks

With the stack running, `scripts/test-voice` exercises encrypted native audio
using isolated Linux virtual devices and a generated signal. It does not record
the physical microphone or change the system's default devices. The [Linux test report](measurements/2026-10-02-linux-voice-audio.json) records
a 30-second wait alone, a muted late join, native encrypted audio in both
directions, mute, deafen, undeafen staying muted and clean leave. It also stalls
authorization after membership publication, cancels with Leave, checks membership
removal and then joins again. The two clients
run in separate processes because LiveKit shares its audio engine within a
process. A regression exposed stale key indices on native tracks first published
after rotation; the [pinned dependency patch](../vendor/matrix-rtc-livekit/README.talk.md)
reuses the SDK's active-index handling. The original
[encrypted audio experiment](native-call-experiment.md) separately covers recovery,
invalid keys and departure key rotation.

Linux UI captures cover the preview and real accounts at 1120×760 and 800×560,
including persistent controls, device selection and channel switching. Closing
one native window removed its participant from the other account. A separate
[waiting-alone debug sample](measurements/2026-10-02-linux-voice-waiting-debug.json)
records resources; release performance remains unverified.

On 2026-10-02 the user confirmed audio in both directions between Linux and the
Windows x64 build on the shared beta, along with mute/deafen and browsing text
channels while voice stays connected. This is a manual user report; exact Windows,
GPU/driver and audio-device models remain unrecorded. See [the Windows report](windows-beta.md).

Continue manual checks with the [Windows package](windows-beta.md) and two verified
accounts in the same encrypted channel:

1. Join alone and wait at least ten minutes. Join from the other machine; both
   users should appear, initially muted.
2. Talk in both directions. Mute and deafen, then undeafen; the microphone should
   stay muted. Change both devices and confirm sound follows the selection.
3. Browse text channels and DMs during the call. Return to voice and leave;
   check that the other participant disappears.
4. Interrupt and restore the connection. Record whether voice resumes, or shows
   a useful failure and permits a fresh join. Close Talk during a call and check
   departure and the microphone indicator.
5. Record OS, GPU, device models, resource measurements and any audible glitches.

The controller shows reconnecting and allows up to 60 seconds before ending a
degraded connection. A joining request has a 35-second deadline. Leave cancels a pending join
immediately and waits up to ten seconds for membership cleanup; normal leaving
has the same cleanup deadline. Crash membership expiry uses a five-minute sticky lifetime.
Extended waiting, forced crashes, network recovery and outbound media-key
failure reporting still need evidence. Ringing DM/group calls, video, screenshare,
push-to-talk, device preference persistence and tray behavior are later slices.
