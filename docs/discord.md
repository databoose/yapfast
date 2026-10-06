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
described in `vendor/songbird/README.talk.md`.

Server text channels from step 4 are in progress. They're listed under
their Discord categories and open in Talk's timeline. History loads from
Discord 50 messages at a time, and new messages, edits, deletions and
reactions arrive live. Opening a channel subscribes to its server, as the web
client does, because Discord sends large servers' messages only to
subscribers. Talk sends, replies, edits, deletes, reacts and sends files
through Discord's cloud uploads. Mentions, custom emoji and timestamps show as
text, every attachment shows, images with previews scaled by Discord's media
proxy, and channels the account can't send in are read-only. Talk keeps only
the open channel's messages, in memory.

Unread dots and mention counts follow Discord's read states, mutes and
@everyone and role suppression, including passive updates from large servers.
Showing an unread channel marks it read, and with the "Mark messages read on
Discord" setting, on by default, acknowledges it on Discord within a second.
The open text channel lists its members on the right as Discord's sidebar
does: under hoisted roles, then online, then offline, with offline members
faded. Each shows a status dot for online, idle or do not disturb, their name
in their highest coloured role's colour, and their custom status or what
they're playing below it. Talk shows the list's first 100 rows, headings
included. A build without Matrix shows the Discord account's name, avatar and
connection in the bottom-left panel. Forwarding,
typing, desktop notifications and DMs are still open.

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

Video and watching screen shares come after the beta, together with video in
Talk. The group uses screen sharing often, so this is the first thing to add
after the beta.

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
- Notifications follow the person's Discord settings, including per-server mutes
  and "mentions only".
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

There's no local Discord server to test against, so Discord tests need real
Discord. Use a separate test account, which the owner provides, in a private
test server. Run these tests by hand, never in CI: repeated automated logins
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

Video and screen sharing follow after the beta.
