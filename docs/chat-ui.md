# Chat UI

The first chat UI was implemented on 2026-10-02 and moved to Slint on
2026-10-03. The Milestone 2 conversation features followed on 2026-10-03; they
add the SDK's Markdown feature, `image` for bounded decoding, `mime`,
`pulldown-cmark` for plain reply snippets, and `rfd` for native file dialogs.
The same day the window was redesigned after Discord's layout, set in Satoshi.

## Try it

From the repository folder:

```sh
nix develop path:./nix --command cargo run --locked -- --preview
```

The preview has two sample servers, DMs and a group DM. The upper-left button
opens DMs; buttons beneath it select servers. The sidebar shows only that server's
channels, or only DMs when the DM button is selected. Filter the current list and
send messages locally. The gear in the account panel opens settings, where "Back
to sign in" opens the real account form. Samples and entered messages disappear on leaving
the preview. They never connect to Matrix.

Launch without `--preview` for password sign-in and automatic restoration.
`--data-dir PATH` selects an isolated session directory. Passwords and recovery
keys are masked and cleared after submission. SSO and OAuth are still pending.

## Implemented behavior

- Show existing DMs behind the upper-left button and joined Matrix Spaces as
  servers below it. Scope the channel list to the selected server, with a name
  filter that resets when switching servers or DMs.
- Flatten channels in joined subspaces, deduplicate shared links, and tolerate
  cyclic Space relationships. A shared channel can appear in multiple servers.
  Removed child links update the channel list during sync.
- Restore each server's last selected channel and the last selected DM. Talk
  keeps each server's channel in `layout.json`, so after a restart a server
  opens its last channel; a server opened for the first time shows its top one.
- List the joined members of the open text channel or group DM on the right,
  grouped under ADMINS, MODERATORS and MEMBERS by Matrix power level (100 and
  50), each by name. The first view fetches the full list from the homeserver,
  since sync loads members lazily, and membership changes refresh it. Voice
  channels and one-to-one DMs have no member list; a one-to-one DM shows the
  other person's profile there instead, which the person icon in its header
  hides or shows. A Discord text channel
  lists whoever can read it, grouped as in Discord: under hoisted roles, then
  ONLINE and OFFLINE, with counts, avatars, status dots, role colours and
  custom statuses. Talk subscribes to the list's
  first 100 rows while the channel is open, and Discord keeps them current.
- Read live text messages and load history in batches of 30.
- Recover encryption keys or recheck verification performed in another client.
- Send only from a cross-signed device into a joined encrypted room. Existing
  plaintext rooms are read-only.
- Write Markdown. Messages with formatting render bold, italic, code and links;
  Slint's subset has no headings, so those show as typed. Formatted text isn't
  selectable, so every message has a Copy action.
- Start a DM from the pen in the DM list's header: type a name to search the
  homeserver's user directory, or a full Matrix id. Talk opens the DM you
  already have with them, or starts an encrypted one and opens it once sync
  lists it.
- Use the keyboard for messages: Up in an empty composer edits your last
  message; Alt+Up moves into the messages, where arrows step, Enter opens the
  menu, + reacts, R replies, E edits, Delete deletes, C copies, F forwards and
  Escape returns to the composer.
- Clicking a mention, a link to a Matrix user, opens their profile card.
- Mention someone by typing `@` and picking from the room's members. The
  message links to them (`[Name](https://matrix.to/#/@user:server)`) and lists
  them in `m.mentions`, so their client notifies them. Who else is typing
  shows just above the composer, gently pulsing, and Talk tells the room while you type unless a
  setting turns it off.
- Hover or click a message for its toolbar: your three most used reactions,
  Add reaction, Edit on your own message or Reply on someone else's, and More.
  More, or a right click on the message, opens a menu with your four most used
  reactions, Add reaction, Edit, Reply, Forward, Copy text and Delete; Edit and
  Delete appear on your own messages only. Until you have reacted, the most used
  reactions are 👍, ❤️, 😂 and 🎉. Talk counts the reactions you add, not the
  ones you remove, in `layout.json` on this computer. The icons have tooltips and
  accessible names. Add reaction opens a picker with a search field, frequently
  used emoji and every category of the `emojis` crate up to Unicode 15.0, which
  Noto Color Emoji and Windows 11 draw; Enter picks the first match. Clicking an
  existing reaction adds or removes yours.
