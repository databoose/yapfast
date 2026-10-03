# Chat UI

The first chat UI was implemented on 2026-10-02 and moved to Slint on
2026-10-03. The Milestone 2 conversation features followed on 2026-10-03; they
add the SDK's Markdown feature, `image` for bounded decoding, `mime`,
`pulldown-cmark` for plain reply snippets, and `rfd` for native file dialogs.

## Try it

From the repository folder:

```sh
nix develop path:./nix --command cargo run --locked -- --preview
```

The preview has two sample servers, DMs and a group DM. The upper-left button
opens DMs; buttons beneath it select servers. The sidebar shows only that server's
channels, or only DMs when the DM button is selected. Filter the current list and
send messages locally. "Back to sign in"
opens the real account form. Samples and entered messages disappear on leaving
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
- Restore each server's last selected channel and the last selected DM.
- Read live text messages and load history in batches of 30.
- Recover encryption keys or recheck verification performed in another client.
- Send only from a cross-signed device into a joined encrypted room. Existing
  plaintext rooms are read-only.
- Write Markdown. Messages with formatting render bold, italic, code and links;
  Slint's subset has no headings, so those show as typed. Formatted text isn't
  selectable, so every message has a Copy action.
- Hover or click a message for its actions: React, Reply, Edit and Delete for
  your own messages, and Copy. Reactions offer six quick emoji. Clicking an
  existing reaction adds or removes yours. Delete asks for confirmation, then
  redacts the message for everyone. Replies quote the original; edits are marked
  "(edited)". Reply and edit modes show above the composer, with Cancel.
  Cancelling an edit restores the draft it replaced.
- Attach a file with the system file dialog. The SDK encrypts and uploads it
  through its send queue, up to 50 MB. Images show a decoded preview of up to
  360×270, decoded with dimension and memory limits. Other files show a card.
  Save writes the decrypted file where you choose.
- Send through the SDK's persistent send queue. A message appears at once with
  "Sending...". After a connection problem it shows "Not sent yet" and goes out
  by itself when sync recovers. Sync now uses the SDK's offline mode, which keeps
  probing the homeserver. A refused message shows "Not sent" with Retry. Queued
  messages can be cancelled, and they survive closing Talk.
- Save each room's draft in the encrypted session store, so drafts survive
  restarts. Sending clears the saved draft.
- Show unread channels in bold with a dot, and a dot on servers and the DM
  button that contain them. Mentions show a count. Opening a conversation marks
  it read. Talk counts unread messages on this device; for a room that isn't open,
  sync often delivers only the newest event, so Talk shows that something is
  unread rather than how much.
- Show sync status, reconnect, and sign out while retaining encrypted stores.

Network operations run on the existing two-worker Tokio runtime. The UI receives
SDK events, and the message list virtualizes its rows. There is no UI polling loop.
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
cargo test --locked -p talk-core chat::tests -- --ignored --test-threads=1
```

On 2026-10-03 the conversation features were also driven through the real
window on the private desktop with two fixture accounts. Hover actions, the
reaction picker, a reaction chip, a reply, an edit, delete confirmation, the
Markdown and link rendering, an image preview, file cards, and the unread dot
clearing on open all worked. The native file dialogs need an XDG portal, which
the private desktop doesn't run, so Attach and Save were checked only through
the integration test. Slint's Skia renderer drew nothing for the COLRv1 build
of Noto Color Emoji; the CBDT build, which most distributions ship, rendered in
color. Talk asks for "Noto Color Emoji" on Linux, "Segoe UI Emoji" on Windows.

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
