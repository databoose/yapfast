# Discord in Talk

Status on 2026-10-07: steps 1 to 4 of the [order of work](#order-of-work) work
on Linux, and step 5, mirroring into Matrix, hasn't started. Decisions were
recorded on 2026-10-06.

| Area | Works on Linux | Open |
| --- | --- | --- |
| [Linking](#account-linking) | QR code from Settings, or the sign-in of a build without Matrix; unlinking | Captcha fallback |
| [Voice](#voice) | Server voice channels, mute, deafen, per-person volume, voice gate with its sensitivity, [DeepFilterNet noise suppression](#microphone) and its strength, taking a room's echo out | A check of Windows audio on real Windows; Discord's push-to-talk; soundboard |
| [DM calls](#dm-calls) | Starting, ringing, answering and declining DM and group DM calls | A check by hand through real Discord |
| [Go Live](#screen-sharing) | Sharing a screen with its sound, watching streams | A check on real Windows; camera video, after the beta |
| [Channels and messages](#servers-channels-and-messages) | History, live updates, sending, replies, edits, reactions, files, embeds, custom emoji, stickers, spoilers, unread state, notifications and their settings, member lists, pins and search | Threads, forums, pinning, forwarding, the menus of servers and channels |
| [DMs](#dms) | DMs, group DMs, message requests and spam, starting a DM, [a DM's menu](#a-dms-menu): pinning, muting and closing | Leaving a group DM |
| [Friends](#friends-and-requests) and [profiles](#profiles) | Friends list, friend requests, adding by username, profile cards and the DM panel, custom status, notes, friend nicknames, removing friends, ignoring and blocking | A list of the blocked and ignored |
| [Mirroring](#mirroring-into-matrix) | | Everything |

The [later list](#later) has the rest.

Talk connects to Discord directly, so a person who links their Discord account
doesn't need the Discord desktop app. Talk joins Discord voice as that person,
shows their servers, channels and DMs, and sends and receives messages. This is
part of the private beta. Talk must still be complete without Discord: nothing
Discord-related appears until someone links an account, and Discord work doesn't
replace beta features of Talk itself.

[PRODUCT.md](../PRODUCT.md#discord) records the agreed scope. This document
describes how to build it.

Protocol references: Discord doesn't document what its own client sends. The
community's
[discord-userdoccers](https://github.com/discord-userdoccers/discord-userdoccers)
does, for example in `pages/gateway/gateway-events.mdx`,
`pages/topics/read-state.mdx`, `pages/resources/relationships.mdx`,
`pages/remote-authentication/desktop.mdx` and `pages/topics/cloud-uploads.mdx`.
The settings protobuf is in
[discord-protos](https://github.com/discord-userdoccers/discord-protos).

## Risk to accounts

Discord's terms don't allow third-party clients to use personal accounts.
Bans are uncommon, but they happen, and Discord can change its protocol and
break Talk at any time. Linking is opt-in per person. Talk shows this warning
before linking, and each person decides for their own account.

Behave like the official client to keep the risk low. Respect rate limits,
never act without a user action behind it, and don't poll. Keep all Discord code
in one crate, so a protocol change breaks only that part and Talk can disable it
without affecting Matrix.

### Staying within Discord's limits

Talk follows the rules Discord documents for every client:

- **HTTP rate limits.** Each request's route maps to the bucket Discord names in
  `X-RateLimit-Bucket`. Discord counts a bucket apart for each major ID, the
  channel, server or webhook, so Talk keys its count by both: sending in one
  channel doesn't hold back another. Talk waits for an empty bucket to reset
  before it asks again. It also waits after a global 429, marked so in the
  headers or the body. If Discord still answers 429, Talk waits the body's
  `retry_after`, or `Retry-After`, or a second when Discord names neither, and
  tries once more. A wait longer than a minute fails at once, with a message to
  try again later.
- **Invalid requests.** Discord bans an IP that sends 10,000 requests in a
  10-minute window answered 401, 403 or 429. Talk doesn't ask for what it knows
  will be refused: a channel the account's roles hide shows a notice instead
  of fetching messages.
- **Gateway resume.** When the connection drops, Talk resumes the session at
  READY's `resume_gateway_url` with op 6 and the last sequence number, and
  Discord replays what was missed. Talk also resumes after op 7, after op 9
  with `d: true`, and when a heartbeat goes unacknowledged. It identifies again
  only when Discord won't resume: op 9 with `d: false`, close codes 4007 and
  4009, or three failed attempts. After op 9 it waits a random one to five
  seconds first, or longer when sessions keep ending. Opening a connection,
  Discord's HELLO and each send give up after 10 seconds, so a dead connection
  fails rather than hangs. Talk drops a connection without a close frame,
  because a close with 1000 or 1001 ends the session.
- **Gateway send limit.** A connection may send 120 commands a minute and is
  closed with 4008 past that. Talk holds commands back at 110, so heartbeats
  always fit. The connection runs in a task of its own, so heartbeats go on
  time while commands wait.
- **Backoff.** Reconnects back off from 2 to 60 seconds, and start over only
  once a session has stayed up for a minute. Each wait is a random 50% to 100%
  of the step, so clients cut off together don't return together. Talk sends
  at most 10 IDENTIFYs an hour: an account that keeps identifying gets flagged,
  so past that Talk stops reconnecting and says so until it restarts. The first
  heartbeat comes after a random part of the interval, as Discord asks.
- **Typing.** Mention search waits 300 ms after the last key before it asks
  Discord for members, so typing a name sends one search, not one per letter.
- **Cookies.** Talk keeps the cookies Discord's servers set and sends them
  back where they apply, as any HTTP client with a cookie jar does. These are
  Discord's device cookies and Cloudflare's rate limiting and bot management
  cookies. One jar serves the whole process. Persistent cookies are saved,
  readable only by the user, in `discord-cookies.json` in the data directory,
  so Talk stays the same device across restarts. Talk can't pass a Cloudflare
  challenge or captcha, so it never has a `cf_clearance` cookie.
- **Downloads.** Pictures and files from Discord's CDN download eight at a
  time, over one HTTP/2 connection, as a browser's requests to one host go.
  Opening the emoji picker queues its pictures in the picker's order instead
  of asking for hundreds at once.

## Why not an existing bridge

[mautrix-discord](https://github.com/mautrix/discord) bridges messages through
a service hosted next to Synapse. It has no voice, and voice matters most here.
It also adds a Matrix user for every Discord contact, so the homeserver learns
who each person talks to. Beeper uses the same bridge and also has no Discord
calls.

A Discord bot relaying voice into a Matrix room, like
[matrix-discord-voice-bridge](https://github.com/lukacsi/matrix-discord-voice-bridge),
follows Discord's rules. But Discord users would hear every Talk user as one bot,
it only works in servers that install the bot, and bots can't join DM calls.

## Account linking

Link by QR code, the method mautrix-discord uses. Talk shows a code from
Discord's remote login service, and the person scans it with the Discord phone
app. Pasting a token and email/password login aren't offered. A build with
Matrix links from Settings, under Discord; one without Matrix links on its
sign-in screen, and signing out there unlinks. Either way, Talk first says
that Discord doesn't allow third-party clients, so linking puts the account at
risk, and shows the code only when asked.

The remote login gateway (`wss://remote-auth-gateway.discord.gg/?v=2`, with
Discord's web origin) works as Discord's own login page does,
`talk_discord::remote_auth`:

1. Talk makes a 2048-bit RSA key and sends its public half, as base64 DER.
2. Discord sends a nonce encrypted to it (RSA-OAEP with SHA-256), and Talk
   answers with the SHA-256 of the nonce, base64url, which proves it holds the
   key.
3. Discord sends a fingerprint, and the code carries
   `https://discord.com/ra/{fingerprint}`.
4. Scanning it sends who scanned (`id:discriminator:avatar:username`, encrypted
   to the key), and Talk shows "Confirm on your phone".
5. Confirming sends a ticket, which `POST /users/@me/remote-auth/login` trades
   for the login, encrypted to the key again.

The gateway closes the connection about two minutes after the code shows, and
Talk connects again for a new code while the code is on screen. Saying no on
the phone stops linking, with "Try again". The key lives only in memory for the
one attempt. The `rsa` crate carries the Marvin timing advisory
(RUSTSEC-2023-0071), which needs an attacker to time many decryptions; Talk
decrypts three messages from Discord per attempt. Linking connects Discord at
once, without restarting: voice reaches it through a slot the chat backend
fills. When Discord can't start then, or as Talk starts, such as when the
credential store can't be read, Talk says why. What waits on Discord, such as
a friend request, the custom status or a profile, fails with that reason
rather than waiting, as it does when the chat backend stops. Discord may
still ask for a captcha when trading the ticket, which Talk
reports as an error for now; a fallback through a small embedded web view is
still open.

Store the Discord token in the OS credential store, next to the Matrix session.
The token gives full control of the account, so it never goes into Matrix, not
even encrypted. A device that links appears in Discord as its own session.
Devices that only use the mirror don't need to link.

Unlinking a device, from Settings or by signing out of a build without Matrix,
leaves any Discord call, ends the session with `POST /auth/logout`, and deletes
its token. Removing
the mirrored rooms is a separate step, described under [Unlinking](#unlinking).

## Voice

Scope for the beta:

- Server voice channels: join, leave, a participant list, speaking indicators,
  mute, deafen and per-person volume.
- DM and group DM calls, both incoming with ringing and outgoing.
- One connection at a time across Talk and Discord. Joining Discord voice asks
  to leave a Talk call, and the other way round, as in PRODUCT.md.
- The same devices, push-to-talk, voice activation, echo cancellation and
  voice leveling as Talk calls. Noise suppression is DeepFilterNet, or
  RNNoise, rather than WebRTC's, as described under Microphone below.

Clicking a voice channel joins through the same voice dock and one-call rule
as Matrix voice. Calls start muted, mute and deafen show in Discord, and a
voice gate sends audio only during speech. The owner and a friend talked
through it on 2026-10-06. Audio goes through `audio_io.rs`, PulseAudio on
Linux and WASAPI on Windows, and libwebrtc's processing module; see the
[Windows guide](windows-beta.md#discord-voice-on-windows) for what's been
checked there. songbird's DAVE receive fix is
described in `vendor/songbird/README.talk.md`, and davey's change for Go Live
groups in `vendor/davey/README.talk.md`.

Discord may move a call to another voice server, saying so with a new
`VOICE_SERVER_UPDATE` while the call is connected. Talk connects the call
there and it goes on. The old server may drop the connection before that
word comes, so a dropped connection waits five seconds for it before the
call ends. A stream moves the same way, with a new `STREAM_CREATE` and
`STREAM_SERVER_UPDATE`: sharing sends to the new server, after waiting up to
five seconds for it when the old connection ends, and watching connects again.

The group uses screen sharing often, so sharing a screen and watching
streams (Go Live) came before the beta; see [Screen sharing](#screen-sharing).
Camera video comes after the beta, together with video in Talk.

Since March 2026, Discord voice requires its end-to-end encryption, DAVE, and
[rejects clients without it](https://support.discord.com/hc/en-us/articles/38749827197591-A-V-E2EE-Enforcement-for-Non-Stage-Voice-Calls).
[davey](https://github.com/Snazzah/davey) (MIT) implements DAVE in Rust.
[songbird](https://github.com/serenity-rs/songbird) (ISC) has a voice connection
built on it, but it's written for bots and may only partly fit.

Talk's current audio path runs inside LiveKit's libwebrtc. Discord voice needs
its own Opus encoding and decoding plus the same audio processing. libwebrtc's
device module can't be reused outside a LiveKit room: it hands captured audio
straight to WebRTC and only plays remote WebRTC streams. Its audio processing
module can, through `livekit::webrtc::native::apm::AudioProcessingModule`. So
Discord voice gets a small path of its own: `audio_io.rs` for the microphone and
speakers at 48 kHz, the processing module for echo cancellation and gain
(noise suppression became DeepFilterNet; see [Microphone](#microphone)), then
songbird for Opus and DAVE. Feed the speaker mix to
the processing module's reverse stream so echo cancellation has its reference.

songbird encodes with `opus2`, which builds its own libopus, while Talk's
libwebrtc archive already contains libopus. On Linux both link, and the linker
takes each symbol from the first. Under MSVC they'd clash, so the Windows build
gives `opus2` an empty library and it uses libwebrtc's; see the
[Windows guide](windows-beta.md#build-validation-on-2026-10-13).

### DM calls

A DM or group DM has a call, as in Discord. The phone in its header starts
one, or joins the one going on, through the same voice dock and one-call
rule as a voice channel. Joining sends op 4 without a server, naming the DM
as the channel; VOICE_STATE_UPDATE and VOICE_SERVER_UPDATE come back without
a server too, the latter with the channel's id, and the voice connection
identifies with the DM's id where a server's call gives the server's. Talk
keeps those voice states under the server `@me`, as DM room ids do.

Discord sends CALL_CREATE, CALL_UPDATE and CALL_DELETE for the DMs' calls,
including one for each call going on when Talk connects (the
AUTO_CALL_CONNECT capability Talk already asks for). Each names whom the call
rings. Starting a call, rather than joining one with someone in it, rings the
others with `POST /channels/{id}/call/ring` and no recipients, once Discord
has reported the call: it rings only in a call it knows. Calling and ringing
are allowed separately. In a DM, Talk rings a friend at once and anyone else
only when `GET /channels/{id}/call` says they're `ringable`, as discord.py-self
does; otherwise, or when Discord refuses, the call says so until someone
comes, and the other person sees the call only in the DM. Talk plays a soft
ringback while nobody has come. People
being rung show in the call faded, "Calling...", until they answer or Discord
stops ringing them.

A call ringing this account shows a card at the top of the window with the
DM's picture, Decline and Accept, and Talk rings: a rising arpeggio
synthesized in `ring.rs` and played through `audio_io.rs` on the call's speakers
until the call is answered, declined or stops ringing. While Talk isn't in
front a desktop notification says who is calling. On Do Not Disturb the card
shows without the ring or the notification. Accepting opens the DM and
joins, leaving any other call without asking, since answering already chose.
Declining posts `POST /channels/{id}/call/stop-ringing` with this account,
which leaves the call to the others. The DM's own call shows the same choice
above its messages when it's open.

While this account is in a DM's call, the call's stage shows above the DM's
messages, taking less than half the conversation, more while watching a
stream. Without it, a DM with a call going on shows who is in it and a button
to join, and its row in the DM list a green phone. Discord's call messages
(type 3) read "started a call.", and once Discord adds when it ended, "started
a call that lasted 5 minutes.". Go Live works in DM calls too: op 18 with the
type `call` and no server, and the stream key `call:{channel}:{user}`. The
fake server checks all of this and answers for Bob, who picks up after three
seconds when Alice rings him; in a group nobody answers, and Dave, who isn't
her friend, can't be rung. Message requests
have no call until accepted. A DM call through real Discord, and Go Live in
one, haven't been checked by hand yet.

### Microphone

Discord's own client suppresses noise with Krisp, a neural network, and
WebRTC's suppressor sounded different from it. So Talk's Discord microphone
runs, in 10 ms blocks: WebRTC's echo cancellation and high-pass filter; if
asked, taking the room's echo out; a noise suppressor, which also tells the
gate which blocks are speech; WebRTC's gain control on what's left; then
Talk's leveling.

The suppressor is DeepFilterNet 3, through libDF, its Rust implementation on
tract (crate `deep_filter`; the code and the model's weights are MIT or
Apache-2.0). crates.io has only libDF 0.2.5, from before DeepFilterNet 3, so
Talk takes the repository's last commit, `d375b2d`, and its `default-model`
feature builds the 8 MB model into Talk. It runs both of its stages on every
block, as libDF's command line does. The library's defaults skip deep
filtering above 20 dB SNR, which dulled clean speech (wideband PESQ 4.53 fell
to 3.91) and took up to 5 dB off speech over a fan.

RNNoise, through `nnnoiseless` (BSD-3-Clause), was the suppressor until
2026-10-12. On real calls it let keyboard clicks through, and they held the
gate open. Settings → Voice & audio → "Noise suppression in Discord calls"
still chooses it, to compare the two, as does Off, for a quiet room: RNNoise
then only tells the gate which blocks are speech. Talk keeps the choice in
`layout.json` as `noise_suppression`. A call going on switches at its next
block, after DeepFilterNet's 0.2 s load if switching to it.

"Noise suppression strength" limits how far noise is turned down: Full takes
out all it can, Medium at most 30 dB and Light 20 dB (`noise_strength`). libDF
mixes the microphone back into its output spectrum to keep to the limit;
RNNoise's output gets the same mix of the input it lines up with, a block
earlier. Below 20 dB, typing that DeepFilterNet left got past the gate's level
after gain control, so there's no lighter step. PESQ falls as the limit does,
since it counts what's left of the noise against clean speech:

| DeepFilterNet's limit | Full | 30 dB | 20 dB | 12 dB |
| --- | --- | --- | --- | --- |
| PESQ, typing | 3.05 | 2.81 | 2.62 | 2.30 |
| PESQ, fan | 3.37 | 3.33 | 3.21 | 2.97 |
| PESQ, clean speech | 4.53 | 4.54 | 4.56 | 4.59 |
| Typing left in pauses, mean and loudest block | -79, -53 dBFS | -67, -52 | -58, -45 | -50, -37 |
| Pauses sent with typing | 6.3-6.4% | 6.3-6.4% | 6.5-11% | 81-86% |
| Times typing opened the gate in pauses, each mix | 0 | 0 | 0-2 | 17-19 |

The voice gate opens on a block the suppressor takes for speech that's louder
than the input sensitivity, -60 dBFS unless changed, or on any block 18 dB
louder still, and holds 400 ms. Levels are RMS over the block, after gain
control. "Input sensitivity in Discord calls" sets it from -80 to 0 dBFS
(`input_sensitivity`); behind its handle, the microphone's level goes from
green through orange at -18 dBFS to red at -6, with whether the gate is open.
While the setting shows, Talk runs the chosen microphone through a call's
processing on a thread of its own, `talk-gate-meter`, and sends nothing; in a
call that's a second copy of the processing. DeepFilterNet takes a block
for speech when its own estimate of the block's SNR is at least 5 dB, RNNoise
when its speech probability is at least 0.6. In a simulation of the gate on
the mixes below, a 0 dB threshold let typing open it in pauses, and 10 dB
missed quiet speech. RNNoise's probability beside DeepFilterNet's output did
as well as 5 dB (99.4% of speech sent rather than 99.5%, no opening in pauses
rather than one), so Talk doesn't run both. The
gate keeps the last 100 ms while shut and sends them when it opens, so a soft
start such as an "h" isn't cut off; speech goes out that much later until the
gate closes. It stays shut for the first 200 ms after the microphone opens,
while the suppressor learns the noise. Opus stays at songbird's 128 kbps in
its music mode.

"Take out your room's echo in Discord calls" (`reduce_room_echo`, off unless
turned on) is for a voice that sounds far away in a bare room: the reverb the
room puts on it, which echo cancellation doesn't touch, since that only takes
out what the speakers play. It's online weighted prediction error (WPE;
Yoshioka and Nakatani 2012, in Caroselli et al.'s 2017 recursive least
squares form), in `crates/talk-core/src/dereverb.rs`: in each band up to
5 kHz, the reverb in a 20 ms frame is predicted from the 200 ms of frames
starting 20 ms before it, and subtracted. The first 20 ms of the room, which
the voice needs to sound whole, stay. DeepFilterNet takes some of a room out
on its own, more than RNNoise, but its short view of the past limits it on
late reverb. Its post-filter, tried at
0.05-0.2, cut tails by up to 3 dB more without bringing PESQ closer to the dry
speech, and cost 0.1-0.6 PESQ on the noise mixes, so Talk leaves it off. A
search in October 2026 found no open model that takes a room out at 48 kHz in
real time on a processor; LocalVQE does at 16 kHz.

`scripts/compare-room-echo` runs the noise comparison's speech through
synthetic rooms (a direct sound, then a decaying diffuse tail) and the whole
microphone. Results on 2026-10-12, as above:

| Room's reverberation time, reverb against direct sound | 0.4 s, -6 dB | 0.6 s, -3 dB | 0.8 s, 0 dB |
| --- | --- | --- | --- |
| PESQ against dry speech: unprocessed | 2.94 | 2.31 | 1.89 |
| Room echo left (DeepFilterNet alone takes some out) | 3.15 | 2.66 | 2.24 |
| Room echo taken out | 3.44 | 2.86 | 2.40 |
| Reverb 50-150 ms after phrases, unprocessed, left, out, dB | -21, -29, -32 | -14, -24, -26 | -10, -18, -19 |

On the noise mixes, PESQ fell 0.01-0.08 with it on (hiss 3.51 to 3.43), and
on dry speech 0.05. It takes 0.04-0.08 ms more a block, about a fifth more,
and puts the voice 10 ms later. Working up to 24 kHz rather than 5 kHz took
no more out at 1.7 times the cost; 100 ms of history rather than 200 took a
third less out.

Echo cancellation is WebRTC's AEC3 with its defaults, fed the speakers' mix
as it's written to PulseAudio. Fed a recording of a friend's voice through a
simulated room into the microphone (`TALK_AUDIO_FAR` below), it took 42-48 dB
of echo out with the speakers 60-400 ms behind, 37 dB at 500 ms, and none at
600 ms, past what its delay search covers. A room reverberating for 0.65 s or
more left enough of the echo (21 dB taken out) for the gate to send some of
it, which longer filters in AEC3's configuration could help.

`scripts/compare-noise-suppression [LIMIT]` compares the two, and Off through
the whole microphone, at a strength's limit in dB, on 90 s of speech, two
Spoken Wikipedia readings at -26 dBFS with 1.5-4 s pauses, mixed with four
keyboard recordings (95% of blocks below -32 dBFS), a recorded fan high-passed
as WebRTC does (-42 dBFS), pink hiss (-45 dBFS), and pairs of them; for the
gate, the pairs again 14 dB quieter. PESQ is wideband, against the clean
speech. Results on 2026-10-12, with the `fast` profile on a 12-core desktop CPU:

| | RNNoise | DeepFilterNet |
| --- | --- | --- |
| Time per 10 ms block, suppressor alone, mean (p99) | 0.043 ms (0.05) | 0.34 ms (0.46) |
| Time per block, the whole microphone | 0.06 ms | 0.35 ms |
| Microphone thread in a call, fake Discord | 1.4% of a core | 5.9% of a core |
| Output behind input | 10 ms | 30 ms |
| Memory | | 39 MiB more, loaded in 0.2 s |
| PESQ, clean speech | 4.53 | 4.53 |
| PESQ, typing (unprocessed 1.61) | 1.99 | 3.05 |
| PESQ, fan (2.23) | 2.60 | 3.37 |
| PESQ, hiss (1.72) | 2.88 | 3.49 |
| PESQ, typing and fan (1.52) | 1.93 | 2.78 |
| Typing left in pauses, mean and loudest block | -47, -29 dBFS | -79, -53 dBFS |
| Pauses sent with typing, the gate's 400 ms hold included | 70-83% | 6.3-6.4% |
| Times typing opened the gate in pauses, five typing mixes | 3-11 each | 0 |
| Speech sent, all mixes | 99.8-100% | 99.1-100% |

DeepFilterNet is 20 ms later than RNNoise. Both lag by half their 20 ms
window, 10 ms, and DeepFilterNet also looks two blocks ahead. With the gate's
100 ms, speech goes out 130 ms late. The speech it doesn't send is mostly the first second after
the microphone opens, while its normalization and WebRTC's gain control
settle, and 10-50 ms at the quiet ends of words. These are mixes, not a
microphone in a room, and nothing compared Krisp.

To hear the processing, run a recording through it: raw mono 16-bit 48 kHz in
`TALK_AUDIO_IN`, with `TALK_AUDIO_OUT` and optionally `TALK_AUDIO_SENT`, then
`cargo test -p talk-core processes_a_recording -- --ignored --nocapture`.
`TALK_SUPPRESSOR=rnnoise` or `off` chooses the suppressor,
`TALK_NOISE_LIMIT` limits it in dB, `TALK_DEREVERB=on` takes the room's echo
out and `TALK_LEVELING=off` turns leveling off. `TALK_AUDIO_FAR`, in the same
format and as long, is what the speakers played, for echo cancellation;
`TALK_AUDIO_ECHOED` then gets the microphone with only echo cancelled.

### Screen sharing

Go Live streams come through Discord's gateway: op 18 starts sharing, op 20
watches someone, op 22 pauses and op 19 ends either. Discord answers with
STREAM_CREATE, naming the RTC server, then STREAM_SERVER_UPDATE with an
endpoint and token. The stream gets a voice connection of its own to that
server, reusing the call's voice session id; a stream waits for that id too,
if this account's voice state arrives after both answers. `talk_discord::stream` names
streams by key (`guild:{server}:{channel}:{user}`) and
`Discord::{go_live, watch, end_stream, pause_stream}` run the signalling. Voice
states carry `self_stream`, which shows as a LIVE badge beside the person.

Capture runs in a worker process, `talk --stream-worker`, so its buffers take
memory only while sharing; the process exits when sharing stops. Playing a
video file runs there too (`Job::Play`, `video_player`), which also keeps
FFmpeg's reading of files people sent out of the window's process. Its
frames carry their time, which the seek bar shows.
`stream_worker` starts it with one job as a JSON line and reads its reports
back. On Linux it starts `/proc/self/exe`, the running Talk, since Talk's path
names a deleted file once a rebuild or update replaces it. Frames reach the UI through `frames`, a triple buffer in a memory-mapped
file in the runtime directory. The file is unlinked once the worker opens it.
Windows won't delete a mapped file, so there it's a temporary file that
Windows deletes once both processes close it.
On Linux, libwebrtc's capturer asks the screen sharing portal what to share.
The portal answers over D-Bus through GLib, so the worker runs GLib's main
context while it waits for the dialog and for frames. Talk tells Discord it's
live only once frames flow, so a cancelled dialog shares nothing. Windows has
no such dialog, so the share menu lists the screens and windows libwebrtc's
capturers find (`stream_worker::shareable`), and the worker captures the one
picked by its id; see the
[Windows guide](windows-beta.md#go-live-on-windows).
`TALK_STREAM_TEST_PATTERN=1` shares moving colour bars instead of a screen.

Once Discord has started the stream, Talk unpauses it with op 22, as
Discord-video-stream does, and the worker gets its server and sends the video
there (`talk_discord::stream_connection`). The connection is like a call's,
but on voice gateway version 8 as Discord's clients use, where songbird uses
4: heartbeats acknowledge the last sequence number, and binary DAVE messages
start with one. It identifies with the stream's RTC server and `video: true`,
discovers its address, then uses `aead_aes256_gcm_rtpsize` transport
encryption. Talk offers H264 only and announces its size with op 12, again
whenever someone connects. Frames come from OpenH264 in libwebrtc's archive
through `native/h264.c`: constrained baseline, realtime and tuned for screen
content, with a keyframe at least every 3 s and whenever RTCP asks for one.
Talk also has VP8 (`native/vp8.c`, still built and tested); every Go Live
sender known to work sends H264. H264 goes as whole NAL units or FU-A
fragments, and past a frame's first 8 packets, 4 go out each millisecond: a
1080p keyframe of 200 sent at once can overflow buffers on the way. Talk keeps
the last 2048 video packets, and when a viewer reports some lost with a
generic NACK (RTCP 205), resends up to 128 of them as Discord's client does:
on the RTX SSRC with RTX's payload type and numbering, the original sequence
number before the payload (RFC 4588), and the repaired stream id (extension
12) in place of the stream id. The fake server reports one packet lost and
checks what comes back. An RTCP sender report goes out every second.

Before sharing, the share button's menu picks a resolution and a frame rate,
as Discord's client asks for them: 720p, 1080p or the screen's own size (up to
4K), at 15, 30 or 60 fps. Talk remembers the choice beside the chosen audio
devices (`devices.json`), and starts at 1080p30. Larger screens, and those of
another shape, are scaled down to fit. Talk sends them at the bitrates
Discord's client uses: at most 3.5 Mbit/s up to 720p and 9 Mbit/s above, the
cap op 12 announces, with the frame rate as its `max_framerate`.
Like Discord's client it also follows the server's bandwidth estimate
(REMB): when that drops below the cap, the encoder aims for the estimate
instead, though never below 500 kbit/s, and it changes only when the two
differ by more than a tenth. Discord's client allows accounts without Nitro
720p30, but trials 1080p30 at 3.5 or 5 Mbit/s for them (its
"2026-05-frontier-tuning" experiment). The worker converts captured
frames to I420 with libyuv, scales them with libyuv's filtering scaler, and
encodes those; the preview is the same picture scaled down to 720p.

Discord shows a still of each stream, in the call and to people outside it.
Like Discord's client, Talk makes one once the stream connects and the
picture isn't black any more (a screen is black until what's shared draws),
then every 5 minutes: the picture scaled down to fit 512 by 288, a JPEG at
quality 92. The worker reports it as a data URI, and Talk uploads it with
`POST /streams/{stream key}/preview`. A failed upload is tried again a minute
later, up to three times; a newer still replaces an upload not done yet, and
stopping the share cancels it. The fake server takes previews of Alice's
stream only, and lists them at `/_control/video`.

A shared screen carries the sound every other application plays, as Discord's
client and Vesktop share it; Talk's own sound stays out, so viewers never hear
the call echoed back (`stream_audio`, with `audio_io::others` hearing the
applications). On Linux, PulseAudio or PipeWire's
PulseAudio server monitors each application's playback stream on its own
(`pa_stream_set_monitor_stream`, as pavucontrol's level meters do), and
streams from Talk's processes are skipped by process id. So are other Discord
clients' (Discord, Vesktop, Equibop, WebCord, Legcord, ArmCord, by application
name or program): with one of them in the call too, its sound echoed the call
back to the people in it. The worker mixes them every 20 ms, waiting until an
application has 40 ms queued and keeping at most 200 ms of each, and encodes
Opus at 64 kbit/s, what Discord's client gives a stream's sound. Frames go out
as plain Opus RTP on the stream's audio SSRC, DAVE-encrypted, with sender
reports, and op 5 marks them as shared sound (speaking flag 2). Nothing is
sent while no application plays. On Windows, WASAPI's process loopback hears
everything but Talk's own process tree as one stream, from Windows 10's 2004
update on; it can't leave other Discord clients out. Other systems share
video only for now, and so does the test pattern. Two ignored tests check the capture against the
machine's sound server with a quiet tone: another process's is heard, and
Talk's own is not.

Each video packet carries the one-byte extensions a Discord client sends: a
transport-wide sequence number (id 5), playout delay (6), screen content
type (7) and the stream id `100` (11). The stream id is what ties a packet to
the stream op 12 declared. These ids aren't the ones in Discord's WebRTC
SDP, where 5 is playout delay. Talk once sent only playout delay as id 5, and
Discord's server dropped every packet: its receiver reports said it had
received nothing, while it did read our RTCP, and viewers timed out with
error 2012. concord and IMDiscord, two clients that broadcast over this
transport, use the same set. The fake server drops video without the stream
id as well.

DAVE follows songbird's handshake (`dave.rs`) and encrypts each frame once the
group is ready. A stream's MLS group isn't the voice channel's: its id is the
RTC server id minus one, which Discord-video-stream found too. If the server's
proposals or commits name another group, Talk moves to it. The vendored
davey's `set_channel_id` keeps the key package already sent, so the streamer's
welcome still opens. Talk accepts the people op 11 names into the group, and
also those named in video attributes (op 12) or in the stream key.

With `TALK_LOG=debug`, Talk logs every 5 s while sharing what it sent and the
RTCP that came back (`stream sent`): receiver reports with the highest
sequence number Discord's server got and its echo of our sender reports
(`lsr`), its bitrate estimate (REMB), and keyframe requests, and each change
of bitrate. The voice connection logs the voice server's RTCP too, to compare
with a sender Discord accepts. Messages the stream server sends are logged
too, except those carrying keys.

Watching works the same way in reverse. Clicking someone's LIVE badge, or
their tile in the call, sends op 20 and starts a worker with a watch job. The
worker connects as a viewer, offers H264 and VP8 for decoding only, and asks
for video with op 15. It learns the streamer's SSRCs and codec from op 12 and
op 14, then puts RTP packets back in order (`rtp::Reorder`) and together
into frames (`rtp::Assembler`: VP8 descriptors, and H264 single units,
STAP-A and FU-A as Annex B). As Discord's client does, it asks for a missing
packet again at once with a generic NACK, again every 80 ms up to three
times, and takes the resend from the streamer's RTX SSRC (RFC 4588); padding
there, which probes the path, is dropped. Packets after a gap wait for it at
most 300 ms. A frame still missing a packet is dropped, and so is everything
after it until the next keyframe. So are frames while none has come yet,
and after one the decoder was too far behind to take. The connection asks
for a keyframe with a picture loss indication, at most every 500 ms. It
sends a receiver report every second (packets lost, the highest received,
and the streamer's last sender report echoed). With `TALK_LOG=debug` it logs
what it received every 5 s (`stream received`): packets out of order,
resent, recovered, given up on, and keyframes asked for. The fake server
loses one packet of each stream it sends a viewer and delivers another late,
and resends what's asked for. The worker decodes with
libvpx or with the FFmpeg H264 decoder in libwebrtc's archive
(`native/decode.c`) and writes RGBA at up to 1080p into the shared frames.
Watching is possible only in the call you're in. The stream's sound, Opus on
the streamer's audio SSRC, is DAVE-decrypted as audio and played by the worker
through `audio_io.rs` on the call's speakers (`stream_audio::Player`), about 100
ms queued; being deafened silences it, which Talk tells the worker with a
`hear` control. Talk's workers play as "Talk", which sharing leaves out, so a
stream you watch never reaches one you share. Watching a stream from Discord's
own client works: H264 with DAVE, confirmed by hand.

The fake server checks the transport, packetization, sender reports and frames
that Talk sends, the sound's Opus packets and soundshare flag, reading an H264
picture's size from its SPS, without DAVE. To viewers it streams the
recordings in `testing/fixtures` (VP8, or H264 after `POST
/_control/stream-codec`) once they ask with op 15. Checked by hand against
real Discord: Discord's client shows Talk's stream at 1080p with its sound,
without the call's, and Talk shows streams from Discord's client.

## Servers, channels and messages

Talk shows Discord live from its own Discord connection. Every server, channel
and DM is visible without copying anything into Matrix.

Talk lists the linked account's Discord servers in the rail with their
channels. Channels the account's roles and channel overwrites keep it out of
show greyed out with a lock, as in Discord; channels hidden from it appear the
same way only with the "Show channels you can't access" setting, which is off
by default. Server text channels are listed under their Discord categories and
open in Talk's timeline. Clicking a category's heading collapses it, leaving
the open channel, unread ones and voice channels with people in them, as in
Discord; Talk remembers which categories are collapsed, per server. History
loads from Discord 50 messages at a time (`GET /channels/{id}/messages` with
`before` the oldest loaded message and `limit=50`) as the timeline scrolls
near its top, one page at a time per channel, until a short page shows the
channel's first message is there; and new messages, edits, deletions
and reactions arrive live. A jump to a message that isn't loaded, from a
reply's quote, a pin or a search result, asks for the 50 around it
(`around` its snowflake), as Discord's client does, and they show in place
of the timeline as a window of older messages; see [jumping to a
message](#jumping-to-a-message). Opening a channel subscribes to its server, as the
web client does, because Discord sends large servers' messages only to
subscribers. Talk sends, replies, edits, deletes,
reacts and sends files through Discord's cloud uploads. Mentions and timestamps
show as text, custom emoji and stickers as pictures, every attachment shows,
images with previews scaled by Discord's media proxy, and channels the account
can't send in are read-only. Bots and webhooks have a BOT tag after their
name. An author's name takes their highest coloured role's colour, once their
roles are known: live messages carry them, and for history Talk asks Discord
for the authors' member profiles.

Talk keeps the open channel's messages in memory, and those of the 40
channels open before it, so going back to one shows it at once. A closed
channel keeps its newest 200 messages as metadata and follows new messages,
edits, deletions and reactions while it's kept. All channels share one
8 MiB cache for decoded preview pixels, with least recently used eviction.
Only messages in or near the timeline viewport request previews; evicted
previews can be decoded again from the disk cache. Opening it again loads the
latest page in the background and fits it in: messages kept within the page's span that it lacks were deleted
meanwhile, and when a full page starts after everything kept, more came than
a page holds, so only the page stays. The UI keeps the same 40 channels'
last timeline and member list, so going back shows them before the chat side
answers, without "Loading messages...". It keeps what the chat side keeps of
each timeline, the newest 200 messages as metadata. It drops decoded preview
handles when closing a channel, so remembered UI timelines do not pin pixels
outside the shared cache. Previews show as loading until the viewport requests
them. Channels also go to disk, as the startup cache below describes, so a
channel opened after a restart shows its saved messages while the latest page loads
and fits in the same way.

The open channel renders each message for the timeline once, with its
author's name, colour and avatar and its mentions as names, and keeps it. A
new message, an edit, a reaction or a preview that arrived renders only that
message again; a change to someone's name, avatar or roles, to the
server's roles or channel names, or to who's blocked, renders them all. A closed channel drops its rendered messages,
since they hold its previews.

Unread dots and mention counts follow Discord's read states, mutes and
@everyone and role suppression, including passive updates from large servers.
A message that calls on this account is highlighted in the timeline, as in
Discord: one that mentions it, one of its roles in the server, or @everyone
or @here, from a message's `mentions`, `mention_roles` and
`mention_everyone`, which count only what the author may mention. The
account's settings for the server can suppress @everyone and @here, and
roles. A reply to its message is highlighted too, whether it pings or not.
Each server in the rail shows its mentions as a count, and the DM button
counts DM mentions and the friend requests waiting for an answer.
Showing an unread channel marks it read, and with the "Mark messages read on
Discord" setting, on by default, acknowledges it on Discord within a second.
Reads within that second go together: one channel as its own
acknowledgement, several, as when a server is marked read from its menu, in
one `POST /read-states/ack-bulk` of up to 100 channels, as the official
client does.
Before it does, Talk notes the channel's last read message, from READY's read
state or the last acknowledgement since, and the "New" divider goes above the
first message after it from someone else. Snowflakes order messages, so the
divider finds its place even when the last read message was deleted. When
every loaded message is unread, the 50 around the last read one load in
place of the newest, as a window of older messages that opens at the
divider; when even those are all newer, the divider goes above the oldest.
The open text channel lists its members on the right as Discord's sidebar
does: under hoisted roles, then online, then offline, with offline members
faded. Each shows a status dot for online, idle or do not disturb, their name
in their highest coloured role's colour, and their custom status or what
they're playing below it. Talk shows the list's first 100 rows, headings
included. Discord keeps one list for all channels with the same readers: the
overwrites that allow or deny seeing them, `everyone` when none do. Talk keys
its lists the same way, so moving between such channels shows the same list
and asks Discord for nothing. Talk keeps the last 40 lists. One Discord still
sends, or sent within 10 minutes, shows without asking again. An older one
shows at once while Talk subscribes, and Discord's sync replaces it. A list
on screen that Discord doesn't send is asked for again once it's 10 minutes
old. When no list comes within 8 seconds of opening a channel, the panel
says Discord didn't send one instead of loading on, and the header says
"Members" until a count is known. A build without Matrix shows the Discord
account's name, avatar and connection in the bottom-left panel.

Talk starts from Discord as it last saw it, kept in the data directory by
`talk_discord::cache`, and Discord's answers replace it as they arrive:

- `discord-account.json` holds the account's name and avatar, its servers
  with their channels, roles and emoji, its DMs, friends, friend requests and
  status. It also holds the names Talk learned (users, server nicknames,
  server avatars and members' roles), which messages and member lists show
  people by, and the member lists shown lately. Every message Discord sends
  from any server teaches Talk its author, so it keeps the 20,000 people it
  learned most lately and forgets a quarter of them, those learned longest
  ago, once there are more. It's written at most every 20
  seconds while connected. Calls and who is in voice aren't saved. On the
  next start Talk shows it all at once, identifies with the saved status, and
  READY replaces it. A saved member list shows at once, even before Talk
  connects, and Discord's sync replaces it. The list on screen is kept with
  them when the connection drops, so it shows again at once after
  reconnecting.
- `discord-messages/` holds, for each channel shown lately, its newest 100
  messages, written within 20 seconds of a change, when the channel leaves
  the 40 kept in memory, and when Talk closes. The 300 channels used last are
  kept. Opening a channel shows its saved messages, and the latest page fits
  in as it does for a channel kept in memory.
- `discord-pictures/` holds avatars, server icons, emoji and stickers, and the
  media proxy's previews of attachments and links, up to 8 MiB each. Discord
  gives each version of a picture its own address, so a kept one is never
  stale; the proxy's signature (`ex`, `is` and `hm`) isn't part of the key,
  so a picture signed again is found. Past 256 MiB the least recently shown
  go. Attachments' own files aren't kept.

The files are readable only by the user and aren't encrypted. Each names its
login by a SHA-256 hash of the token, so another linked account starts empty,
and unlinking deletes them all. While Discord is unreachable, an open channel
shows what was saved, waits, and loads once Talk is back, rather than showing
an error.

- Discord servers appear in the server rail with a Discord badge, and Discord
  DMs appear in the DM list with the same badge. A build without Matrix leaves
  the badge off, since everything in it is Discord's.
- A build without Matrix leaves out the composer's line about encryption and
  keys, since everything in it is Discord's; what stops sending still shows
  there. The account's dot in the bottom-left shows its Discord status: idle,
  do not disturb, or the grey ring when invisible or disconnected.
- Discord servers keep the order the account's server list has in Discord,
  from `guild_folders` in its settings, and follow it when it changes
  elsewhere. A server the settings don't list yet, such as one just joined,
  goes first.
- Folders show in the rail as in Discord. Closed, a folder is a tile of its
  colour holding its first four servers' icons, with the pill, unread dot
  and mention count of all its servers; its tooltip is its name, or its
  servers' names when it has none. Clicking opens it: a folder icon in its
  colour, its servers below it on a strip of that colour. Talk remembers the
  open folders on this device, in `layout.json`, as Discord's client keeps
  them per device. Settings entries with an id are folders, read with their
  name and colour (`GuildFolder` fields 2 to 4, each in a wrapper); one
  without is a server on its own. Folders can't be made or changed in Talk
  yet. `--folders` gives the fake Discord's filler servers two folders.
- A server out of reach in a Discord outage (`GUILD_DELETE` with
  `unavailable`, or READY listing it so) stays in the rail, faded, as last
  known, until its `GUILD_CREATE` brings it back. Only a `GUILD_DELETE`
  without `unavailable`, as when the account leaves, removes it.
- Server icons and avatars come from Discord's CDN, which needs no login.
  Talk asks for each one once it shows: in the rail, beside people in a
  voice channel of the open server, in the call, or beside a message. A
  member's server avatar wins over their own. Talk decodes them at 128px
  before the UI sees them. Once more than 400 have arrived, those off
  screen go, and Talk asks for them again, from disk, when they show.
  Custom emoji stay, since the picker asks for hundreds at once.
  Initials show until a picture arrives or when it fails. A download that
  may work later, when the CDN is out of reach, fails or answers 429, is
  tried up to six times; every download then waits together, 10 seconds
  and doubling up to five minutes, so a CDN in trouble hears from Talk
  rarely. One Discord refuses, such as a 404, isn't tried again. A server
  without an icon keeps its initials, and a person without
  an avatar gets Discord's default avatar for their account.
- Messages support text, replies, edits, deletions, reactions, mentions and
  attachments. Up to ten files go with a message, uploaded to Discord's
  storage first and then named in the message. Each file is limited as
  Discord limits it since 13 August 2026: 20 MB without Nitro, 50 MB with
  Nitro Basic or the old Nitro Classic, and 500 MB with Nitro, by the
  account's `premium_type`. A server boosted to level 2 allows 50 MB and one
  at level 3 100 MB to everyone, whichever is higher. Discord sends the
  server's `premium_tier` in READY and GUILD_UPDATE, and the account's
  `premium_type` in READY and USER_UPDATE.
- Link previews and bots' cards show from the embeds Discord adds to a
  message: Discord's servers read the linked page's Open Graph tags, often a
  moment after sending, and the preview arrives in a message update. A card
  shows the site, author, title, description, fields and footer behind a bar
  of its colour, with its picture large for articles and videos and small
  beside the text otherwise. A video the media proxy has (`video.proxy_url`)
  plays in place, as an attached one does, under the card's thumbnail as its
  poster, whatever the card's type: vxtwitter's posts come as `rich` cards
  with one. Others, such as YouTube's, open in the browser. Other pictures open in the image viewer. A linked image or GIF
  shows with the message's images, without its address when that's all the
  message says. Pictures come through Discord's
  media proxy (`images-ext-*.discordapp.net`), so Talk never contacts the
  linked site, and embeds of a message marked to hide them don't show.
- Animated GIFs, APNGs and animated WebPs play, in messages, as stickers and
  in the image viewer. The media proxy keeps GIFs as GIFs; other pictures
  come as WebP, asked for with `animated=true` as Discord's client does, so
  an animated one, such as vxtwitter's AVIF GIFs, stays animated rather than
  turning into its first frame. An animation keeps its first frame in
  memory, and its file in `discord-pictures/`, or in memory when it isn't
  kept there, as one over 8 MB isn't. Its frames decode one at a time on the
  UI thread as they show, reading the file, and a second after it last
  showed, as when its row scrolls away, it lets its decoder go. Decoded
  whole, a few seconds of a GIF took tens of megabytes, and an open channel
  kept every one it had loaded. Frames shorter than 20 ms play at 100 ms, as
  browsers do. Tenor and Giphy links arrive as MP4 "gifv" embeds, which play
  the same way, silent and looping, from the proxy's copy of the MP4
  (`video.proxy_url`): up to a minute of it, decoded by the FFmpeg, libvpx
  and dav1d inside libwebrtc (`native/video.c`). One that can't be decoded
  shows its still picture. It's named and saved as its video, not its
  picture.
- Attached videos show their poster, which the media proxy makes from the
  first frame (`proxy_url?format=webp`), with a play button; they play in
  place, with sound, under a bar to pause, move through them, mute and show
  them full screen. Full screen, as the image viewer shows a picture, the
  video is enlarged to fit the window with its bar under it; Space plays or
  pauses it, the arrow keys move five seconds, and Escape brings it back to
  its cell. The worker draws its pictures at the size they show: playing,
  the next ones come at a new size; paused, the one showing is redrawn,
  which moving back to it would hold up while playing. MP4, MOV and WebM
  play, with H264, VP8, VP9 or AV1 pictures in 8-bit 4:2:0 and AAC, Opus,
  MP3, Vorbis or FLAC sound; HEVC, which iPhones record, doesn't, as
  libwebrtc's FFmpeg lacks it. The file downloads into `discord-videos/`
  beside the account file, and plays as it arrives: the worker's reads wait
  for the download. It goes when the video stops, as when the channel
  closes. Sound plays on the call's speakers through `audio_io.rs`, on
  Linux and Windows. Over the mute button a slider sets how loud, as does the
  wheel; its travel is squared, so its middle sounds about half as loud.
  Turning it to nothing mutes, and unmuting brings back the level it had.
  The next video plays as loud, until Talk closes.
- Spoilers, `||like this||`, show under an opaque cover that a click lifts.
  The cover is drawn by Talk's patched Slint text renderer over a link to
  `talk:spoiler/{item}/{n}`; see [building](building.md). A spoiler in a reply
  or a notification reads "(spoiler)". Attachments named `SPOILER_...` or
  flagged as spoilers, and pictures linked inside a spoiler, show blurred with
  a "Spoiler" label until clicked. Revealing lasts while Talk runs. The eye
  on a file waiting in the composer marks it as a spoiler, which blurs its
  thumbnail; Talk uploads it as `SPOILER_` and its name, as Discord's client
  does. Matrix has no spoiler for a file, so its rooms don't offer the eye.
- Custom emoji show as pictures: in messages, as reactions and in the emoji
  picker, which lists them by server after the frequently used ones, the open
  server's first. In messages Talk draws them as text: each emoji whose
  picture arrived gets a code point in the Supplementary Private Use Area B,
  and each batch of them a colour bitmap (CBDT) font made on the fly, which
  text falls back to after Satoshi, so they wrap and select like letters.
  The picker, suggestions and reaction buttons show the picture itself, so
  the hundreds a picker lists add no fonts; a quiet square holds each place
  until its picture arrives. A message of only
  emoji, up to 30, shows them large, as Discord does. Without Nitro, the
  picker offers only the open server's emoji, none animated, and sending
  turns `:name:` into Discord's `<:name:id>` only for those. Animated emoji
  show their first frame.
- Stickers show as pictures at 160px, PNG and APNG stickers from the CDN and
  GIF ones from the media proxy. Lottie stickers, which are vector
  animations, show their name.
- Messages from people this account blocked show collapsed as "Blocked
  message", which "Show it" opens, and they don't notify or show typing.
  Talk learns the blocked from READY's relationships (type 2) and keeps them
  current from RELATIONSHIP_ADD and RELATIONSHIP_REMOVE.
- A reply's quoted line jumps to the original and highlights it for a
  moment, as described under [jumping to a message](#jumping-to-a-message).
- Desktop notifications follow the person's Discord settings: each channel's,
  category's or server's level (every message, mentions only or nothing),
  falling back to the server's default, with mutes leaving only mentions. DMs
  notify unless muted; message requests and spam don't. Apps' answers to
  slash commands and context menu commands notify as people's messages do;
  joins, pins and call notices don't. Nothing notifies while the account's
  status is Do Not Disturb. They show while Talk
  isn't in front, titled with who wrote where, and clicking one opens the
  conversation. On Linux they go to the freedesktop notification service.
  Each also plays Talk's message sound, or its mention sound, unless it's in
  the conversation in front; see [voice](voice.md#sounds).
- Mentions in messages are links on a light fill, in the role's colour or the
  person's name colour, as Discord shows them. A person's opens their profile
  card beside the mention, and a role's lists the members Talk has seen with it (from member lists, messages
  and member requests), each opening their card in turn.
- Typing `@` suggests who to mention: the channel's listed members, those who
  wrote lately, and server members a search by name finds (the gateway's
  member request with a query, as Discord's client does), then the roles
  anyone may mention, and every role with @everyone and @here for those
  allowed to mention everyone. Sending turns each `@Name` into Discord's
  `<@id>` or `<@&id>`.
- Who else is typing shows above the composer, for ten seconds after Discord
  last said so or until their message arrives. Writing tells Discord Alice is
  typing, at most every eight seconds, unless a setting turns it off.
- Reading a message in Talk marks it read on Discord, so the phone app agrees.
  A setting turns this off.
- Clicking the account's avatar or name, bottom left, opens a menu of Online,
  Idle, Do Not Disturb and Invisible, as in Discord. Talk saves the choice in
  the settings below and sends it with opcode 3. Do Not Disturb holds back
  Talk's message notifications. Closing Talk takes the person offline, like
  closing the Discord app.
- The custom status shows under the account's name. "Change custom status" in
  that menu, or "Set a custom status" in My account, sets it: an emoji from the
  picker, words, and when it clears (never, 30 minutes, an hour, four hours,
  or at midnight). Discord keeps it in the account's settings, a protobuf
  (`PreloadedUserSettings`, field 11), which arrive in READY as
  `user_settings_proto` and change in USER_SETTINGS_PROTO_UPDATE. Talk reads
  only the status and custom status from it, and changes either with `PATCH
  /users/@me/settings-proto/1`, whose body sets nothing else. Each session
  shows the status to others itself, so Talk sends it with opcode 3 once
  connected, whenever the settings change, and clears it when it ends. A
  custom emoji in a status needs Nitro.

### Jumping to a message

A reply's quote, a pin, a search result and the bar over the "New" divider
jump to a message. A loaded one scrolls into view. Otherwise Talk asks for
`GET /channels/{id}/messages?around={id}&limit=50`, as Discord's client does,
and the messages around it show in place of the timeline: a window of older
messages, with "You're viewing older messages" and "Jump to present" in a bar
over its end. Discord answers around a snowflake even when no message has it,
so a deleted message's place shows, and Talk says it couldn't find the
message. A jump into another channel opens that channel at the message.

Scrolling down the window loads the 50 `after` its newest message, and up,
the 50 `before` its oldest, a page at a time. Discord doesn't say how it
splits a page around a snowflake, so Talk learns where the channel starts
from a page before, and that the window reached the newest message from a
short page after, or once it reaches the newest messages Talk kept.

While the window shows, the channel's newest messages wait beside it, at
most 200 as a closed channel keeps, and follow new messages, edits,
deletions and reactions; edits, deletions and reactions show in the window
too, but new messages don't go in it. The channel stays unread, and isn't
marked read, until the newest show. Jump to present, sending a message, or a
page that reaches them shows them again without asking Discord, and the
window goes. Leaving the channel lets the window go too, and only the
newest go to disk. Pages loading for a window that a newer jump or the
present replaced are dropped when they come.

The window holds what was asked for, not every page between it and the
present. Growing upwards past 200 messages, it lets go of its newest,
below the screen, which load again; growing downwards it keeps its oldest,
which are above the rows on screen and would move them if they went, until
it joins the newest messages.

### Notification settings

Discord keeps the account's notification settings per server, and one set
for DMs: READY lists them in `user_guild_settings`, and
USER_GUILD_SETTINGS_UPDATE replaces a server's
(`talk_discord::notifications`). Each holds the server's mute, with
`mute_config`'s `end_time` when it ends, its level (every message, mentions
only, nothing, or the server's own default, `default_message_notifications`),
whether @everyone and @here, and role mentions, call on the account, whether
muted channels hide, and `channel_overrides`: each channel's and
category's own mute and level, where 3 means the category's or server's.
Talk publishes each server's settings with the server, shared between
snapshots until Discord changes them, and marks each channel muted when it
or its category is. A muted channel shows faint, as a muted DM does, and a
server set to hide muted channels leaves them out of the channel list, but
for the open one and those that mention the account, whose count the rail
shows. A mute with an end lifts by itself then: the session wakes at the
soonest end and publishes again.

`PATCH /users/@me/guilds/{guild}/settings` changes them, with `@me` for DMs.
The body names only what changes, and Discord keeps the rest; a channel's or
category's goes in `channel_overrides`, keyed by its id rather than listed:
`{"channel_overrides": {"<id>": {"muted": true, "mute_config":
{"selected_time_window": 900, "end_time": "..."}}}}`. A mute for good is
`selected_time_window` -1 and no end, as Discord's client sends it, and a
level is `message_notifications`. `Action::ChangeNotifications` asks for a
change to a server, by its server id, or to a channel or category, by its
room id: a mute for one of the DM menu's times, a level, or one of the
server's switches. The menus that offer them are still to come. The fake
keeps each server's settings and answers with USER_GUILD_SETTINGS_UPDATE.

### Permissions and roles

What the account may do in a server is its @everyone role's permissions
and those of its roles, everything for the owner and administrators
(`Server::server_permissions`); in a channel, the channel's overwrites for
@everyone, the roles and the member then apply
(`Server::channel_permissions`). Acting on another member, to kick, ban,
time out, rename or change their roles, also needs to outrank them: nobody
outranks the owner, the owner outranks everyone else, and otherwise the
highest role must be higher (`Server::outranks`). A role can be given or
taken by those who may manage roles and whose highest role is above it,
or by the owner, but never @everyone or one an integration manages
(`Server::may_grant`). The core asks through `Application::may`,
`outranks` and `may_grant`, and `server_roles` lists a server's roles,
highest first, with their colours, from the server the latest snapshot
shares. Members' roles are learned with their names; member list rows and
`ChatMember::roles` share each member's list with them rather than copying
it.

### A server member's menu

Right-clicking someone in a server channel's member list, or a message's
author there, opens their menu in the order of Discord's: Profile, Mention,
Message, Call, Add note and the friend nickname; Change nickname, friendship,
Ignore and Block; Roles; Copy ID; then, in red, Time out, Kick and Ban, each
naming them. What shows depends on this account's permissions and on its
highest role against theirs, checked when the menu opens
(`application/members.rs`). The member's roles and timeout come from the
service's names, which each snapshot shares as `SharedNames` rather than
copying them.

- Mention puts `@Name ` in the composer, which sending turns into `<@id>`,
  when the channel can be written in.
- Message opens the DM with them, or starts one with any member through `POST
  /users/@me/channels`. Call opens the DM and starts its call; with a friend
  Talk has no DM with, it starts the DM first and calls once it opens.
- Add note works for any member, and the friend nickname for friends, without
  a DM.
- Change nickname: one's own needs Change Nickname and is `PATCH
  /guilds/{id}/members/@me` with `{"nick": ...}`; another's needs Manage
  Nicknames and outranking them, and is `PATCH /guilds/{id}/members/{user}`.
  Empty resets it. `Action::AskMember` with `PersonAsk::ServerNickname` and
  this account's own id opens the same dialog from anywhere, by the server's
  id, as a server menu's "Edit server profile" can.
- Roles needs Manage Roles. Its submenu lists every role but @everyone,
  highest first, ticked where they hold it. Roles at or above this account's
  highest, and those an integration manages, show greyed, as Discord shows
  them. Ticking one is `PUT` or `DELETE
  /guilds/{id}/members/{user}/roles/{role}`; the box changes at once and the
  menu stays open.
- Time out needs Moderate Members and outranking them, and isn't offered for
  administrators, whom Discord doesn't let anyone time out. Its dialog offers
  Discord's lengths, 60 seconds to a week, and a reason, then `PATCH
  /guilds/{id}/members/{user}` with `communication_disabled_until`. Someone
  timed out is offered Remove timeout instead, which sends null, and the
  member list marks them with a red clock beside their name.
- Kick needs Kick Members and outranking them: a confirmation with a reason,
  then `DELETE /guilds/{id}/members/{user}`.
- Ban needs Ban Members and outranking them: how far back their messages go
  (none, the last hour, 6, 12 or 24 hours, 3 or 7 days) and a reason, then
  `PUT /guilds/{id}/bans/{user}` with `delete_message_seconds`.
- Reasons go to the server's audit log in the `X-Audit-Log-Reason` header,
  URL-encoded and cut to the 512 characters Discord keeps.
- One's own entry has Profile, Change nickname, Roles and Copy ID. Nobody
  outranks the owner, so nobody kicks, bans or times out the owner.

Discord answers each change with GUILD_MEMBER_UPDATE, or for a kick with
GUILD_MEMBER_REMOVE, and for a ban with GUILD_BAN_ADD as well, and sends the
member list's subscribers its operations. Talk learns the member's
nickname, roles and `communication_disabled_until`, shows the open member
list again when it lists them, and renders the open channel again, so names,
colours and the list follow. A member who leaves is forgotten. A nickname,
timeout, kick or ban that fails says why in its dialog; a role says so in a
notice.

Discord's menu also has Invite to listen (Spotify), Apps, an Invite to server
submenu and Open in moderator view, which Talk leaves out for now.

### DMs

DMs and group DMs are listed under the DM button after Matrix DMs, the latest
conversation first as in Discord, with the other person's avatar or the
group's icon and a Discord badge. They open, send and keep their messages
like server channels, under the room id `discord:@me:<channel>`, which takes
`@me` for the server as Discord's links do. Every unread message in a DM
counts as a mention, and a muted DM shows neither, as in Discord. A DM
without a read state counts as read, since Discord keeps one for each DM
with something to read. A one-to-one DM shows its person's status dot, with
their custom status or what they're doing under the name. Discord sends
presence only for friends, so anyone else shows the grey offline ring, as in
Discord's DM list. These follow PRESENCE_UPDATE live. The header of an open
one-to-one DM shows the same dot on their avatar, and their custom status or
what they're doing in a bubble beside the name. A group DM
shows how many are in it. A group DM lists its members by name, with the
presence of those who are friends. Talk downloads the pictures of the first
150 DMs listed; the rest keep their initials.

Friend requests, message requests and spam have their own screen. A
"Requests" entry below Friends at the top of the DM list opens it in place of the
conversation, with a badge counting the incoming friend requests (spam
excluded) and message requests. Message requests come from people outside the
account's DMs. Those Discord marks as spam are listed too, under "Spam", so a
person Discord misjudged can still be found. The entry stays highlighted
while a request from the screen is open. Opening one shows its messages with Accept and Ignore in place of the
composer: replying waits for accepting, as in Discord. Accepting makes it an
ordinary DM; ignoring removes it, and the sender isn't told. Requests count
no mentions, spam isn't marked unread, and Talk never opens a request on its
own when it picks a conversation. Forwarding is still open.

The pen in the DM list's header starts a DM: it lists the account's friends
(from READY's relationships), then anyone else Talk knows by name, and
opening one posts to `/users/@me/channels`, which finds the DM with them or
starts one. Talk opens it once its CHANNEL_CREATE lists it.

### Friends and requests

"Friends", at the top of the DM list, opens the friends list in place of the
conversation. It lists the account's friends from READY's relationships by
name, each with their status dot and custom status or activity, under two
tabs in the header: Online, for those online, idle or on do not disturb, and
All. A search field filters it by name, and "Add Friend" in the header opens
the add-friend dialog. Discord sends presence for friends in READY's
`merged_presences` and in PRESENCE_UPDATE, so the list keeps up with them.
Clicking a friend, or the message button on their row, opens the DM with
them, or starts one as the pen does and opens it once Discord lists it.

The person beside the pen, in the same header, adds a friend by username: `POST
/users/@me/relationships` with `{"username": ..., "discriminator": null}` and
`{"location":"Add Friend"}` as the context. Discord's codes 80004 (nobody by
that name), 80000 (they take no requests from strangers) and 80007 (already
friends) get their own words.

Friend requests come first on the requests screen, from READY's
relationships (type 3 sent to this account, 4 sent by it) and kept current by
RELATIONSHIP_ADD and RELATIONSHIP_REMOVE. One sent to this account is
accepted with the check or ignored with the cross; one it sent is cancelled
with the cross. A profile card offers "Send friend request" to someone who
isn't a friend, or "Accept friend request" when they asked first, and the
header of a DM with them offers the same. Sending or accepting is
`PUT /users/@me/relationships/{id}` with an empty body, and ignoring or
cancelling is `DELETE` on the same path, each with the X-Context-Properties
header Discord's client sends: `{"location":"User Profile"}` when sending
from a profile, `{"location":"Friends"}` otherwise, as base64. Discord
doesn't let a client send many requests quickly, and Talk only sends one when
the person clicks. Requests Discord took for spam are listed too, marked
"Maybe spam".

### A DM's menu

Right-clicking a DM offers, besides Profile and copying, these:

- Pin keeps a DM or group DM at the top of the list. Discord's desktop client
  doesn't pin DMs, so Talk keeps the pins in `layout.json`, on this device.
- Start call joins the DM's call, as the phone in its header does.
- Add note writes this account's note on the other person:
  `GET /users/@me/notes/{id}` fills the dialog, a 404 meaning there's none,
  and `PUT` with `{"note": ...}` saves it, an empty one removing it.
- Add friend nickname, for a friend, is `PATCH
  /users/@me/relationships/{id}` with `{"nickname": ...}`, or null to remove
  it. RELATIONSHIP_UPDATE carries it back, and Talk shows a friend by their
  nickname in the DM list and the friends list, as Discord does.
- Close DM is `DELETE /channels/{id}`; Discord answers with CHANNEL_DELETE,
  and the DM returns when either side writes.
- Remove friend is `DELETE /users/@me/relationships/{id}`, and Block is `PUT`
  on the same path with `{"type": 2}`, each after the person confirms, with
  `{"location":"ContextMenu"}` as the context. Unblock is the same `DELETE`.
- Ignore is `PUT /users/@me/relationships/{id}/ignore`, and `DELETE` stops it.
  Discord marks the relationship `user_ignored`, which Talk tracks to offer
  the opposite.
- Mute is `PATCH /users/@me/guilds/@me/settings` with the channel's override:
  `muted` and a `mute_config` of `selected_time_window`, in seconds or -1 for
  good, and its `end_time`. Unmute sets `muted` false. USER_GUILD_SETTINGS_UPDATE
  brings the change back, and a muted DM shows faint and counts nothing unread.

Someone who isn't a friend is offered Add friend, Accept friend request or
Cancel friend request in place of Remove friend, unless they're blocked.
Right-clicking the other person's avatar or name in the DM's messages opens
the same menu. The person's choices show only in a one-to-one DM. A message request offers
neither pinning, closing nor muting, as it waits for an answer.

### A DM's header

A DM or group DM's header has, as Discord's does, its call, its pinned
messages, adding friends, the profile or member list, and a search. Video
calls aren't there yet. A server's text channel has its pinned messages and
a search too; the rest of its header is still to come.

- Pinned messages open under the pin, the latest pinned first, from `GET
  /channels/{id}/messages/pins`, whose `items` carry each `message`. Talk asks
  each time they open, and for the first 50.
- Search is `GET /channels/{id}/messages/search?content=...&offset=...` in a
  DM, and `GET /guilds/{id}/messages/search?channel_id=...&content=...` in a
  server's channel, 25 messages a page, newest first; each of `messages` is
  a list whose first is the one found. The results take the member list's or
  the profile's place, and Show more loads the next page. Discord indexes a
  DM or a server the first time it's searched, answering 202 with code
  110000 and a `retry_after`; Talk asks again up to five times.
- In a server, pins and results carry no member profiles, so Talk asks the
  gateway for those of authors it doesn't know yet and waits up to five
  seconds for them; then they show with their server nickname, avatar and
  role colour.
- Clicking a pin or a result jumps to it in the timeline, as a reply's
  original does; see [jumping to a message](#jumping-to-a-message).
  Pins and results show their text and name their files, without
  downloading them.
- Adding friends to a one-to-one DM makes a new group of them and the other
  person with `POST /users/@me/channels` and their ids in `recipients`, as
  Discord's client does, and opens it; the DM stays as it was. In a group,
  each is `PUT /channels/{id}/recipients/{user}`, which CHANNEL_RECIPIENT_ADD
  confirms. A group holds ten people, this account included, so the picker
  stops there, and it offers only friends who aren't there already.

### Profiles

Clicking someone in a channel's member list shows their profile in a small
card beside it, which "View full profile" enlarges into a centred one;
right-clicking a DM and choosing "Profile" opens the full one. A one-to-one
DM shows its other person's profile in a panel on the right, with the bio in
full and the mutual servers and friends. Mutual servers show their icon and
the person's nickname there; clicking one opens a card beside it with the
server's banner, member count (from READY's `member_count`), channel count
and "Go to server". Mutual friends show their avatar and status dot;
clicking one opens their profile card beside it, or their full profile from
a full profile; the person icon in the DM's header
hides or shows it, and Talk remembers the choice in `layout.json`. Talk fetches the
profile once each time the open DM changes to someone else. Talk fetches it
from `GET /users/{id}/profile` with the open server's id, so the server's
nickname, avatar, banner, bio and pronouns win over the person's own, and the
card lists their roles in role order, when they joined Discord and the server,
and the servers and friends the account shares with them. Profiles are kept for
five minutes, at most 32 of them. Presence and activity come from the member
list, since the profile doesn't carry them. When Discord refuses, the card
keeps what the member list knew and says so. Matrix members and one-to-one
Matrix DMs get the same card with their name, Matrix ID, avatar and room role.

### Later

In rough order. The browser covers these until then.

- Threads and forum channels.
- Pinning and unpinning messages.
- Forwarding Discord messages, which Discord marks as forwarded, rather than
  sending their text again.
- Making and changing server folders.
- Joining servers by invite, and leaving them.
- Discord's push-to-talk, and its voice settings.
- Camera video and the soundboard.
- Polls, activities, and server administration.
- [Mirroring into Matrix](#mirroring-into-matrix).
- A captcha fallback for linking, through a small embedded web view.
- The blocked on the friends page, beside Online and All.
- Blocking and unblocking from a profile.
- Animated custom emoji, which show their first frame.
- Copying custom emoji: copied text has their private code points, not
  `:name:`.
- A custom status's end while Talk is closed, or set from a device whose
  clock is off: Talk trusts the time Discord gives.
- Opening embed pictures from a card in the viewer with their neighbours.

## Mirroring into Matrix

Mirroring copies Discord DMs and chosen server channels into Matrix rooms on the
person's homeserver. It does two things:

- Devices without a Discord login can read and reply. Only one device needs
  to link Discord.
- History builds up on the homeserver from the moment of linking, independent
  of Discord.

Mirroring is on by default after linking. All DMs and group DMs are mirrored.
Server channels are chosen per server or per channel, because large servers
would fill the homeserver with other people's history. A setting turns
mirroring off completely. It's the last phase, because everything else works
without it. If it turns out not to be useful, it can be dropped.

### Layout

One space, named "Discord (Talk)", holds every mirrored room directly. Talk
groups the rooms by Discord server using the encrypted details below. Without
nested spaces, the homeserver can't count servers or channels per server. Each
mirrored room has only the person as a member.

Element Web hides rooms that belong to a space from its Home list, unless they
are DMs or the "Show all rooms in Home" setting is on
([`SpaceStore.showInHomeSpace`](https://github.com/element-hq/element-web/blob/develop/apps/web/src/stores/spaces/SpaceStore.ts)).
Talk never adds mirrored rooms to `m.direct`, so Element doesn't treat them as
DMs. Element X filters by space only on request, so it may still list them.
To keep other clients quiet:

- Mirrored messages use a custom event type, so other clients show nothing.
- Talk posts them as the person, so the homeserver sends no notifications
  for them and other clients show no unread badges.
- Mirrored rooms carry the `m.lowpriority` tag.
- The space topic says "Mirrored from Discord by Talk. Open in Talk to read."

### What the homeserver can see

Room names, topics and avatars stay empty. Talk stores each room's Discord
server name, channel name and IDs in a custom state event,
`app.yapfast.talk.discord.channel`. The event content is ciphertext. The space
carries the same kind of event for the Discord account. Element shows the rooms
as unnamed.

Talk creates one random key per Matrix account and stores it in secret storage
under a custom name through the SDK's `SecretStore::put_secret`. Any device
that enters the recovery key gets it, and Talk keeps a local copy after that.
Encrypt with AES-256-GCM or XChaCha20-Poly1305 from the RustCrypto crates, with
a fresh random nonce per value. Include the room ID as associated data, so
ciphertext copied into another room fails to decrypt. Messages are ordinary
Megolm-encrypted room events.

The only plain record is the space's room ID, kept in account data so every
device finds the space. The homeserver can still see the number of mirrored
rooms, message counts, sizes and timing. That's accepted: the homeserver is the
group's own VPS.

[MSC4362](https://github.com/matrix-org/matrix-spec-proposals/pull/4362) would
encrypt state events in Matrix itself. The pinned SDK has the
`experimental-encrypted-state-events` feature, but the proposal is a draft and
needs homeserver support. Switch to it if it lands; only Talk reads these
events.

### Which device mirrors

Only one Talk instance with a Discord login runs the mirror at a time. The most
recently active linked instance takes a lease, stored in account data with an
expiry, and renews it while it runs. Instances without a Discord login never
take it. Every mirrored event carries its Discord message ID, and writers skip
IDs already in the room, so a takeover during a lease change doesn't post twice.

While no linked Talk instance runs, nothing is mirrored. The next instance to
take the lease fetches the missed messages from Discord.

### Devices without a Discord login

A device without a Discord login shows Discord servers and DMs from the mirror,
with the same badges as a linked device. Its Discord activity becomes custom
request events in the mirrored room: sending, replying, editing, deleting,
reacting and marking as read. The instance holding the lease carries these out
on Discord, then replaces each request with the resulting mirrored event, or
marks it failed with the reason.

Until a linked instance picks a request up, the message shows as waiting, with
the reason "Waiting for a device with Discord". Like Talk's offline queue, the
person can cancel it. Requests carry a client-generated ID, so a lease change
never sends one twice.

Voice, calls and status need a Discord login on the device itself. A device
without one shows Discord voice channels from the mirror, without participants,
and offers to link Discord to join.

### History and storage

Linking a channel to the mirror copies the last 100 messages or 7 days,
whichever is less. After that the mirror keeps everything. On a linked device,
scrolling past the start of the mirror loads older messages from Discord and
adds them to it. Attachments over a size cap stay on Discord: a linked device
loads them when opened, and other devices show a placeholder. Smaller ones are
re-uploaded encrypted to the homeserver.

Devices keep only a bounded local cache of recent messages and attachments, as
for Matrix rooms. Older history loads on demand from the mirror, or from Discord
for channels that aren't mirrored. The mirror is the long-term, encrypted copy
of the history. A new device reads old mirrored messages through key backup
after entering the recovery key, as in other encrypted rooms. Attachments make
the homeserver's disk grow. Synapse's media retention can expire old ones if
that becomes a problem.

A message deleted on Discord is also deleted from the mirror, matching Discord.
A mirroring setting, "Keep messages deleted on Discord", keeps them instead.
It's off by default. Edits keep their earlier versions in Matrix either way.

### Unlinking

Logging one device out of Discord leaves the mirror alone, since other devices
may use it. Turning off mirroring for a channel, or removing Discord from the
account, makes Talk leave and forget the affected rooms. Talk asks first,
because this deletes the history kept on the homeserver. Synapse deletes a room
once nobody is in it.

## Memory

The Discord connection and its server state count towards the same targets as
the rest of Talk: under 250 MiB settled idle and under 400 MiB in a ten-person
call. Large servers carry big member lists, so request members only for the
channel on screen. Measure settled idle and a Discord call with the method in
[measurements](measurements/README.md).

## Testing

`testing/fake_discord.ts` is a local Discord: the gateway, the parts of the
HTTP API Talk uses, a voice server that answers the handshake without DAVE,
Go Live signalling, and the remote login gateway. Set
`TALK_DISCORD_TEST_SERVER` to it, and Talk uses a test token instead of the
linked account, kept in memory. With `TALK_DISCORD_TEST_UNLINKED=1` too, Talk
starts unlinked and links against the fake, whose controls scan and confirm
the code. The fake resumes a dropped gateway session, replaying what it
missed, and can send op 7. `crates/talk-discord/tests/fake_server.rs`
runs against it. It counts the audio a call sends (`/_control/voice-audio`),
which the ignored `discord_audio` test `a_call_sends_what_the_microphone_hears`
checks: Talk joins the Lounge, unmutes and beeps into a microphone that hears
its speakers, as a private PulseAudio of null sinks does. What the fake can't show, such as media and DAVE with
Discord's servers, needs real Discord. Use a separate test account, which the
owner provides, in a private test server. Run these tests by hand, never in CI: repeated automated logins
are what gets accounts flagged. Keep the QR-linked test token in the developer's
OS credential store, like any other Discord login. Matrix-side behavior, such
as the mirror, request events and encrypted names, is tested against the local
Synapse stack with recorded Discord events instead of a live connection.

## Licences

davey and songbird are permissive, so closed-source Talk can use them. GPL
clients such as [Abaddon](https://github.com/uowuo/abaddon) are reference
material only, under the borrowing rules in the
[implementation plan](implementation-plan.md#reference-projects).

## Order of work

1. Prove Discord voice outside the UI. Link by QR code, then join a voice
   channel with DAVE in a command-line harness, like Milestone 1 did for Matrix
   calls. Settle the audio question above here.
2. Voice in Talk: the voice channel list, joining, controls, and the shared
   connection rule.
3. DM and group DM calls with ringing.
4. Live servers, channels, DMs and messages, notifications, read state and
   status.
5. Mirroring into Matrix.

Steps 1 to 4 work on Linux; step 5 hasn't started.

Screen sharing and watching streams in Discord calls (Go Live) came early.
Camera video follows after the beta.
