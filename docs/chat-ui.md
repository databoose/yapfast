# First usable chat UI

Implemented on 2026-10-02. The native desktop uses the existing GPUI Kit and
Matrix SDK pins. No new runtime dependency was needed.

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
- Send plain text through the Matrix SDK only from a cross-signed device into
  a joined encrypted room. Existing plaintext rooms are read-only.
- Keep per-room drafts through conversation switches. On a send failure keep
  the text and reuse the transaction ID when retrying the same draft.
- Show sync status, reconnect, and sign out while retaining encrypted stores.

Network operations run on the existing two-worker Tokio runtime. The UI receives
SDK events, and the message list virtualizes its rows. There is no UI polling loop.
Only joined Spaces and their joined channels appear. DMs stay in the DM list even
if linked to a Space. Standalone rooms outside Spaces, channel discovery and room
creation remain later work, along with unread handling, message actions, attachments,
rich formatting, durable drafts, offline queuing, calling and tray support remain
later work. Closing the last window exits Talk.

## Validation

The local chat integration test creates disposable accounts and checks restoration,
live message delivery, retry without duplication, wire encryption and decryption,
plaintext-room refusal, unverified-device refusal, and logout credential cleanup.
It also checks separate server channel lists, nested/cyclic Spaces, deduplication,
and live channel removal.
Run it after `scripts/test-stack up`:

```sh
cargo test --locked -p talk-core chat::tests::encrypted_chat_restore_send_retry_and_security_gates -- --ignored
```

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