- Forward a sent message to any encrypted DM or channel from a dialog with a
  search field; each conversation can receive it once per forward. Talk sends
  the message's current content as a new message, without its reply quote. An
  encrypted attachment keeps its uploaded file, and its key travels in the new
  encrypted event. Delete asks for confirmation, then
  redacts the message for everyone. Replies quote the original; edits are marked
  "(edited)". Reply and edit modes show above the composer, with Cancel.
  Cancelling an edit restores the draft it replaced.
- Attach files with the system file dialog, or paste them with Ctrl+V: files
  copied in a file manager, or a picture copied from a browser or a screenshot
  tool. They wait above the text as tiles, with a preview for pictures and an
  X to take each out, until the message is sent; text can be typed beside
  them, and the message may be the files alone. A message carries up to ten
  files. Each must fit the conversation's limit, and a file that doesn't is
  refused with its size and the limit. On Wayland the clipboard is read
  through the data-control protocol, so a picture keeps its copied bytes and a
  GIF stays animated; elsewhere arboard reads it, and pixels are saved as PNG.
  Pasted pictures are saved as `image.png` in a temporary folder removed when
  Talk closes. Text on the clipboard pastes as text. In Matrix the SDK encrypts
  and uploads each file through its send queue, up to 50 MB, then sends the
  text after them. Images show a decoded preview of up to
  360×270, decoded with dimension and memory limits, with a save button over the
  picture on hover. Other files show a card with a save button. Saving writes the
  decrypted file where you choose. A Discord message with several images shows
  them as square tiles, two to a row.
- Click an image to see it large over the window. It opens at the image's full
  proportions with the preview, then sharpens when Talk has downloaded it at up
  to 1920×1200. The left and right arrow keys or the side buttons step through
  the message's other images, a counter shows which one it is, and Escape, the
  close button or a click outside the image closes it. It can save the image
  too.
- Send through the SDK's persistent send queue. A message appears at once with
  "Sending...". After a connection problem it shows "Not sent yet" and goes out
  by itself when sync recovers. Sync now uses the SDK's offline mode, which keeps
  probing the homeserver. A refused message shows "Not sent" with Retry. Queued
  messages can be cancelled, and they survive closing Talk.
- Save each room's draft in the encrypted session store, so drafts survive
  restarts. Sending clears the saved draft.
- Show unread channels in bold with a short pill at the sidebar edge, and the
  same pill beside servers and the DM button that contain them. Mentions show a
  red count. Opening a conversation marks
  it read. Talk counts unread messages on this device; for a room that isn't open,
  sync often delivers only the newest event, so Talk shows that something is
  unread rather than how much.
- Show sync status in the account panel. Reconnect appears there when offline;
  sign out (which retains encrypted stores) is in settings.

## Layout and look

Ctrl with + or -, or with the scroll wheel, zooms the whole window, text and
layout together, from 50% to 200%: 5% a step up to 150%, then 10%. Ctrl+0
returns to 100%. The level shows
for a moment at the top and is kept in `layout.json`. Talk multiplies the
display's scale factor by it, so the window lays out in fewer logical pixels,
and zooming in stops before those would fall below the window's 800×560
minimum, which would make it grow.

The window follows Discord's arrangement:

- A 72px server rail. Each server is a circle that becomes a rounded square,
  with a white pill at the rail edge for hover, selection or unread activity.
  Hovering shows the server's name.
- A sidebar, 240px by default and resizable by dragging its edge, with the
  server name, a filter, and uppercase TEXT CHANNELS and VOICE CHANNELS groups.
  DMs show the person's avatar. Participants of the call you are in appear under
  its voice channel, with a green ring while they speak and a muted icon. Talk
  knows only its own call's participants.
- A dock across the rail and sidebar: the voice panel, then the account panel
  with avatar, status, mute and deafen during a call, and settings (account,
  audio devices, message text size, clock and link previews, notifications,
  reconnect, sign out). A caret beside mute and deafen opens a
  device menu; the input menu shows a local level meter.
- A conversation with a 48px header. Messages from one person within seven
  minutes are grouped under one avatar and name. Hovering a grouped message
  shows its time in the gutter. Replies draw a spine to the quoted message, and
  a date divider starts each day. An "Earlier messages" pill sits above the
  oldest loaded message. The composer is a rounded field with attach, the
  message ("Message #general"), and a send button; reply and edit modes attach
  a bar above it. With no conversation selected, the area shows "Pick a
  conversation" and hides the composer. The local preview labels itself in a
  strip under the header.
