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
  ONLINE and OFFLINE, with counts, avatars, status dots, role colours,
  custom statuses, and a red clock beside someone timed out. Talk subscribes to the list's
  first 100 rows while the channel is open, and Discord keeps them current.
- Read live text messages. Earlier history loads as the timeline scrolls
  within a screen of its top, a page at a time (30 events for Matrix, 50
  messages for Discord), until the conversation's first message, which "This
  is the start of #general." marks. While a page is on its way a pill above
  the oldest message says "Loading earlier messages..."; if it fails, the pill
  offers Retry, and pages load again only when asked. The messages shown
  stay where they are on screen as older ones go in above them, even when
  the top of the history is on screen; see
  [building](building.md#dependencies).
- Jump to a message from a reply's quote, a pin or a search result. One
  that's loaded scrolls a third of the way down and is highlighted for a
  moment. Otherwise the messages around it load in place of the timeline,
  as a window of older messages: Discord's 50 around it, Matrix's 30 around
  it through the SDK's event-focused timeline (`/context`). A bar over its
  end says "You're viewing older messages" with "Jump to present", and
  scrolling near its end loads newer pages, with "Loading newer messages..."
  under the last one, as scrolling up loads earlier ones. New messages don't
  go in the window, and the conversation stays unread. Jump to present,
  sending a message, or a newer page that reaches the newest shows the
  newest again, as Talk kept them, and leaving the conversation lets the
  window go. A jump to a conversation that isn't open opens it at the
  message.
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
  shows just above the composer, gently pulsing, after three dots under the
  avatars that rise in turn, and Talk tells the room while you type unless a
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
  it read. A conversation opened with unread messages opens at a marigold
  "New" divider above the first one, sitting at the top of the view rather
  than at the newest message, and the author's name shows again below it. The
  divider stays put while the conversation is open, as newer messages are
  read as they come, and it's gone the next time once the conversation was
  read. While the divider is out of view, a marigold bar at the top says how
  many messages are new and since when ("80 new messages since 14:44"), with
  an arrow toward it, and clicking it scrolls back to the divider. A Discord
  channel places it by message order; when every loaded message is unread,
  it opens on a window of the messages around the last read one, where the
  bar counts those loaded with a plus ("50+ new messages"). A Matrix room
  places it after the event its read receipt is on, once that event is in
  the loaded timeline.
- Highlight messages that call on the user with a faint marigold tint and a
  marigold edge on the left, as Discord does: a mention of them, of a Discord
  role they have, or @everyone or @here where those reach them, always, and a
  reply to their message while it's new. New is from the "New" divider down,
  and what comes while the conversation is open; a conversation opened with
  nothing unread counts from its newest message, and the thread beside a
  channel from its own last read message. A reply that pings only lists
  them as mentioned, so it counts as a mention only when its text names
  them. Matrix's push rules decide which messages mention the user; a reply
  stays a mention when its text has their user ID or a link to it, or
  @room. Their own messages are never highlighted. Talk counts unread messages on this device; for a room that isn't open,
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
  Hovering shows the server's name. A right click on a server, or the Menu
  key or Shift+F10 on a focused one, opens its menu: Mark as read when any of
  its channels is unread, then Copy link and Copy ID. A Discord server's
  menu also has, while Discord is connected, Invite to server where the
  account may invite; Mute server, whose submenu
  mutes for the DM menu's times, or Unmute server with when the mute ends;
  Notification settings, whose submenu picks All messages, Only @mentions
  or Nothing, with the level that applies under it, and ticks Suppress
  @everyone and @here and Suppress all role @mentions; Hide muted
  channels, a box to tick; Edit server profile, which opens the member
  menu's nickname dialog for one's own nickname there, where the account
  may change it; where the account may manage channels, Create channel
  and Create category; and, in red, Leave server, which asks first and
  isn't offered to the server's owner.
- A sidebar, 240px by default and resizable by dragging its edge, with the
  server name, a filter, and uppercase TEXT CHANNELS and VOICE CHANNELS groups.
  A Discord server's name, with a chevron beside it, opens a dropdown below
  it, as in Discord, and the chevron turns over while it's open; Enter, Space,
  the Menu key or Shift+F10 open it too. A person with a plus, on the
  header's right, invites to the server. The dropdown lists the choices of the
  server's menu that Discord's dropdown has, in its order, from the same list
  (`Application::header_choices`), so the two can't disagree. A Discord
  channel list heads each category's channels with its name; a category
  without channels shows its heading alone, unless its permissions hide it.
  A right click on a heading, or the Menu key or Shift+F10 on it, opens the
  category's menu: Mark as read when its channels have messages to read,
  its mute and notification settings (below), Edit category and Delete
  category where the account may manage it, and Copy ID. The heading
  names its first channel, or an empty category's own id, and Talk finds
  the category from that.
  DMs show the person's avatar. Participants of the call you are in appear under
  its voice channel, with a green ring while they speak and a muted icon; for
  Discord, who is in every other voice channel shows too, with a LIVE tag for a
  screen share and a camera icon. Clicking someone sharing their screen opens
  their channel's call and watches them. Outside that call it asks first, "Join
  <channel> to watch <name>'s stream?", or in another call the switch prompt
  asks, and the stream opens once the call connects and shows them sharing.
  Cancelling, leaving, a failed join or opening another room forgets it.
  Clicking someone with only a camera on opens the call's view, which offers
  Join if you aren't in it. A right click on a conversation, or
  the Menu key or Shift+F10 on a focused one, opens its menu: Mark as read when
  it's unread, Join voice for a voice channel you aren't in, Profile for a
  one-to-one DM, Pin or Unpin and Start call for a DM, then Copy link and Copy
  ID; a Discord channel's adds Invite to channel before Copy link, and
  where the account may manage it Edit channel, Duplicate channel, Create
  text (or voice) channel, which goes in its category, and Delete channel
  after it. A pinned
  DM stays at the top of the list, with a pin, on this device. A
  Discord DM's menu also has Add note, Add or Change friend nickname, Close DM,
  Remove friend, Ignore and Block, each undone by its opposite, and Mute, whose
  submenu mutes for 15 minutes, 1, 3, 8 or 24 hours, or until unmuted. A
  Discord server's text channel offers "Mute channel", with the same times,
  or "Unmute channel", and "Notification settings", whose submenu picks which
  messages notify; a right click on a category's heading offers the same for
  the category. A muted
  DM or Discord channel shows faint, and a Discord server set to hide muted
  channels leaves them out, but for the open one and those that mention you.
  Removing a friend and blocking ask first. Matrix links go through
  matrix.to. Arrow keys, Home and End move through the menu, Right opens a
  submenu and Left leaves it, Enter chooses, and Escape closes it.
- Inviting opens a dialog with a link Discord made at once, lasting 7 days
  and working any number of times, a button that copies it, and a line on
  how long it lasts. Choices for how long a link lasts (30 minutes to 7
  days, or for good) and how many may use it (any number, or 1 to 100), as
  Discord's dialog offers them, make another with Generate a new link.
- Creating a channel asks for its name and, as Discord's dialog does,
  whether it's for text or voice; a category asks only for a name. Editing
  a text channel loads its name, topic, slowmode, stepped through Discord's
  choices from off to 6 hours, and age restriction, and saves them with
  Save changes; a voice channel or a category edits only its name.
  Duplicating asks for the copy's name. Deleting asks first, in red. Text
  channel names become lower case with dashes, as Discord makes them, and
  a text channel made opens. The dialogs are in `ui/server.slint`.
- Menus (`ContextMenu` in `ui/controls.slint`) are as wide as their rows'
  text, from 188 to 320px, and each row may open a submenu of its own,
  beside the menu, or on its left near the window's right edge. A row can
  carry a second line under its text, such as the level a notification
  submenu has chosen, a check in place of its icon for the current choice,
  or a box to tick. Ticking leaves the menu open. A row can show greyed, as a
  role this account may not give does, and can't be chosen. A menu or
  submenu taller than the window scrolls, and the keys keep their row in
  sight. `src/ui/menus.rs` builds a menu's rows with each row's submenu, and
  ticks a box when it's chosen.
- A dock across the rail and sidebar: the voice panel, then the account panel
  with avatar, status, mute and deafen during a call, and settings (account,
  audio devices, message text size, clock and link previews, notifications,
  sounds, reconnect, sign out). A caret beside mute and deafen opens a
  device menu; the input menu shows a local level meter.
- A conversation with a 48px header: the conversation's icon and name, and
  for a Discord server channel its topic on one line after a rule, which a
  click shows in full. Its buttons sit on the right, as in Discord: a DM's
  call, pins, adding friends, member list or profile, and search; a server
  channel's bell, pins, member list and search. The bell opens the channel's
  mute and notification level, and shows slashed while it's muted. A server
  channel's search looks through the whole server, takes Discord's filters
  such as `from:`, `in:` and `has:`, heads its results with their channels
  and stays open in them. Messages from one person within seven
  minutes are grouped under one avatar and name. Hovering a grouped message
  shows its time in the gutter. Replies draw a spine to the quoted message, and
  a date divider starts each day. The top of the history shows what's
  before it: earlier messages loading, a way to retry them, or the start of
  the conversation. The composer is a rounded field with attach, the
  message ("Message #general"), and a send button; reply and edit modes attach
  a bar above it. With no conversation selected, the area shows "Pick a
  conversation" and hides the composer. The local preview labels itself in a
  strip under the header.
- A 240px member sidebar right of the conversation, under its header, on the
  sidebar's tone. The member count in a group DM's header shows or hides it,
  as the people icon does in a server channel's, bright while the list
  shows, and Talk remembers a closed list. While it shows, dragging the left sidebar still
  leaves the conversation 420px.
- A right click on someone, in the member list, on a message author's avatar
  or name, or in the friends list, opens their menu: Profile, Message to open
  or start the DM with them, and on Discord Add friend, Accept friend request
  or Cancel friend request, then Copy ID. In a Discord server's channel it's
  their menu as a member, as Discord's: Mention, Call, a note and nickname,
  their roles in a submenu of checkboxes, and Time out, Kick and Ban last, in
  red, as this account's permissions and its rank against theirs allow; see
  [Discord](discord.md#a-server-members-menu). Time out and Ban ask how long
  or how much in their dialog, and each asks a reason. In a one-to-one DM, the
  other person's menu is the DM's own, less Mark as read and Copy link, and
  Copy ID copies theirs. Clicking an author's avatar or name, which underlines while
  pointed at, shows their card beside it.
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
creation, mentions with autocomplete,
threads, a full emoji picker, Matrix video and audio playback, drag and drop, and deleting
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
