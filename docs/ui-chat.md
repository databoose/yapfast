# First chat window

Mode: Operate. Native Linux and Windows desktop, implemented in Rust and GPUI.

Friends should be able to try the layout immediately, then sign into an existing
Matrix account, select a joined conversation, read history, and send encrypted
text. Preserve the agreed Discord-style navigation and the incumbent dark native
controls. Persistent voice channels extend this window; room creation and ringing
DM calls belong to later slices. Navigation follows the user's
correction: DMs at the upper left, server buttons underneath, and channels scoped
to the selected server. Matrix Spaces provide real server membership.

## Direction contract

THESIS: A compact conversation workspace. Messages occupy the main area and
navigation stays in a narrow sidebar. Avoid dashboard cards and promotional copy.

OWN-WORLD: Neutral dark backgrounds, light text, restrained separators, system
type, and bundled Lucide icons. GPUI Kit owns focus, editing, and button states.

STORY: Start with password sign-in or a clearly labeled local preview. Find a
conversation, read it, type a message, and see delivery or an actionable error.

FIRST VIEWPORT: Open at 1120 by 760. A 64px rail has DMs first and servers below.
A 240px sidebar shows only DMs or the selected server's channels. The flexible
timeline has a conversation header above messages and a composer below. Sign-in uses a centered 380px form.

FORM: Code-led extension of the native window. The familiar chat arrangement was
pinned during the product interview. No concept lottery or raster comp applies.
Seed key: incumbent-native-chat. Signature interaction: drafts survive switching
conversations, and the preview composer works without an account. Switching a
server restores its last channel and draft. A voice connection stays active while
switching text channels, servers and DMs. Its controls remain above the account
footer; selecting a voice channel shows participants instead of a composer.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## States and limits

Cover signed out, restoring, signing in, local preview, loading a room, empty
history, recovery required, sending, send failure, offline sync, and sign-out.
Voice states include joining, connected alone, participants, mute, deafen,
reconnecting, leaving, device selection, failure and switching channels.
Join muted with an explicit action. Changing voice channels requires a visible
leave-and-join confirmation. Device selectors use stable native device IDs and
remain local to the current app session. Group text and voice channels separately
in the server sidebar with Hash and Volume2 icons. Preview voice is labeled and
does not open devices.
Keep passwords and recovery keys masked. Sending requires a trusted device and
an encrypted joined room. Voice also requires an encrypted RTC slot and a
compatible calling service. Existing unencrypted rooms remain readable.
Preview content and messages stay local and disappear when the preview closes.
At 800 by 560 retain navigation, visible errors, and the composer. Capture Linux
at both sizes. Windows build and rendering require the Windows tester.

Audio device settings open above the footer as a native popover. The channel
list keeps at least 96px at the minimum window size; voice errors scroll within
48px. While voice is active, the account footer shows account, sync status and
actions, with the storage explanation returning after departure. Leave also
cancels a pending join before it can become connected.
