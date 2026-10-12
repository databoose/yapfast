# Persistent voice channels

Talk can join one encrypted voice channel at a time, wait alone, and keep the
connection while browsing text channels, other servers and DMs. Each join starts
muted. The dock under the rail and sidebar exposes mute, deafen, audio devices and Leave.
A caret beside mute and beside deafen opens a menu that chooses the input or
output device and links to the full settings. Deafen
also mutes the microphone; undeafening keeps it muted, while unmuting when
deafened undeafens too. The voice view shows the
participants and their mute, deafen and speaking state; a deafened person shows
crossed-out microphone and headphones in the channel list, and only the red
crossed-out headphones on their tile in the call. MatrixRTC doesn't share
deafening, so in Talk calls only your own shows. Clicking a voice channel joins it.
Clicking another one during a call opens a dialog that asks before leaving the
current one. Coming back to a server that had a voice channel selected shows it
without joining.

The native LiveKit audio engine captures and plays audio through system devices.
The gear button opens a popover to select microphone and speakers,
without hiding the channel list in small windows. Talk remembers the choice in
`devices.json` in its data folder; it never leaves the machine. A remembered
device that is unplugged gives way to the system default until it returns.
Refresh devices after plugging in hardware. No microphone track is
created until Unmute. Leaving releases the audio handle, and restarting Talk
requires joining again. Preview calls are simulated, but the preview lists and
chooses real devices without remembering them.

WebRTC's PulseAudio devices report no GUID, and LiveKit selects devices only by
GUID, which always chose the first device. Talk names such devices `name:` and
their reported name, and selects them by their current index instead.

Right-clicking someone in a call, in the sidebar or on their tile, sets how
loud they sound from 0% to 200%, and their stream's sound too while they're
live. A drag snaps to 100% near it. Talk remembers both by Discord or Matrix
user id in `layout.json`, so they hold in every call and after a restart. Discord
voice scales each person as it mixes the call, and a watched stream's worker
scales its sound. Talk calls play remote audio inside WebRTC, which LiveKit's
Rust API can't adjust per track, so Talk's patch to `webrtc-sys` sets each
track's volume through WebRTC's `SetVolume` by track id.

Settings' "Even out your voice", on unless turned off, levels the microphone
after WebRTC's processing. Talk follows your usual speaking level over the last
half minute or so, and undoes up to 8 dB of drift from it as you lean towards
the microphone or away from it. Drifts of more than 12 dB, such as whispers and
shouts, pass through as they are, and a shout lets go of any correction at once.
Pauses hold the gain, and raising never pushes a peak past 90% of full scale.
Talk calls level inside LiveKit's WebRTC, through a capture hook in Talk's patch
to `webrtc-sys`. Discord voice levels in its own microphone thread; its voice
activation gate still opens on the level before leveling, so held gain can't
lift noise past it.

Talk calls suppress noise with WebRTC's own suppressor, inside LiveKit's
WebRTC. Discord calls use DeepFilterNet, which also takes out typing; see the
Microphone section of [Discord](discord.md#microphone). Talk calls could share
it through the capture hook, which now only returns a gain for each block.

The input menu's level meter reads the chosen microphone locally through
PulseAudio, raw and before any processing, only while the menu is open. It
works while muted, outside calls and in the preview, and it never touches the
call: muted, nothing is published or sent. It shows as "Input level" in the
system's recording streams. Windows has no meter yet.

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
Ringing DM/group calls, video, screenshare, push-to-talk, input and output
volume, and tray behavior are later slices.

## Reliability

Talk reports call problems instead of leaving them in the log:

- A participant whose device is unverified, unknown, or violates their verified
  identity is labelled in the voice view. Talk can't send them its audio key and
  refuses theirs, so the sidebar also shows a warning.
- A participant whose audio key hasn't arrived after ten seconds, or whose audio
  can't be decrypted, is labelled. Only a failure of Talk's own key mutes the
  microphone with an error.
- A participant who stops sending audio after having connected is shown as
  "Connection lost" until their membership ends.
- If Talk was killed during a call, the next start removes the sticky membership
  this device left, instead of others seeing it for up to five minutes.
- After a full LiveKit reconnect, Talk restores the microphone's key index, so
  others keep hearing it after a key rotation.
- Join failures name the cause users can act on: a homeserver without sticky
  events (MSC4354), a missing MatrixRTC transport, or a calling service that
  refused the connection.

`testing/test_voice_reliability.py` checks these with separate processes, the
local stack and private PulseAudio null devices; no physical microphone is
recorded. Bob runs in a pasta network namespace, so stopping pasta simulates a
network loss for Bob only. Run it from the development shell after
`scripts/test-stack up`:

```sh
python3 testing/test_voice_reliability.py --report .local/voice-reliability.json
```

The full run on 2026-10-03 ([report](measurements/2026-10-03-linux-voice-reliability.json)):

| Scenario | Result |
| --- | --- |
| Wait alone, then a late join | Passed after 600 s alone |
| 20 s network loss | Recovered 1.1 s after the network returned |
| 75 s network loss | Ended after 60 s with an error; rejoined 2.0 s after the network returned, with no stale entry |
| Full reconnect after a key rotation | Bob's tone still decoded (amplitude 107.7 before, 126.0 after) |
| Unverified participant | Labelled, with a warning; Alice's audio unaffected |
| Killed, then restarted | Stale membership removed 0.2 s after restart |
| Killed, not restarted | Shown as connection lost after 21 s; removed by expiry after 298.7 s |
| Homeserver without sticky events | Refused with the MSC4354 explanation (rerun, see below) |

The first full run failed only the last check: the test proxied a homeserver
that hides MSC4354, but Synapse's login response pointed the SDK back to the
real address. Talk now keeps the homeserver address the user entered, which
also stops a login response from moving Talk to an unchecked plain-HTTP
address. Rerun on its own after that fix, the scenario passed.
