# Discord in Talk

Status: decisions recorded 2026-10-06. Step 2 of the [order of work](#order-of-work)
works on Linux. Talk lists the linked account's Discord servers in the rail
with a Discord badge and their voice channels. Channels the account's roles
and channel overwrites keep it out of show greyed out with a lock, as in
Discord; channels hidden from it appear the same way only with the "Show
channels you can't access" setting, which is off by default. Clicking a
voice channel joins through the same voice dock and one-call rule as Matrix
voice. Calls start muted, mute and deafen show in Discord, and a voice gate
sends audio only during speech. The
owner and a friend talked through it on 2026-10-06. Audio uses PulseAudio and
libwebrtc's processing module, so Windows needs its own audio backend. The
token still comes from the browser through `talk-discord-probe store-token`;
QR linking and DM calls are still open. songbird's DAVE receive fix is
described in `vendor/songbird/README.talk.md`, and davey's change for Go Live
groups in `vendor/davey/README.talk.md`.

Server text channels from step 4 are in progress. They're listed under
their Discord categories and open in Talk's timeline. History loads from
Discord 50 messages at a time, and new messages, edits, deletions and
reactions arrive live. Opening a channel subscribes to its server, as the web
client does, because Discord sends large servers' messages only to
subscribers. Talk sends, replies, edits, deletes, reacts and sends files
through Discord's cloud uploads. Mentions, custom emoji and timestamps show as
text, every attachment shows, images with previews scaled by Discord's media
proxy, and channels the account can't send in are read-only. An author's
name takes their highest coloured role's colour, once their roles are known:
live messages carry them, and for history Talk asks Discord for the authors'
member profiles.

Talk keeps the open channel's messages in memory, and those of the 16
channels open before it, so going back to one shows it at once. A closed
channel keeps its newest 100 messages and the previews of its newest 12
images (a decoded preview is about 400 KB), and follows new messages, edits,
deletions and reactions while it's kept. Opening it again loads the latest
page in the background and fits it in: messages kept within the page's span
that it lacks were deleted meanwhile, and when a full page starts after
everything kept, more came than a page holds, so only the page stays. The
UI keeps the same 16 channels' last timeline and member list, so going back
shows them before the chat side answers, without "Loading messages...".
Nothing is written to disk.

Unread dots and mention counts follow Discord's read states, mutes and
@everyone and role suppression, including passive updates from large servers.
Showing an unread channel marks it read, and with the "Mark messages read on
Discord" setting, on by default, acknowledges it on Discord within a second.
The open text channel lists its members on the right as Discord's sidebar
does: under hoisted roles, then online, then offline, with offline members
faded. Each shows a status dot for online, idle or do not disturb, their name
in their highest coloured role's colour, and their custom status or what
they're playing below it. Talk shows the list's first 100 rows, headings
included. The member lists of the last 16 channels shown are kept too: going
back to one shows its list at once, and Discord's fresh sync replaces it, even
under another list id, as after the channel's readers changed. A build without Matrix shows the Discord account's name, avatar and
connection in the bottom-left panel.

DMs and group DMs are listed under the DM button after Matrix DMs, the latest
conversation first as in Discord, with the other person's avatar or the
group's icon and a Discord badge. They open, send and keep their messages
like server channels, under the room id `discord:@me:<channel>`, which takes
`@me` for the server as Discord's links do. Every unread message in a DM
counts as a mention, and a muted DM shows neither, as in Discord. A DM
without a read state counts as read, since Discord keeps one for each DM
with something to read. A group DM lists its members by name, with the
presence of those who are friends. Talk downloads the pictures of the first
50 DMs listed; the rest keep their initials.

Message requests, from people outside the account's DMs, are listed at the
top of the DM list under "Message requests", and those Discord marks as spam
at the bottom under "Spam", so a person Discord misjudged can still be found.
Opening one shows its messages with Accept and Ignore in place of the
composer: replying waits for accepting, as in Discord. Accepting makes it an
ordinary DM; ignoring removes it, and the sender isn't told. Requests count
no mentions, spam isn't marked unread, and Talk never opens a request on its
own when it picks a conversation. Forwarding and DM calls are still open.

The pen in the DM list's header starts a DM: it lists the account's friends
(from READY's relationships), then anyone else Talk knows by name, and
opening one posts to `/users/@me/channels`, which finds the DM with them or
starts one. Talk opens it once its CHANNEL_CREATE lists it.

Clicking someone in a channel's member list shows their profile in a small
card beside it, which "View full profile" enlarges into a centred one;
right-clicking a DM and choosing "Profile" opens the full one. Talk fetches it
from `GET /users/{id}/profile` with the open server's id, so the server's
nickname, avatar, banner, bio and pronouns win over the person's own, and the
card lists their roles in role order, when they joined Discord and the server,
and the servers and friends the account shares with them. Profiles are kept for
five minutes, at most 32 of them. Presence and activity come from the member
list, since the profile doesn't carry them. When Discord refuses, the card
keeps what the member list knew and says so. Matrix members and one-to-one
Matrix DMs get the same card with their name, Matrix ID, avatar and room role.

Talk connects to Discord directly, so a person who links their Discord account
doesn't need the Discord desktop app. Talk joins Discord voice as that person,
shows their servers, channels and DMs, and sends and receives messages. This is
part of the private beta. Talk must still be complete without Discord: nothing
Discord-related appears until someone links an account, and Discord work doesn't
replace beta features of Talk itself.

[PRODUCT.md](../PRODUCT.md#discord) records the agreed scope. This document
describes how to build it.

## Risk to accounts

Discord's terms don't allow third-party clients to use personal accounts.
Bans are uncommon, but they happen, and Discord can change its protocol and
break Talk at any time. Linking is opt-in per person. Talk shows this warning
before linking, and each person decides for their own account.

Behave like the official client to keep the risk low. Respect rate limits,
never act without a user action behind it, and don't poll. Keep all Discord code
in one crate, so a protocol change breaks only that part and Talk can disable it
without affecting Matrix.

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
app. Pasting a token and email/password login aren't offered. Discord may still
ask for a captcha, so plan a fallback through a small embedded web view.

Store the Discord token in the OS credential store, next to the Matrix session.
The token gives full control of the account, so it never goes into Matrix, not
even encrypted. A device that links appears in Discord as its own session.
Devices that only use the mirror don't need to link.

Logging a device out of Discord ends its session and deletes its token. Removing
the mirrored rooms is a separate step, described under [Unlinking](#unlinking).

## Voice

Scope for the beta:

- Server voice channels: join, leave, a participant list, speaking indicators,
  mute, deafen and per-person volume.
- DM and group DM calls, both incoming with ringing and outgoing.
- One connection at a time across Talk and Discord. Joining Discord voice asks
  to leave a Talk call, and the other way round, as in PRODUCT.md.
- The same devices, push-to-talk, voice activation, noise suppression and echo
  cancellation as Talk calls.

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
Discord voice gets a small path of its own: `cpal` for the microphone and
speakers at 48 kHz, the processing module for echo cancellation, noise
suppression and gain, then songbird for Opus and DAVE. Feed the speaker mix to
the processing module's reverse stream so echo cancellation has its reference.

Check one linking risk before building on this. songbird encodes with `opus2`,
which builds its own libopus, while Talk's libwebrtc archive already contains
libopus. Linking both into Talk may clash, particularly on Windows MSVC.

### Screen sharing

Go Live streams come through Discord's gateway: op 18 starts sharing, op 20
watches someone, op 22 pauses and op 19 ends either. Discord answers with
STREAM_CREATE, naming the RTC server, then STREAM_SERVER_UPDATE with an
endpoint and token. The stream gets a voice connection of its own to that
server, reusing the call's voice session id. `talk_discord::stream` names
streams by key (`guild:{server}:{channel}:{user}`) and
`Discord::{go_live, watch, end_stream, pause_stream}` run the signalling. Voice
states carry `self_stream`, which shows as a LIVE badge beside the person.

Capture runs in a worker process, `talk --stream-worker`, so its buffers take
memory only while sharing; the process exits when sharing stops.
`stream_worker` starts it with one job as a JSON line and reads its reports
back. Frames reach the UI through `frames`, a triple buffer in a memory-mapped
file in the runtime directory. The file is unlinked once the worker opens it.
On Linux, libwebrtc's capturer asks the screen sharing portal what to share.
The portal answers over D-Bus through GLib, so the worker runs GLib's main
context while it waits for the dialog and for frames. Talk tells Discord it's
live only once frames flow, so a cancelled dialog shares nothing.
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
the call echoed back (`stream_audio`). On Linux, PulseAudio or PipeWire's
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
sent while no application plays. Other systems share video only for now, and
so does the test pattern. Two ignored tests check the capture against the
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
through PulseAudio on the call's speakers (`stream_audio::Player`), about 100
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

- Discord servers appear in the server rail with a Discord badge, and Discord
  DMs appear in the DM list with the same badge.
- Server icons and avatars come from Discord's CDN, which needs no login.
  Talk asks for each one once it shows: in the rail, beside people in a
  voice channel of the open server, in the call, or beside a message. A
  member's server avatar wins over their own. Talk decodes them at 128px
  before the UI sees them. Initials show until a picture arrives or when it
  fails. A server without an icon keeps its initials, and a person without
  an avatar gets Discord's default avatar for their account.
- Messages support text, replies, edits, deletions, reactions, mentions and
  attachments.
- Link previews and bots' cards show from the embeds Discord adds to a
  message: Discord's servers read the linked page's Open Graph tags, often a
  moment after sending, and the preview arrives in a message update. A card
  shows the site, author, title, description, fields and footer behind a bar
  of its colour, with its picture large for articles and videos and small
  beside the text otherwise. A video's picture opens the video in the
  browser; other pictures open in the image viewer. A linked image or GIF
  shows with the message's images, without its address when that's all the
  message says. GIFs show their first frame. Pictures come through Discord's
  media proxy (`images-ext-*.discordapp.net`), so Talk never contacts the
  linked site, and embeds of a message marked to hide them don't show.
- Desktop notifications follow the person's Discord settings: each channel's,
  category's or server's level (every message, mentions only or nothing),
  falling back to the server's default, with mutes leaving only mentions. DMs
  notify unless muted; message requests and spam don't. They show while Talk
  isn't in front, titled with who wrote where, and clicking one opens the
  conversation. On Linux they go to the freedesktop notification service.
- Mentions in messages are links: a person's opens their profile card, and a
  role's lists the members Talk has seen with it (from member lists, messages
  and member requests), each opening their card in turn.
- Typing `@` suggests who to mention: the channel's listed members, those who
  wrote lately, and server members a search by name finds (the gateway's
  member request with a query, as Discord's client does), then the roles
  anyone may mention, and every role with @everyone and @here for those
  allowed to mention everyone. Sending turns each `@Name` into Discord's
  `<@id>` or `<@&id>`.
- Who else is typing shows under the composer, for ten seconds after Discord
  last said so or until their message arrives. Writing tells Discord Alice is
  typing, at most every eight seconds, unless a setting turns it off.
- Reading a message in Talk marks it read on Discord, so the phone app agrees.
  A setting turns this off.
- Talk sets the online, idle and do-not-disturb status on Discord. Closing Talk
  takes the person offline, like closing the Discord app.

Later: threads, forum channels, stickers, polls, soundboard, activities, friend
requests, joining servers by invite, and server administration. The browser
covers these until then.

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
and Go Live signalling. Set `TALK_DISCORD_TEST_SERVER` to it, and Talk uses a
test token instead of the linked account. `crates/talk-discord/tests/fake_server.rs`
runs against it. What the fake can't show, such as media and DAVE with
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

Screen sharing and watching streams in Discord calls (Go Live) came early.
Camera video follows after the beta.