- A 240px member sidebar right of the conversation, under its header, on the
  sidebar's tone. The member count in the header shows or hides it, and Talk
  remembers a closed list. While it shows, dragging the left sidebar still
  leaves the conversation 420px.
- A voice stage with participant tiles, a green border while someone speaks,
  and round mute, deafen and leave controls.

Atkinson Hyperlegible Next (SIL Open Font License, embedded in the binary; see
`ui/fonts/README.md`) sets all text in four weights. Satoshi, the second choice,
stays in `ui/fonts` with its imports commented out. The palette is layered graphite with one
marigold accent for selection and primary actions, red only for mentions and
destructive actions, and green only for live voice. Avatars get one of eight
tints from the person's name.

Network operations run on the existing two-worker Tokio runtime. The UI receives
SDK events, and the message list virtualizes its rows. Rows keep their place by
message id: a change rebuilds only the rows of the messages it changed, and
older messages go in above the rows shown, which stay where they are on screen.
There is no UI polling loop.
Only joined Spaces and their joined channels appear. DMs stay in the DM list even
if linked to a Space. Standalone rooms outside Spaces, channel discovery, room
creation, mentions with autocomplete, jumping to the first unread message,
threads, a full emoji picker, video/audio playback, drag and drop, and deleting
other people's messages remain later work. Message actions need a pointer;
keyboard access to them is still missing. Closing the last window exits Talk.

## Validation

The local chat integration tests create disposable accounts:

- `encrypted_chat_restore_send_retry_and_security_gates` checks restoration,
  live delivery, wire encryption and decryption, plaintext-room and
  unverified-device refusal, logout cleanup, server channel lists, nested and
  cyclic Spaces, and live channel removal.
- `markdown_replies_reactions_edits_attachments_and_drafts` sends Markdown, a
  reply, a reaction and its removal, an edit and a deletion. It uploads an image
  and checks the server only stores an encrypted event with an encrypted file,
  that the preview decodes, and that Save writes the original bytes. It also
  sends a text file and checks that a draft survives a restart.
- `offline_messages_wait_survive_restart_and_send_once` cuts the network with a
  local relay, sends and cancels messages offline, closes and restarts Talk,
  then restores the network. The queued message went out 0.6 s later, once; the
  cancelled one never reached the server.
- `opening_a_room_shows_messages_missed_while_it_was_closed` checks that eight
  messages from another account appear in order after reopening the room, and
  that the room then reads as read.

Run them after `scripts/test-stack up`:

```sh
cargo test --locked -p talk-core matrix_chat::tests -- --ignored --test-threads=1
```

On 2026-10-03 the conversation features were also driven through the real
window on the private desktop with two fixture accounts. Hover actions, the
reaction picker, a reaction chip, a reply, an edit, delete confirmation, the
Markdown and link rendering, an image preview, file cards, and the unread dot
clearing on open all worked. The native file dialogs need an XDG portal, which
the private desktop doesn't run, so Attach and Save were checked only through
the integration test. Slint's Skia renderer drew nothing for the COLRv1 build
of Noto Color Emoji; the CBDT build, which most distributions ship, rendered in
color. Fontconfig can prefer the COLRv1 build when both are installed, which
left reactions blank. On Linux Talk now finds an installed emoji font with CBDT
or sbix bitmaps and no COLR table, maps it and registers it with Slint as "Talk
Color Emoji". That font also leads the sans-serif and UI fallback lists, ahead
of display fonts that carry a few monochrome emoji, so emoji in message text
draw in color too; symbols the UI typeface lacks that have an emoji form, such as ♥,
then show as emoji. Windows uses "Segoe UI Emoji".

Linux native checks cover sign-in, restoration, device trust, encrypted sending,
local preview input, Enter submission, room switching, draft retention, DM and
name filtering, sign-out, and resize. The navigation refinement checks switching
between two servers and DMs, scoped channels, resetting filters, and restoring the
last channel and its draft. Captures use 1120 by 760 and 800 by 560.
The sign-in form scrolls when errors make it taller than the minimum window.
Windows build and rendering still require the tester's machine. The previous
empty-window resource baseline does not measure this chat UI.

The navigation refinement was checked on Linux with two preview servers. Switching
to DMs hid server channels; switching servers replaced the channel list. Returning
to Friends restored `games` and its draft, and returning to Co-op crew restored
`looking-for-group` and a separate draft. A `games` search cleared on leaving
Friends. The composer and both navigation columns stayed usable at 800 by 560.
Review captures are in `.impeccable/review/nav-*.png`.
