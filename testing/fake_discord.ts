// A fake Discord for tests and memory measurements: the gateway and the parts
// of the HTTP API Talk uses, with one deterministic server, "Server", that
// mirrors the Matrix fixture (Alice reads #general, Bob posts, three people sit
// in the Lounge voice channel). Run `bun testing/fake_discord.ts --port 18400`,
// then start Talk with TALK_DISCORD_TEST_SERVER=http://127.0.0.1:18400.
// `--extra N` sets how many history messages follow the four fixture ones
// (default 56, 60 in all). `--servers N` adds N - 1 large, quiet servers, as a
// busy account has, for measuring; POST /_control/busy-start makes Bob post
// in their channels ten times a second, and /_control/busy-stop stops him.
// `--folders` puts them in folders in Alice's server list: the first three in
// a red "Games" folder, the next two in one without a name or colour.
// `--dms N` adds N older DMs, each with a person of its own, every other one
// a friend, as a busy account has. `--images N` attaches a picture of its own
// to every Nth extra history message, for a channel heavy with previews.
// `--channels N` adds text channels room-1 to room-N to "Server", each with 60
// messages, every third with a picture, written in turn by `--authors N`
// people of their own (Bob without it), for a long session across channels.
// POST /_control/crowd-start makes someone new post in the last of them, or
// in #general, 50 times a second, and /_control/crowd-stop stops them.
// POST /_control/chat-start makes Bob post in
// #general every 2 s; POST /_control/chat-stop stops him. POST
// /_control/gallery makes him post three photos in one message. Joining voice
// connects to a voice server here that answers the handshake without DAVE and
// counts the audio each call connection sends: GET /_control/call-audio
// lists it by connection, and GET /_control/voice-audio by SSRC with op 5's
// latest speaking value, and whether it ever said it was speaking. POST /_control/bob-speak makes Bob say two
// seconds of a tone in every call Alice is in. POST /_control/voice-drop closes her call's voice
// connections with 4015, as a voice server restarting does, for them to
// resume with op 7. Each call connection gets an RTCP receiver report about
// its audio every 50 packets, as Discord's voice server sends; POST
// /_control/voice-link?ping=300&loss=7 holds each heartbeat ACK (op 6) back
// 300 ms and reports 7% of the audio lost, and with no parameters sets both
// back to none. POST /_control/camera turns Bob's camera on in the call
// he's in, and with ?off off again; ?who=carol or dave turns theirs. Calls
// connected there hear of it with op 12, and get the recording, or the
// Annex B file FAKE_DISCORD_CAMERA names, once they ask with op 15. Alice's camera
// is decrypted and its frames counted; GET /_control/call-video lists what
// each call connection sent and asked for. A Go Live stream's video is decrypted and its VP8 frames
// counted; GET /_control/video lists what each stream received. POST
// /_control/voice-move moves Alice's call in the server to another voice
// server, and GET /_control/voice-identifies lists the tokens voice
// connections identified with. POST /_control/elsewhere has Alice join the
// Lounge on another client, whose session her voice state then names, and
// ends her calls here; ?leave leaves it there. Op 4 with no channel takes
// her out of voice whichever session sent it, as Discord may, so a client
// must not send it for another's call. Joining "Create Room" has a bot move Alice to
// "Alice's room", made the first time, on another voice server; POST /_control/stream-move with a stream key
// moves that stream to another server, and POST
// /_control/stream-viewer?user=bob has Bob watch Alice's stream (&of=carol
// Carol's instead; &stop stops him), as STREAM_UPDATE's viewer_ids tell. Watching
// Bob's stream gets testing/fixtures' recording once the viewer asks for video
// with op 15: VP8 or, after POST /_control/stream-codec with "h264", H264.
// Alice has a DM with Bob, whose last two messages she hasn't read, and a
// group DM, "Weekend plans", with Bob and Carol. POST /_control/dm makes Dave
// write to her, opening a DM with her the first time, and POST
// /_control/mention makes Bob mention her in #general. #general has a topic
// that mentions Bob, links #general and a page; POST /_control/topic sets
// it to the body, or with none takes it away, and CHANNEL_UPDATE says so. Erin, who shares no
// server with her, sent a message request, and Frank one Discord marked as
// spam; PUT and DELETE /channels/{id}/recipients/@me accept and ignore them.
// Erin also sent her a friend request; see "relationships" below.
// POST /_control/call makes Bob call her; see "DM calls" below.
// Bob and Carol are her friends; POST /users/@me/channels opens a DM, as with
// Carol, whom she has none with yet, or with several people makes a group DM,
// and PUT /channels/{id}/recipients/{user} adds someone to a group.
// GET /channels/{id}/messages/pins lists a channel's pins: Bob's welcome in
// #general, his question in his DM and Carol's in the group. GET
// /channels/{id}/messages/search finds a DM's messages by their text, 25 at
// a time, and GET /guilds/{id}/messages/search a server's, in the channels
// channel_id names or in all of them; both take author_id, mentions, has,
// min_id, max_id and pinned too. The first search of each DM or server
// answers 202, as Discord's does while it indexes it.
// POST /_control/typing makes Bob start typing in #general, or with ?dm in
// his DM; GET /_control/typed lists the channels Alice said she was typing in.
// GET /channels/{id}/messages takes `before`, `after` or `around` a snowflake,
// with `limit`; GET /_control/history lists each such request's query, oldest
// first, and POST clears the list.
// GET /_control/acks lists Alice's read acknowledgements, oldest first: each
// request's channels, several for POST /read-states/ack-bulk.
// POST /_control/react makes Carol add or take back a reaction on the newest
// message in #general.
// GET /users/{id}/profile answers for everyone: Bob has a bio, pronouns, a
// banner and a server bio, Carol a banner colour, and the rest nothing more.
// POST /_control/embeds makes Bob post an article link, whose preview arrives
// a moment later in a MESSAGE_UPDATE as Discord's does, a YouTube link and a
// GIF link, and a bot post a card with fields; their pictures come from
// /external/, standing in for Discord's media proxy.
// The server has two custom emoji, :talk: and the animated :party:, served
// from /emojis/, and a sticker from /stickers/. `--nitro` gives Alice Nitro,
// so every server's emoji are hers to use, and `--cdn-delay MS` makes each
// emoji and sticker take that long, as a round trip to the real CDN does. POST /_control/showcase makes
// Bob post a spoiler, custom emoji, stickers, a GIF, a spoilered picture and a reply
// to the start of #general's history, and Alice block Dave, who then posts.
// /gifs/trending, /gifs/trending-gifs, /gifs/search and /gifs/suggest find
// ten GIFs, made by testing/make-gif-fixtures and served from /_gifs/; a
// search for "nothing" finds none. GET /_control/gif-selects lists what POST
// /gifs/select reported. Alice's favourite GIFs, two to start with, are in
// /users/@me/settings-proto/2, and a sent GIF's page unfurls into a gifv
// embed a moment later, a direct link to a GIF's file into an image embed.
// Alice's custom status, "Testing Talk", is in her settings; PATCH
// /users/@me/settings-proto/1 replaces them, and GET /_control/presence shows the
// presence her session last sent with op 3. POST /users/@me/relationships sends
// a friend request by username: Frank takes none, and unknown names fail.
// From a DM's menu, PUT /users/@me/relationships/{id} with type 2 blocks,
// DELETE ends a friendship or block, PUT and DELETE .../{id}/ignore ignore and
// stop, and PATCH gives a friend a nickname. GET and PUT /users/@me/notes/{id}
// read and write Alice's note on someone, DELETE /channels/{id} closes a DM,
// and PATCH /users/@me/guilds/@me/settings mutes and unmutes DMs. PATCH
// /users/@me/guilds/{id}/settings changes her notification settings for a
// server and its channels, and USER_GUILD_SETTINGS_UPDATE says so; PATCH
// /users/@me/guilds/settings changes several servers' at once, as Discord's
// client pins a channel to the top with its override's flags. POST
// /channels/{id}/invites makes an invite to a channel of "Server" or of a
// `--servers` filler, which @everyone may, as on Discord, and GET
// /_control/invites lists them, each with the location its
// X-Context-Properties named, as Invite to Server's does.
// Alice's Crew role may manage channels: POST /guilds/{id}/channels makes a
// channel or category in "Server", and GET, PATCH and DELETE /channels/{id}
// read, change and delete one, each with its CHANNEL_ event. Deleting a
// category leaves its channels outside any, as Discord does. DELETE
// /users/@me/guilds/{id} leaves "Server", owned by Bob, with GUILD_DELETE and
// GUILD_MEMBER_REMOVE, and READY leaves it out until POST /_control/rejoin
// brings her back with its GUILD_CREATE.
// Linking by QR code goes through a remote login gateway at /remote-auth/, as
// remote-auth-gateway.discord.gg speaks it. POST /_control/qr-scan scans the
// code shown as Alice, /_control/qr-confirm confirms it, /_control/qr-cancel
// says no, and /_control/qr-expire ends the code as the real one does after
// two minutes. POST /_control/qr-captcha makes the next ticket's trade ask
// for an hCaptcha, as Discord does of a login it doesn't trust, until a try
// brings X-Captcha-Key "solved" with the rqtoken and session it was given. Start Talk with TALK_DISCORD_TEST_UNLINKED=1 to link. POST
// /auth/logout ends the session, and GET /_control/logouts counts them.
// A gateway session whose connection drops resumes with op 6, getting the
// events it missed; a close with 1000 or 1001 ends it instead. POST
// /_control/gateway-reconnect sends op 7, and GET /_control/sessions counts
// identifies and resumes. The API sets a device cookie, as Discord's does, and
// GET /_control/cookies counts how often it was set and sent back.
// GET /_control/list-requests counts member list subscriptions. POST
// /_control/slow with a number of milliseconds slows history, attachments
// and links' pictures, to show loading states.
// POST /_control/clip makes Bob attach clip.mp4, six seconds with sound, and
// link a post whose card plays a video, as vxtwitter's do.
// POST /_control/bob-away makes Bob idle with a custom status,
// /_control/bob-mobile puts him online on his phone alone, and
// /_control/bob-offline takes him offline.
// Alice has the Crew role, which only she has. `--unread N` sets how many of
// #general's newest messages she hasn't read (default 5). POST
// /_control/mentions makes Bob mention her in #general every way Discord
// highlights for her, and two ways it doesn't: by name, through Crew, with
// @everyone and @here, in a reply to her message, then through Mods, which
// she lacks, and not at all.
// #general has a thread, "Plans", started from Bob's welcome, which Alice is
// in and has read. POST /_control/thread-reply makes Carol write in it; POST
// /_control/thread-start makes Bob start "Movie night" without a message,
// which Alice isn't in, with its notice in #general and his first message in
// it; POST /_control/thread-archive archives "Plans", and with ?open opens it
// again. Writing in a thread adds Alice to it, as Discord does. #general has
// other threads too: "Book club", which Alice isn't in, a private one she
// can't see, and 30 older, archived ones. Subscribing to the server with
// `threads` sends THREAD_LIST_SYNC with its active threads and each one's
// newest message, and GET /channels/{id}/threads/search lists a channel's
// threads, as Discord's list of threads asks; GET /_control/thread-searches
// lists each search's query. POST /channels/{id}/messages/{id}/threads and
// POST /channels/{id}/threads start threads as Discord does, with
// THREAD_CREATE, THREAD_MEMBERS_UPDATE adding Alice, the message's
// MESSAGE_UPDATE and the THREAD_CREATED notice; GET /_control/thread-creates
// lists each body sent.

import { createCipheriv, createDecipheriv } from "node:crypto";
import { readFileSync } from "node:fs";
import { parseArgs } from "node:util";
import { deflateSync } from "node:zlib";
import type { ServerWebSocket } from "bun";

const { values: args } = parseArgs({
  options: {
    port: { type: "string", default: "18400" },
    host: { type: "string", default: "127.0.0.1" },
    extra: { type: "string", default: "56" },
    servers: { type: "string", default: "1" },
    dms: { type: "string", default: "0" },
    images: { type: "string", default: "0" },
    channels: { type: "string", default: "0" },
    authors: { type: "string", default: "0" },
    nitro: { type: "boolean", default: false },
    folders: { type: "boolean", default: false },
    "cdn-delay": { type: "string", default: "0" },
    unread: { type: "string", default: "5" },
  },
});

const TOKEN = "talk-test-token";
/** Whether Alice has Nitro: `--nitro`, or POST /_control/nitro, with ?off
 *  to take it away again, which tells her session with USER_UPDATE. */
let nitro = args.nitro;
const DISCORD_EPOCH = 1_420_070_400_000n;
const HEARTBEAT_INTERVAL = 41_250;
const CHAT_INTERVAL = 2_000;

// Fixed ids, so runs compare.
const GUILD = "1400000000000000100";
const CATEGORY = "1400000000000000101";
const GENERAL = "1400000000000000102";
const LOUNGE = "1400000000000000103";
// A bot makes a room for whoever joins Create Room and moves them there.
const CREATE_ROOM = "1400000000000000104";
const ALICES_ROOM = "1400000000000000105";
const MODS = "1400000000000000110";
// Only those who may mention everyone can mention Bots.
const BOTS = "1400000000000000111";
// Alice's own role.
const CREW = "1400000000000000112";
// Carol's Leads rank above Alice's Staff, which moderates those below it.
const LEADS = "1400000000000000113";
const STAFF = "1400000000000000114";
const DM_BOB = "1400000000000000120";
const WEEKEND = "1400000000000000121";
const DM_DAVE = "1400000000000000122";
const REQUEST_ERIN = "1400000000000000123";
const SPAM_FRANK = "1400000000000000124";
const EMOJI_TALK = "1400000000000000300";
const EMOJI_PARTY = "1400000000000000301";
const STICKER_WAVE = "1400000000000000310";
const STICKER_BOUNCE = "1400000000000000311";
const ALICE = "1400000000000000001";

// Dave has no avatar, so he gets a default one; Carol set another for the server.
type User = { id: string; username: string; global_name: string; avatar: string | null; bot?: boolean };
const users: Record<string, User> = {
  alice: { id: ALICE, username: "alice", global_name: "Alice", avatar: "alice1" },
  bob: { id: "1400000000000000002", username: "bob", global_name: "Bob", avatar: "bob1" },
  carol: { id: "1400000000000000003", username: "carol", global_name: "Carol", avatar: "carol1" },
  dave: { id: "1400000000000000004", username: "dave", global_name: "Dave", avatar: null },
};
// People Alice shares no server with, who wrote to her anyway.
const strangers: Record<string, User> = {
  erin: { id: "1400000000000000005", username: "erin", global_name: "Erin", avatar: "erin1" },
  frank: { id: "1400000000000000006", username: "frank_deals", global_name: "Free Nitro", avatar: null },
};
const userById = new Map([...Object.values(users), ...Object.values(strangers)].map((user) => [user.id, user]));

// Alice's relationships by user id: Bob and Carol are friends (1), and Erin
// sent her a friend request (3). Alice's own requests are outgoing (4); Dave
// accepts hers after FRIEND_ACCEPT_MS, and nobody else answers.
const relationships = new Map<string, number>([
  [users.bob.id, 1],
  [users.carol.id, 1],
  [strangers.erin!.id, 3],
]);
const FRIEND_ACCEPT_MS = 2000;
// Whom Alice ignores, the nicknames she gave friends and her notes, by user id.
const ignored = new Set<string>();
const nicknames = new Map<string, string>();
const notes = new Map<string, string>();
const wireRelationship = (id: string, type: number) => ({
  id, type, nickname: nicknames.get(id) ?? null, user_id: id, since: JOINED_AT, user_ignored: ignored.has(id),
});
// Alice's notification settings by server, "@me" for her DMs, as Discord
// keeps her user guild settings: the server's mute and level, which
// mentions call on her, whether muted channels hide, and each channel's and
// category's override, whose flags pin a channel to the top of her list
// (FAVORITED, 1 << 11). PATCH /users/@me/guilds/{id}/settings changes them,
// or PATCH /users/@me/guilds/settings several servers' at once.
type MuteConfig = { end_time: string | null; selected_time_window: number };
type ChannelOverride = { muted: boolean; mute_config: MuteConfig | null; message_notifications: number; collapsed: boolean; flags: number };
type GuildSettings = {
  muted: boolean;
  mute_config: MuteConfig | null;
  message_notifications: number;
  suppress_everyone: boolean;
  suppress_roles: boolean;
  hide_muted_channels: boolean;
  overrides: Map<string, ChannelOverride>;
};
const guildSettings = new Map<string, GuildSettings>();
const settingsOf = (guild: string) => {
  let settings = guildSettings.get(guild);
  if (!settings) {
    settings = {
      muted: false,
      mute_config: null,
      // Her own level in "Server" is mentions; DMs use the default.
      message_notifications: guild === "@me" ? 3 : 1,
      suppress_everyone: false,
      suppress_roles: false,
      hide_muted_channels: false,
      overrides: new Map(),
    };
    guildSettings.set(guild, settings);
  }
  return settings;
};
const wireGuildSettings = (guild: string) => {
  const settings = settingsOf(guild);
  return {
    guild_id: guild === "@me" ? null : guild,
    muted: settings.muted,
    mute_config: settings.mute_config,
    message_notifications: settings.message_notifications,
    suppress_everyone: settings.suppress_everyone,
    suppress_roles: settings.suppress_roles,
    hide_muted_channels: settings.hide_muted_channels,
    mobile_push: true,
    mute_scheduled_events: false,
    notify_highlights: 0,
    flags: 0,
    version: 1,
    channel_overrides: [...settings.overrides].map(([channel_id, override]) => ({ channel_id, ...override })),
  };
};

// Alice's settings as Discord keeps them: a protobuf message in base64, of
// which the fake knows only the status, field 11, holding the custom status.
const protoBytes = (field: number, bytes: Uint8Array) => {
  const out = [field << 3 | 2];
  for (let length = bytes.length; ; length >>= 7) {
    if (length < 0x80) { out.push(length); break; }
    out.push(length & 0x7f | 0x80);
  }
  return new Uint8Array([...out, ...bytes]);
};
const text = (value: string) => new TextEncoder().encode(value);
let settings = Buffer.from(
  protoBytes(11, protoBytes(2, new Uint8Array([...protoBytes(1, text("Testing Talk")), ...protoBytes(3, text("🧪"))]))),
).toString("base64");
// The presence Alice's session last sent, if any.
let alicePresence: unknown = null;

const HISTORY_START = Date.UTC(2026, 9, 1, 9, 0, 0);
const JOINED_AT = "2026-09-01T09:00:00.000000+00:00";
const bit = (n: number) => 2 ** n;
// Invite, view, send, embed, attach, read history, react, connect, speak,
// voice activity, change one's own nickname, make public and private threads
// and write in them: Discord's defaults for @everyone.
const EVERYONE = [0, 10, 11, 14, 15, 16, 6, 20, 21, 25, 26, 35, 36, 38].reduce((sum, n) => sum + bit(n), 0);
// Kick, ban, manage nicknames, manage roles, moderate members, and mute,
// deafen and move members in voice.
const MODERATION = [1, 2, 27, 28, 40, 22, 23, 24].reduce((sum, n) => sum + bit(n), 0);

function snowflake(ms: number, increment = 0): string {
  return (((BigInt(ms) - DISCORD_EPOCH) << 22n) | BigInt(increment)).toString();
}

function iso(ms: number): string {
  return new Date(ms).toISOString().replace("Z", "000+00:00");
}

function timeOf(id: string): number {
  return Number((BigInt(id) >> 22n) + DISCORD_EPOCH);
}

function wireUser(user: User) {
  return {
    ...user,
    discriminator: "0",
    public_flags: 0,
    avatar_decoration_data: null,
  };
}

// --- Server members ---------------------------------------------------------
// "Server"'s members as its member menu changes them: roles, nicknames and
// timeouts, and who was kicked or banned. Bob owns it and is a Mod
// everywhere his member object goes, so his name takes its colour. Alice is
// Staff and Crew; Carol, a Lead, ranks above her, and Dave, with no role,
// below. PATCH /guilds/{id}/members/@me renames Alice, and PATCH
// /guilds/{id}/members/{user} renames others or times them out; PUT and
// DELETE .../members/{user}/roles/{role} give and take roles, DELETE
// .../members/{user} kicks, and PUT /guilds/{id}/bans/{user} bans, each as
// far as Alice's roles allow, as Discord checks them. Staff may also mute,
// deafen and move members in voice: PATCH .../members/{user} with `mute`,
// `deaf` or `channel_id`, null to disconnect them, changes the voice state
// of someone in the server's voice, which VOICE_STATE_UPDATE says, and GET
// /_control/voice-changes lists each. POST /_control/voice-reset puts Carol
// and Dave back in the Lounge as they started. Each answers with
// GUILD_MEMBER_UPDATE, or GUILD_MEMBER_REMOVE and GUILD_BAN_ADD, and the
// member list's operations. GET /_control/audit-log lists the reasons given,
// and POST /_control/members-reset puts everyone back but keeps the log.
// Members have server profiles too: Carol a server avatar, Bob a server
// bio. Alice changes hers as Discord's per-server profile editor does: her
// nickname and server avatar with PATCH /guilds/{id}/members/@me, which
// answers with GUILD_MEMBER_UPDATE, and her banner, bio and pronouns with
// PATCH /guilds/{id}/profile/@me, which has no gateway event. An empty
// value or null goes back to her main profile. A picture, a data URI, is
// served back from the CDN as it came; the avatar, banner and bio need
// Nitro, as on Discord: `--nitro`, or POST /_control/nitro (see below).
// GET /_control/server-profile shows hers.
const memberRoles = new Map<string, string[]>();
const memberNicks = new Map<string, string>();
/** Server profiles by user: avatar and banner hashes, bio and pronouns. */
const serverProfiles = new Map<string, { avatar: string | null; banner: string | null; bio: string; pronouns: string }>();
/** Pictures uploaded as avatars and banners, by hash, served from the CDN. */
const uploadedPictures = new Map<string, { type: string; data: Uint8Array }>();
function serverProfile(id: string) {
  let profile = serverProfiles.get(id);
  if (!profile) {
    profile = { avatar: null, banner: null, bio: "", pronouns: "" };
    serverProfiles.set(id, profile);
  }
  return profile;
}
const timeouts = new Map<string, string>();
const removed = new Set<string>();
/** What Alice's PATCH of a member changed in voice, oldest first, for GET
 *  /_control/voice-changes. */
const voiceChanges: Record<string, unknown>[] = [];
const auditLog: { action: string; user: string; reason: string | null }[] = [];
function resetMembers() {
  memberRoles.clear();
  memberRoles.set(ALICE, [CREW, STAFF]);
  memberRoles.set(users.bob.id, [MODS]);
  memberRoles.set(users.carol.id, [LEADS]);
  memberNicks.clear();
  serverProfiles.clear();
  serverProfile(users.carol.id).avatar = "carol-server";
  serverProfile(users.bob.id).bio = "Keeps #general tidy.";
  timeouts.clear();
  removed.clear();
}
resetMembers();

function member(user: User, roles: string[] = memberRoles.get(user.id) ?? []) {
  return {
    user: wireUser(user),
    user_id: user.id,
    roles,
    nick: memberNicks.get(user.id) ?? null,
    communication_disabled_until: timeouts.get(user.id) ?? null,
    avatar: serverProfiles.get(user.id)?.avatar ?? null,
    banner: serverProfiles.get(user.id)?.banner ?? null,
    joined_at: JOINED_AT,
    premium_since: null,
    deaf: voice.get(user.id)?.deaf ?? false,
    mute: voice.get(user.id)?.mute ?? false,
    pending: false,
    flags: 0,
  };
}

// --- Files ------------------------------------------------------------------

const crcTable = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});

function crc32(bytes: Uint8Array): number {
  let c = 0xffffffff;
  for (const byte of bytes) c = crcTable[(c ^ byte) & 0xff]! ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const out = new Uint8Array(12 + data.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, data.length);
  out.set(new TextEncoder().encode(type), 4);
  out.set(data, 8);
  view.setUint32(8 + data.length, crc32(out.subarray(4, 8 + data.length)));
  return out;
}

/** The Matrix fixture's sunset.png gradient, drawn at any size. */
function sunset(width: number, height: number): Uint8Array {
  return png(width, height, (x, y) => {
    const sx = Math.floor((x * 800) / width);
    const sy = Math.floor((y * 500) / height);
    return [Math.floor(sx / 4) & 255, Math.floor(sy / 3) & 255, 160, 255];
  });
}

/** An avatar or server icon: two tones split diagonally, picked from its path. */
// Banners are wide, as Discord's are.
function picture(path: string): Uint8Array {
  const hue = crc32(new TextEncoder().encode(path));
  const light = [(hue >>> 16) & 255, (hue >>> 8) & 255, hue & 255].map((c) => 96 + (c % 160));
  const dark = light.map((c) => c >> 1);
  const [width, height] = path.includes("/banners/") ? [600, 240] : [128, 128];
  return png(width, height, (x, y) => [...(x / width + y / height < 1 ? light : dark), 255]);
}

// A person's profile as GET /users/{id}/profile gives it, with the server's
// side when asked for with guild_id.
function wireProfile(user: User, guild: string | null) {
  const bob = user.id === users.bob.id;
  const carol = user.id === users.carol.id;
  const inServer = Object.values(users).includes(user) && user.id !== ALICE;
  const isMember = Object.values(users).includes(user);
  const profile = {
    bio: bob ? "Plays bass in a **ska** band. Ask me about https://example.com/gigs" : "",
    pronouns: bob ? "he/him" : "",
    banner: bob ? "bob-banner" : null,
    accent_color: carol ? 0x5865f2 : null,
  };
  return {
    user: { ...wireUser(user), banner: profile.banner, accent_color: profile.accent_color, bio: profile.bio },
    user_profile: profile,
    ...(guild === GUILD && isMember
      ? {
          guild_member_profile: guildMemberProfile(user.id),
          guild_member: { ...member(user), bio: serverProfile(user.id).bio },
        }
      : {}),
    mutual_guilds: inServer ? [{ id: GUILD, nick: null }] : [],
    mutual_friends: bob ? [wireUser(users.carol)] : [],
    badges: [],
    premium_type: null,
  };
}

/** A server profile as GET /users/{id}/profile and PATCH
 *  /guilds/{id}/profile/@me give it. */
function guildMemberProfile(id: string) {
  const profile = serverProfile(id);
  return {
    guild_id: GUILD,
    pronouns: profile.pronouns,
    bio: profile.bio,
    banner: profile.banner,
    accent_color: null,
    theme_colors: null,
    popout_animation_particle_type: null,
    emoji: null,
    profile_effect: null,
  };
}

/** A picture sent as a data URI, kept for the CDN; its hash, or an error. */
function keepPicture(uri: unknown): { hash: string } | { error: string } {
  const found = typeof uri === "string" && uri.match(/^data:(image\/(?:png|jpeg|gif|webp));base64,(.+)$/);
  if (!found) return { error: "Invalid image data" };
  const data = Uint8Array.from(Buffer.from(found[2]!, "base64"));
  if (data.length > 10 * 1024 * 1024) return { error: "File cannot be larger than 10240.0 kb." };
  const hash = (found[1] === "image/gif" ? "a_" : "") + crc32(data).toString(16).padStart(8, "0") + data.length.toString(16);
  uploadedPictures.set(hash, { type: found[1]!, data });
  return { hash };
}

/** Discord's answer to a field only Nitro may set. */
function premiumRequired(field: string) {
  return json(
    {
      message: "Invalid Form Body",
      code: 50035,
      errors: { [field]: { _errors: [{ code: "PREMIUM_REQUIRED", message: "You must be a Nitro subscriber to do that." }] } },
    },
    400,
  );
}

/** Discord's answer to a text longer than it takes. */
function tooLong(field: string, limit: number) {
  return json(
    {
      message: "Invalid Form Body",
      code: 50035,
      errors: { [field]: { _errors: [{ code: "BASE_TYPE_MAX_LENGTH", message: `Must be ${limit} or fewer in length.` }] } },
    },
    400,
  );
}

function png(width: number, height: number, pixel: (x: number, y: number) => number[]): Uint8Array {
  const raw = new Uint8Array((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    const row = y * (width * 4 + 1);
    for (let x = 0; x < width; x++) raw.set(pixel(x, y), row + 1 + x * 4);
  }
  const header = new Uint8Array(13);
  const view = new DataView(header.buffer);
  view.setUint32(0, width);
  view.setUint32(4, height);
  header.set([8, 6, 0, 0, 0], 8);
  const parts = [
    new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(raw)),
    chunk("IEND", new Uint8Array()),
  ];
  return new Uint8Array(Buffer.concat(parts));
}

/**
 * An animated GIF that loops: each frame's pixels as indexes into `palette`,
 * each shown for `delay` hundredths of a second. Its LZW data is uncompressed:
 * literal 9-bit codes, with a clear code before the code table would grow.
 */
function gif(width: number, height: number, palette: number[][], frames: Uint8Array[], delay: number): Uint8Array {
  const bytes: number[] = [];
  const word = (n: number) => bytes.push(n & 255, (n >> 8) & 255);
  bytes.push(...new TextEncoder().encode("GIF89a"));
  word(width);
  word(height);
  bytes.push(0xf7, 0, 0);
  for (let i = 0; i < 256; i++) bytes.push(...(palette[i] ?? [0, 0, 0]));
  bytes.push(0x21, 0xff, 11, ...new TextEncoder().encode("NETSCAPE2.0"), 3, 1, 0, 0, 0);
  for (const frame of frames) {
    bytes.push(0x21, 0xf9, 4, 0, delay & 255, delay >> 8, 0, 0);
    bytes.push(0x2c);
    word(0);
    word(0);
    word(width);
    word(height);
    bytes.push(0, 8);
    const codes: number[] = [];
    frame.forEach((pixel, index) => {
      if (index % 250 === 0) codes.push(256);
      codes.push(pixel);
    });
    codes.push(257);
    const packed: number[] = [];
    let bits = 0;
    let pending = 0;
    for (const code of codes) {
      pending |= code << bits;
      bits += 9;
      while (bits >= 8) {
        packed.push(pending & 255);
        pending >>= 8;
        bits -= 8;
      }
    }
    if (bits > 0) packed.push(pending & 255);
    for (let i = 0; i < packed.length; i += 255) {
      const block = packed.slice(i, i + 255);
      bytes.push(block.length, ...block);
    }
    bytes.push(0);
  }
  bytes.push(0x3b);
  return new Uint8Array(bytes);
}

/** A dot that crosses a square and comes back, in `count` frames. */
function bouncing(side: number, count: number, color: number[]): Uint8Array {
  const frames = Array.from({ length: count }, (_, frame) => {
    const phase = frame / count;
    const center = side * (0.2 + 0.6 * (phase < 0.5 ? phase * 2 : 2 - phase * 2));
    const pixels = new Uint8Array(side * side);
    for (let y = 0; y < side; y++) {
      for (let x = 0; x < side; x++) {
        if (Math.hypot(x - center, y - side / 2) < side * 0.15) pixels[y * side + x] = 1;
      }
    }
    return pixels;
  });
  return gif(side, side, [[30, 31, 36], color], frames, 10);
}

type File = {
  id: string;
  channel: string;
  filename: string;
  contentType: string;
  data: Uint8Array;
  width?: number;
  height?: number;
  /** Draws the image at a requested size, as Discord's media proxy does. */
  draw?: (width: number, height: number) => Uint8Array;
};
const files = new Map<string, File>();
const scaled = new Map<string, Uint8Array>();

function addFile(file: File): File {
  files.set(`${file.channel}/${file.id}/${file.filename}`, file);
  return file;
}

function wireAttachment(file: File, origin: string) {
  const path = `attachments/${file.channel}/${file.id}/${encodeURIComponent(file.filename)}`;
  return {
    id: file.id,
    filename: file.filename,
    size: file.data.length,
    url: `${origin}/${path}`,
    proxy_url: `${origin}/media/${path}`,
    content_type: file.contentType,
    ...(file.width ? { width: file.width, height: file.height } : {}),
  };
}

function pngSize(data: Uint8Array): { width: number; height: number } | undefined {
  if (data.length < 24 || data[0] !== 137 || data[1] !== 80) return undefined;
  const view = new DataView(data.buffer, data.byteOffset);
  return { width: view.getUint32(16), height: view.getUint32(20) };
}

function contentType(filename: string): string {
  const extension = filename.split(".").pop()?.toLowerCase() ?? "";
  const types: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    mp4: "video/mp4",
    webm: "video/webm",
    mov: "video/quicktime",
    txt: "text/plain; charset=utf-8",
  };
  return types[extension] ?? "application/octet-stream";
}

// --- Messages ---------------------------------------------------------------

type Reaction = { name: string; id: string | null; users: Set<string> };
type Message = {
  id: string;
  channel: string;
  author: User;
  content: string;
  type: number;
  nonce?: string;
  reply?: string;
  files: File[];
  reactions: Reaction[];
  edited?: number;
  /** Its embeds, given the server's address for their pictures. */
  embeds?: (origin: string) => unknown[];
  /** A DM call's message: who took part, and when it ended. */
  call?: { participants: Set<string>; ended?: number };
  stickers?: { id: string; name: string; format_type: number }[];
  /** A thread's first message: the message in its channel it was started from. */
  starter?: string;
  /** A notice that a thread was made: the thread. */
  notice?: string;
};
const messages = new Map<string, Message[]>([
  [GENERAL, []],
  [LOUNGE, []],
  [DM_BOB, []],
  [WEEKEND, []],
  [REQUEST_ERIN, []],
  [SPAM_FRANK, []],
]);

// DMs and group DMs by channel id, with everyone in them besides Alice.
type Private = { type: 1 | 3; recipients: User[]; name?: string; request?: boolean; spam?: boolean };
const privates = new Map<string, Private>();
const byId = new Map<string, Message>();
let lastId = 0n;

function nextId(ms: number): string {
  let id = BigInt(snowflake(ms));
  if (id <= lastId) id = lastId + 1n;
  lastId = id;
  return id.toString();
}

function post(
  channel: string,
  author: User,
  content: string,
  ms: number,
  extra: Partial<Message> = {},
): Message {
  const message: Message = {
    id: nextId(ms),
    channel,
    author,
    content,
    type: extra.reply ? 19 : 0,
    files: [],
    reactions: [],
    ...extra,
  };
  messages.get(channel)!.push(message);
  byId.set(message.id, message);
  return message;
}

function wireMessage(message: Message, origin: string, nested = false): Record<string, unknown> {
  const mentions = [...message.content.matchAll(/<@!?(\d+)>/g)]
    .map((match) => userById.get(match[1]!))
    .filter((user) => user !== undefined)
    .map(wireUser);
  // As Discord, only roles and @everyone that the author may mention count;
  // here, Bob and the bots may mention anything.
  const mayMention = message.author.id === users.bob.id || !!message.author.bot;
  const wire: Record<string, unknown> = {
    id: message.id,
    type: message.type,
    content: message.content,
    channel_id: message.channel,
    author: wireUser(message.author),
    attachments: message.files.map((file) => wireAttachment(file, origin)),
    embeds: message.embeds?.(origin) ?? [],
    mentions,
    mention_roles: mayMention ? [...message.content.matchAll(/<@&(\d+)>/g)].map((match) => match[1]) : [],
    pinned: false,
    mention_everyone: mayMention && /@(everyone|here)\b/.test(message.content),
    tts: false,
    timestamp: iso(timeOf(message.id)),
    edited_timestamp: message.edited ? iso(message.edited) : null,
    // HAS_THREAD, once a thread started from it.
    flags: threads.has(message.id) ? 1 << 5 : 0,
    components: [],
  };
  if (message.nonce) wire.nonce = message.nonce;
  if (message.stickers) wire.sticker_items = message.stickers;
  if (message.call) {
    wire.call = {
      participants: [...message.call.participants],
      ended_timestamp: message.call.ended ? iso(message.call.ended) : null,
    };
  }
  if (message.reactions.length) {
    wire.reactions = message.reactions.map((reaction) => ({
      emoji: { id: reaction.id, name: reaction.name },
      count: reaction.users.size,
      count_details: { burst: 0, normal: reaction.users.size },
      burst_colors: [],
      me_burst: false,
      burst_me: false,
      me: reaction.users.has(ALICE),
      burst_count: 0,
    }));
  }
  const thread = threads.get(message.id);
  if (thread && !nested) wire.thread = wireThread(thread);
  if (message.starter && !nested) {
    const original = byId.get(message.starter);
    wire.message_reference = { type: 0, channel_id: original?.channel, message_id: message.starter, guild_id: GUILD };
    wire.referenced_message = original ? wireMessage(original, origin, true) : null;
  }
  if (message.notice) wire.message_reference = { type: 0, channel_id: message.notice, guild_id: GUILD };
  if (message.reply && !nested) {
    const original = byId.get(message.reply);
    wire.message_reference = {
      type: 0,
      channel_id: message.channel,
      message_id: message.reply,
      ...guildOf(message.channel),
    };
    wire.referenced_message = original ? wireMessage(original, origin, true) : null;
  }
  return wire;
}

/** The server of a channel as events carry it; DMs have none. */
function guildOf(channel: string): { guild_id?: string } {
  return privates.has(channel) ? {} : { guild_id: fillerChannels.get(channel) ?? GUILD };
}

/** A message as the gateway dispatches it: with the server and the author's member. */
function dispatched(message: Message, origin: string) {
  if (privates.has(message.channel)) return wireMessage(message, origin);
  const { user: _, ...profile } = member(message.author);
  return { ...wireMessage(message, origin), ...guildOf(message.channel), member: profile };
}

/** A DM or group DM as READY lists it, with user ids, or as an event carries it. */
function wirePrivate(id: string, channel: Private, users: "ids" | "objects") {
  return {
    id,
    type: channel.type,
    last_message_id: messages.get(id)!.at(-1)?.id ?? null,
    flags: 0,
    ...(channel.type === 3 ? { name: channel.name ?? null, icon: null, owner_id: channel.recipients[0]!.id } : {}),
    ...(channel.request
      ? { is_message_request: true, is_message_request_timestamp: iso(timeOf(messages.get(id)![0]!.id)), is_spam: !!channel.spam }
      : {}),
    ...(users === "ids"
      ? { recipient_ids: channel.recipients.map((user) => user.id) }
      : { recipients: channel.recipients.map(wireUser) }),
  };
}

// The Matrix fixture's conversation, then its extra history.
const sunsetFile = addFile({
  id: snowflake(HISTORY_START + 60_000, 1),
  channel: GENERAL,
  filename: "sunset.png",
  contentType: "image/png",
  data: sunset(800, 500),
  width: 800,
  height: 500,
  draw: sunset,
});
const notesFile = addFile({
  id: snowflake(HISTORY_START + 120_000, 1),
  channel: GENERAL,
  filename: "trip-notes.txt",
  contentType: "text/plain; charset=utf-8",
  data: new TextEncoder().encode("Bring snacks."),
});
const welcome = post(
  GENERAL,
  users.bob,
  "Welcome to **Talk**, with `code` and a [link](https://example.org)",
  HISTORY_START,
);
welcome.reactions.push({ name: "👍", id: null, users: new Set([users.carol.id, ALICE]) });
post(GENERAL, users.bob, "", HISTORY_START + 60_000, { files: [sunsetFile] });
post(GENERAL, users.bob, "", HISTORY_START + 120_000, { files: [notesFile] });
post(GENERAL, users.bob, "The welcome has the plan", HISTORY_START + 180_000, {
  reply: welcome.id,
});

function sample(index: number): string {
  switch (index % 4) {
    case 0:
      return `Message ${index}: short one`;
    case 1:
      return `Message ${index}: a longer line about the plan for the weekend, so it wraps across the timeline at ordinary window widths`;
    case 2:
      return `Message ${index} with **bold**, \`code\` and a list:\n- one\n- two`;
    default:
      return `Message ${index}: 👍 sounds good`;
  }
}

const extra = Number(args.extra);
const cdnDelay = Number(args["cdn-delay"]);
const imageEvery = Number(args.images);
for (let index = 0; index < extra; index++) {
  const ms = HISTORY_START + (4 + index) * 60_000;
  const files: File[] = [];
  if (imageEvery > 0 && index % imageEvery === 0) {
    const shade = [(index * 37) % 200, (index * 71) % 200, 160];
    const draw = (w: number, h: number) =>
      png(w, h, (x, y) => [shade[0]! + Math.floor((55 * x) / w), shade[1]! + Math.floor((55 * y) / h), shade[2]!, 255]);
    files.push(addFile({ id: snowflake(ms, 1), channel: GENERAL, filename: `photo-${index}.png`, contentType: "image/png", data: draw(480, 320), width: 480, height: 320, draw }));
  }
  // The oldest of the latest page is a boost, which Talk doesn't show but
  // which still counts toward the page.
  if (index === extra - 50) post(GENERAL, users.bob, "", ms, { type: 8 });
  else if (index < extra - 1) post(GENERAL, users.bob, sample(index), ms, { files });
  else {
    // The newest has reactions from everyone, more than a tooltip names.
    const newest = post(GENERAL, users.bob, sample(index), ms, { files });
    const everyone = [ALICE, users.bob.id, users.carol.id, users.dave.id];
    newest.reactions.push(
      { name: "🎉", id: null, users: new Set(everyone) },
      { name: "😂", id: null, users: new Set([users.bob.id, users.dave.id]) },
      { name: "❤️", id: null, users: new Set([users.carol.id]) },
    );
  }
}

// The channels of `--channels`, written by the people of `--authors`.
const authors: User[] = Array.from({ length: Number(args.authors) }, (_, n) => ({
  id: (1_700_000_000_000_000_000n + BigInt(n)).toString(),
  username: `writer${n}`,
  global_name: `Writer ${n}`,
  avatar: `w${n}`,
}));
for (const author of authors) userById.set(author.id, author);
const ROOM_HISTORY = 60;
const rooms = Array.from({ length: Number(args.channels) }, (_, n) => {
  const id = (1_400_000_000_000_010_000n + BigInt(n)).toString();
  messages.set(id, []);
  for (let index = 0; index < ROOM_HISTORY; index++) {
    const ms = HISTORY_START + (n * ROOM_HISTORY + index) * 1000;
    const author = authors.length ? authors[(n * 7 + index) % authors.length]! : users.bob;
    const files: File[] = [];
    if (index % 3 === 0) {
      const shade = [(n * 53 + index * 11) % 200, (n * 29 + index * 37) % 200, 120];
      const draw = (w: number, h: number) =>
        png(w, h, (x, y) => [shade[0]! + Math.floor((55 * x) / w), shade[1]! + Math.floor((55 * y) / h), shade[2]!, 255]);
      let data: Uint8Array | undefined;
      files.push(addFile({
        id: snowflake(ms, 1),
        channel: id,
        filename: `room-${n + 1}-${index}.png`,
        contentType: "image/png",
        get data() {
          return (data ??= draw(480, 320));
        },
        width: 480,
        height: 320,
        draw,
      }));
    }
    post(id, author, `room-${n + 1} message ${index}: ${sample(index)}`, ms, { files });
  }
  return id;
});
let crowd: ReturnType<typeof setInterval> | undefined;
let crowdCount = 0;

// The extra DMs of `--dms`, older than the fixture's.
const fillerPeople: User[] = [];
for (let n = 0; n < Number(args.dms); n++) {
  const user = { id: (1_600_000_000_000_000_000n + BigInt(n)).toString(), username: `person${n}`, global_name: `Person ${n}`, avatar: `p${n}` };
  fillerPeople.push(user);
  userById.set(user.id, user);
  if (n % 2 === 0) relationships.set(user.id, 1);
  const channel = (1_390_000_000_000_000_000n + BigInt(n)).toString();
  messages.set(channel, []);
  privates.set(channel, { type: 1, recipients: [user] });
  post(channel, user, `Hi from ${user.global_name}`, HISTORY_START + n * 1000);
}

privates.set(DM_BOB, { type: 1, recipients: [users.bob] });
privates.set(WEEKEND, { type: 3, recipients: [users.bob, users.carol], name: "Weekend plans" });
const DM_START = HISTORY_START + 86_400_000;
const hike = post(WEEKEND, users.carol, "Hike on Saturday?", DM_START);
post(WEEKEND, users.bob, "I'm in", DM_START + 60_000);
const tripNotes = post(DM_BOB, users.bob, "Did you see the trip notes?", DM_START + 120_000);
// Pinned messages by channel, the latest pinned first.
const pins = new Map<string, { message: Message; at: number }[]>([
  [GENERAL, [{ message: welcome, at: HISTORY_START + 200_000 }]],
  [DM_BOB, [{ message: tripNotes, at: DM_START + 130_000 }]],
  [WEEKEND, [{ message: hike, at: DM_START + 10_000 }]],
]);
/** What a search's query string asks of a message, as Discord's search
 *  takes it: `content`, and each of `author_id`, `mentions` and `has`
 *  repeated for several, `min_id`, `max_id` and `pinned`. */
function searchFilter(url: URL): (message: Message) => boolean {
  const params = url.searchParams;
  const content = (params.get("content") ?? "").toLowerCase();
  const authors = params.getAll("author_id");
  const mentioned = params.getAll("mentions");
  const has = params.getAll("has");
  const min = params.get("min_id");
  const max = params.get("max_id");
  const pinned = params.get("pinned");
  const carries = (message: Message, what: string) => {
    const types = message.files.map((file) => file.contentType);
    switch (what) {
      case "image": return types.some((type) => type.startsWith("image/"));
      case "video": return types.some((type) => type.startsWith("video/"));
      case "sound": return types.some((type) => type.startsWith("audio/"));
      case "file": return message.files.length > 0;
      case "embed": return (message.embeds?.("").length ?? 0) > 0;
      case "link": return /https?:\/\//.test(message.content);
      case "sticker": return (message.stickers?.length ?? 0) > 0;
      default: return false;
    }
  };
  return (message) =>
    message.content.toLowerCase().includes(content) &&
    (!authors.length || authors.includes(message.author.id)) &&
    mentioned.every((id) => message.content.includes(`<@${id}>`)) &&
    has.every((what) => carries(message, what)) &&
    (!min || BigInt(message.id) > BigInt(min)) &&
    (!max || BigInt(message.id) < BigInt(max)) &&
    (pinned === null || (pins.get(message.channel)?.some((pin) => pin.message.id === message.id) ?? false) === (pinned === "true"));
}
// DMs searched once, which Discord has indexed.
const indexed = new Set<string>();
const readBob = post(DM_BOB, users.alice, "Yes, bringing snacks", DM_START + 180_000);
post(DM_BOB, users.bob, "Great", DM_START + 240_000);
post(DM_BOB, users.bob, "See you at nine", DM_START + 300_000);
privates.set(REQUEST_ERIN, { type: 1, recipients: [strangers.erin!], request: true });
privates.set(SPAM_FRANK, { type: 1, recipients: [strangers.frank!], request: true, spam: true });
post(SPAM_FRANK, strangers.frank!, "Claim your free Nitro at nitro-gift.example", DM_START + 360_000);
post(REQUEST_ERIN, strangers.erin!, "Hi Alice, Bob gave me your name. Can I ask about the trip?", DM_START + 420_000);

// --- Threads ----------------------------------------------------------------

// Threads of "Server" by id, each with its messages in `messages`. One
// started from a message has that message's id.
type Thread = {
  id: string; parent: string; name: string; owner: User; type: 10 | 11 | 12;
  archived: boolean; locked: boolean; created: number;
  /** Minutes of quiet that archive it, and when it was last archived or
   *  opened again, in Unix milliseconds. */
  autoArchive: number; archiveChanged: number;
  /** Who is in it, and since when, in Unix milliseconds. */
  members: Map<string, number>;
};
const threads = new Map<string, Thread>();

/** Alice's thread member, as a thread she's in carries it. */
function wireThreadMember(thread: Thread) {
  return {
    id: thread.id, user_id: ALICE, join_timestamp: iso(thread.members.get(ALICE)!), flags: 1,
    muted: false, mute_config: null,
  };
}

/** A thread's channel object, with Alice's member when she's in it. */
function wireThread(thread: Thread) {
  const list = messages.get(thread.id)!;
  // Discord doesn't count a thread's first message, the one it was started from.
  const count = list.filter((message) => message.type !== 21).length;
  return {
    id: thread.id, type: thread.type, guild_id: GUILD, parent_id: thread.parent, owner_id: thread.owner.id,
    name: thread.name, flags: 0, rate_limit_per_user: 0,
    last_message_id: list.at(-1)?.id ?? null,
    message_count: count, total_message_sent: count, member_count: thread.members.size,
    member_ids_preview: [...thread.members.keys()].slice(0, 5),
    thread_metadata: {
      archived: thread.archived, auto_archive_duration: thread.autoArchive, archive_timestamp: iso(thread.archiveChanged),
      locked: thread.locked, create_timestamp: iso(thread.created),
    },
    ...(thread.members.has(ALICE) ? { member: wireThreadMember(thread) } : {}),
  };
}

/** Makes a thread, in #general unless it says otherwise. */
function makeThread(
  id: string, name: string, owner: User, created: number, members: string[], extra: Partial<Thread> = {},
): Thread {
  const thread: Thread = {
    id, parent: GENERAL, name, owner, type: 11, archived: false, locked: false, created,
    autoArchive: 4320, archiveChanged: created,
    members: new Map(members.map((user) => [user, created])),
    ...extra,
  };
  threads.set(id, thread);
  messages.set(id, []);
  return thread;
}

/** Adds Alice to a thread she writes in, as Discord does, telling her. */
function joinThread(thread: Thread) {
  if (thread.members.has(ALICE)) return;
  thread.members.set(ALICE, Date.now());
  dispatch("THREAD_MEMBERS_UPDATE", {
    id: thread.id, guild_id: GUILD, member_count: thread.members.size,
    added_members: [{ ...wireThreadMember(thread), member: member(users.alice), presence: null }],
    removed_member_ids: [],
  });
  dispatch("THREAD_CREATE", { ...wireThread(thread), newly_created: false });
}

// "Plans", from Bob's welcome: its first message points back at it.
// It was opened again as the fake started, so it lists under #general for
// its three days, as a thread this quiet otherwise wouldn't.
const plans = makeThread(welcome.id, "Plans", users.bob, HISTORY_START + 20_000, [users.bob.id, users.carol.id, ALICE], { archiveChanged: Date.now() });
post(plans.id, users.bob, "", HISTORY_START + 20_000, { type: 21, starter: welcome.id });
post(plans.id, users.bob, "Saturday works for me", HISTORY_START + 40_000);
const plansRead = post(plans.id, users.carol, "Count me in", HISTORY_START + 50_000);

// "Book club", Carol's, which Alice isn't in, is one of #general's other
// active threads; "Surprise for Bob" is private, and hidden from her.
const bookClub = makeThread(nextId(HISTORY_START + 60_000), "Book club", users.carol, HISTORY_START + 60_000, [users.carol.id, users.bob.id]);
post(bookClub.id, users.carol, "First up: Piranesi", HISTORY_START + 61_000);
post(bookClub.id, users.bob, "Chapter 3 by Friday?", HISTORY_START + 62_000);
const surprise = makeThread(nextId(HISTORY_START + 63_000), "Surprise for Bob", users.carol, HISTORY_START + 63_000, [users.carol.id, users.dave.id], { type: 12 });
post(surprise.id, users.carol, "Cake or pie?", HISTORY_START + 64_000);
// #general's older threads, archived, one a day back from the fixture's
// start, more than a page of them: Alice is in every fourth.
const OLD_TOPICS = ["Game night", "Movie picks", "Lunch spots", "Bug hunt", "Road trip", "Playlist"];
for (let index = 0; index < 30; index++) {
  const created = HISTORY_START - (index + 1) * 86_400_000;
  const owner = [users.bob, users.carol, users.dave][index % 3]!;
  const members = index % 4 === 0 ? [owner.id, ALICE] : [owner.id];
  const thread = makeThread(snowflake(created, 900), `${OLD_TOPICS[index % OLD_TOPICS.length]} ${index + 1}`, owner, created, members, { archived: true });
  post(thread.id, owner, `Wrapping up ${thread.name}`, created + 3_600_000, { id: snowflake(created + 3_600_000, 901) });
}

/** Whether Alice may see a thread: a private one only once she's in it. */
const visibleThread = (thread: Thread) => thread.type !== 12 || thread.members.has(ALICE);

/** When a thread was last active: its newest message, else its start. */
const threadActivity = (thread: Thread) => BigInt(messages.get(thread.id)!.at(-1)?.id ?? thread.id);

/** Each page of reactors asked for, as GET /_control/reactor-pages lists them:
 *  the message, the emoji and the query. */
const reactorPages: string[] = [];
/** Each reaction a moderator removed, as GET /_control/reactor-removals lists
 *  them: the message, the emoji and the user. */
const reactorRemovals: string[] = [];
/** Each search of a channel's threads, as GET /_control/thread-searches lists them. */
const threadSearches: string[] = [];
/** Each thread Alice started: the channel, the message, if any, and the body
 *  she sent, as GET /_control/thread-creates lists them. */
const threadCreates: Record<string, unknown>[] = [];

/** Discord's answer to a thread name that's empty or longer than 100. */
function threadNameError(name: unknown): Response | undefined {
  const length = typeof name === "string" ? [...name.trim()].length : 0;
  if (length >= 1 && length <= 100) return undefined;
  return json({
    message: "Invalid Form Body", code: 50035,
    errors: { name: { _errors: [{ code: "BASE_TYPE_BAD_LENGTH", message: "Must be between 1 and 100 in length." }] } },
  }, 400);
}

/** Tells Alice of a thread she started: THREAD_CREATE, then her joining it. */
function announceThread(thread: Thread) {
  const { member: _member, ...wire } = wireThread(thread) as Record<string, unknown>;
  dispatch("THREAD_CREATE", { ...wire, newly_created: true });
  dispatch("THREAD_MEMBERS_UPDATE", {
    id: thread.id, guild_id: GUILD, member_count: thread.members.size,
    member_ids_preview: [...thread.members.keys()].slice(0, 5),
    added_members: [{ ...wireThreadMember(thread), member: member(users.alice), presence: null }],
    removed_member_ids: [],
  });
}

/** A notice in a thread's channel that it started, as type 18. */
function noticeThread(thread: Thread, origin: string) {
  const notice = post(thread.parent, users.alice, thread.name, Date.now(), { type: 18, notice: thread.id });
  dispatch("MESSAGE_CREATE", dispatched(notice, origin));
}

// Alice has read all but the last `--unread` messages of the fixture, and Bob's
// last two in their DM, which count as mentions as in Discord.
const general = messages.get(GENERAL)!;
const readStates = new Map<string, { last: string; mentions: number }>([
  [GENERAL, { last: general[Math.max(0, general.length - 1 - Number(args.unread))]!.id, mentions: 0 }],
  [DM_BOB, { last: readBob.id, mentions: 2 }],
  [plans.id, { last: plansRead.id, mentions: 0 }],
  [REQUEST_ERIN, { last: readBob.id, mentions: 1 }],
  [SPAM_FRANK, { last: readBob.id, mentions: 1 }],
]);

// --- Voice ------------------------------------------------------------------

// Bob shares his screen with Go Live.
// A voice state's guild is null in a DM or group DM call.
// `mute` and `deaf` are the server's, which a moderator sets.
type VoiceState = {
  guild: string | null; channel: string; self_mute: boolean; self_deaf: boolean; session: string; self_stream?: boolean; self_video?: boolean;
  mute?: boolean; deaf?: boolean;
};
/** The session of Alice's other client, such as her phone. */
const OTHER_CLIENT = "voice-alice-phone";
const startingVoice = (): [string, VoiceState][] => [
  [users.bob.id, { guild: GUILD, channel: LOUNGE, self_mute: false, self_deaf: false, session: "voice-bob", self_stream: true }],
  [users.carol.id, { guild: GUILD, channel: LOUNGE, self_mute: true, self_deaf: false, session: "voice-carol" }],
  [users.dave.id, { guild: GUILD, channel: LOUNGE, self_mute: true, self_deaf: true, session: "voice-dave" }],
];
const voice = new Map<string, VoiceState>(startingVoice());

// Go Live streams by stream key; Bob's is running from the start. A DM
// call's streams are keyed `call:{channel}:{user}`.
type Stream = { viewers: Set<string>; paused: boolean };
const streamKey = (guild: string | null, channel: string, user: string) =>
  guild ? `guild:${guild}:${channel}:${user}` : `call:${channel}:${user}`;
/** The channel and streamer a stream key names. */
function streamOf(key: string): { channel?: string; user?: string } {
  const parts = key.split(":");
  return parts[0] === "call" ? { channel: parts[1], user: parts[2] } : { channel: parts[2], user: parts[3] };
}
const streams = new Map<string, Stream>([[streamKey(GUILD, LOUNGE, users.bob.id), { viewers: new Set(), paused: false }]]);

function wireStream(key: string, stream: Stream) {
  return { stream_key: key, rtc_server_id: "fake-rtc", region: "fake", viewer_ids: [...stream.viewers], paused: stream.paused };
}

/** A voice state; one that left says where it was in `left`. */
function wireVoiceState(userId: string, state: VoiceState | undefined, left: string | null = GUILD) {
  return {
    guild_id: state ? state.guild : left,
    channel_id: state?.channel ?? null,
    user_id: userId,
    session_id: state?.session ?? "voice-left",
    self_mute: state?.self_mute ?? false,
    self_deaf: state?.self_deaf ?? false,
    self_video: state?.self_video ?? false,
    self_stream: state?.self_stream ?? false,
    mute: state?.mute ?? false,
    deaf: state?.deaf ?? false,
    suppress: false,
    request_to_speak_timestamp: null,
  };
}

// --- Gateway ----------------------------------------------------------------

type SocketData = {
  sequence: number;
  identified: boolean;
  session: string;
  origin: string;
  voice?: boolean;
  /** The token a voice connection identified with. */
  voiceToken?: string;
  /** A stream connection: its key, from the token it identified with. */
  stream?: string;
  ssrc?: number;
  /** A viewer's stream, which starts once it asks for video with op 15. */
  waiting?: { key: Buffer; ssrc: number };
  /** A call connection's transport key, once it selected the protocol. */
  callKey?: Buffer;
  /** The cameras on in this call, by who films, as op 12 announced them. */
  cameras?: Set<string>;
  /** The member list this session subscribed to last, by its id. */
  listing?: string;
  /** Its server's active threads were synced, as a subscription with
   *  `threads` does once. */
  threadsSynced?: boolean;
  /** A remote login: the client's key, the nonce it must prove, and the code. */
  remoteAuth?: { key?: CryptoKey; nonce?: Uint8Array; fingerprint?: string; scanned?: boolean; timer?: Timer };
};
type Socket = ServerWebSocket<SocketData>;
const sockets = new Set<Socket>();
let sessions = 0;
/** Sessions whose connection dropped, with the events they missed, until
 *  they resume. A close with 1000 or 1001 ends the session instead. */
const parked = new Map<string, { sequence: number; missed: { name: string; data: unknown }[] }>();
/** How long history and media take to answer, set by POST /_control/slow. */
let slowMs = 0;
/** #general's topic, which POST /_control/topic changes; an empty body
 *  takes it away. */
let generalTopic: string | null =
  `Co-op nights, plans and links. Ask <@${users.bob.id}> to join, keep <#${GENERAL}> on topic, and read the **house rules** at https://example.com/rules first.`;
/** How often clients asked for a member list, for GET /_control/list-requests. */
let listRequests = 0;
/** How often clients identified and resumed, for GET /_control/sessions. */
const sessionCounts = { identifies: 0, resumes: 0 };

function send(ws: Socket, op: number, d: unknown, t: string | null = null) {
  const s = op === 0 ? ++ws.data.sequence : null;
  ws.send(JSON.stringify({ op, d, s, t }));
}

function dispatch(name: string, data: unknown) {
  for (const ws of sockets) if (ws.data.identified) send(ws, 0, data, name);
  for (const session of parked.values()) session.missed.push({ name, data });
}

const serverRoles = [
  { id: GUILD, name: "@everyone", permissions: String(EVERYONE), position: 0, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
  { id: MODS, name: "Mods", permissions: String(bit(13)), position: 1, color: 3447003, hoist: true, managed: false, mentionable: true, flags: 0 },
  { id: BOTS, name: "Bots", permissions: "0", position: 2, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
  // Crew may manage channels.
  { id: CREW, name: "Crew", permissions: String(bit(4)), position: 3, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
  { id: STAFF, name: "Staff", permissions: String(MODERATION), position: 4, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
  { id: LEADS, name: "Leads", permissions: "0", position: 5, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
];

/** The highest position of a member's roles; the owner is above all. */
function rank(user: string): number {
  if (user === users.bob.id) return Infinity;
  const positions = (memberRoles.get(user) ?? []).map((id) => serverRoles.find((role) => role.id === id)?.position ?? 0);
  return Math.max(0, ...positions);
}

/** A member's permissions in "Server", as a BigInt: everything for the owner and administrators. */
function serverPermissions(user: string): bigint {
  const all = (1n << 53n) - 1n;
  if (user === users.bob.id) return all;
  let held = BigInt(EVERYONE);
  for (const id of memberRoles.get(user) ?? []) held |= BigInt(serverRoles.find((role) => role.id === id)?.permissions ?? "0");
  return held & (1n << 3n) ? all : held;
}

/** Whether Alice may do what `permission`, a bit number, allows to `user`: she holds it and outranks them. */
function aliceMay(permission: number, user?: string): boolean {
  const holds = (serverPermissions(ALICE) & (1n << BigInt(permission))) !== 0n;
  return holds && (user === undefined || (user !== ALICE && rank(ALICE) > rank(user)));
}

/** A channel of "Server" as Discord sends it. */
function guildChannel(id: string, name: string, type: number, position: number, parent: string | null) {
  return {
    id,
    type,
    name,
    position,
    parent_id: parent,
    guild_id: GUILD,
    flags: 0,
    permission_overwrites: [] as unknown[],
    ...(type === 0
      ? { topic: (id === GENERAL ? generalTopic : null) as string | null, nsfw: false, rate_limit_per_user: 0, last_message_id: messages.get(id)?.at(-1)?.id ?? null }
      : {}),
    ...(type === 2 ? { bitrate: 64000, user_limit: 0, rtc_region: null, last_message_id: null } : {}),
  };
}
type GuildChannel = ReturnType<typeof guildChannel>;

// Channels and categories of "Server" made, changed and deleted through the
// API: POST /guilds/{id}/channels, PATCH and DELETE /channels/{id}.
const madeChannels = new Map<string, GuildChannel>();
const channelEdits = new Map<string, Partial<GuildChannel>>();
const deletedChannels = new Set<string>();

// Whether Alice left "Server"; see DELETE /users/@me/guilds/{id}.
let leftServer = false;

/** "Server"'s channels as they are now. */
function guildChannels(): GuildChannel[] {
  const channel = guildChannel;
  return [
    channel(CATEGORY, "Channels", 4, 0, null),
    channel(GENERAL, "general", 0, 0, CATEGORY),
    channel(LOUNGE, "Lounge", 2, 1, CATEGORY),
    channel(CREATE_ROOM, "Create Room", 2, 2, CATEGORY),
    ...(roomMade ? [channel(ALICES_ROOM, "Alice's room", 2, 3, CATEGORY)] : []),
    ...rooms.map((id, n) => channel(id, `room-${n + 1}`, 0, 2 + n, CATEGORY)),
    ...madeChannels.values(),
  ]
    .filter((channel) => !deletedChannels.has(channel.id))
    .map((channel) => ({ ...channel, ...channelEdits.get(channel.id) }));
}

function guild() {
  const properties = {
    id: GUILD,
    name: "Server",
    owner_id: users.bob.id,
    icon: "server1",
    banner: null,
    splash: null,
    description: null,
    features: [],
    preferred_locale: "en-US",
    verification_level: 0,
    default_message_notifications: 1,
    explicit_content_filter: 0,
    mfa_level: 0,
    nsfw_level: 0,
    premium_tier: 0,
    system_channel_id: GENERAL,
    afk_channel_id: null,
    afk_timeout: 300,
  };
  return {
    id: GUILD,
    properties,
    channels: guildChannels(),
    roles: serverRoles,
    emojis: [
      { id: EMOJI_TALK, name: "talk", animated: false, available: true, managed: false, require_colons: true, roles: [] },
      { id: EMOJI_PARTY, name: "party", animated: true, available: true, managed: false, require_colons: true, roles: [] },
    ],
    stickers: [],
    // A user account hears of the active threads it's in.
    threads: [...threads.values()].filter((thread) => thread.members.has(ALICE) && !thread.archived).map(wireThread),
    stage_instances: [],
    guild_scheduled_events: [],
    activity_instances: [],
    member_count: Object.keys(users).length,
    joined_at: JOINED_AT,
    large: false,
    lazy: true,
    premium_subscription_count: 0,
    data_mode: "full",
    version: 1,
  };
}

// --- Filler servers -------------------------------------------------------

// What a large account's other servers hold, each: channels, in categories of
// ten, roles and custom emoji.
const FILLER_CHANNELS = 50;
const FILLER_ROLES = 50;
const FILLER_EMOJI = 50;
const FILLER_START = 1_500_000_000_000_000_000n;

/** The filler servers' text channels, with the server each is in. */
const fillerChannels = new Map<string, string>();

function fillerId(server: number, item: number): string {
  return (FILLER_START + BigInt(server) * 1000n + BigInt(item)).toString();
}

const fillers = Array.from({ length: Math.max(0, Number(args.servers) - 1) }, (_, index) => {
  const id = fillerId(index, 0);
  const role = (n: number) => fillerId(index, 300 + n);
  const channels = [];
  for (let n = 0; n < FILLER_CHANNELS; n++) {
    const category = fillerId(index, 900 + Math.floor(n / 10));
    if (n % 10 === 0) {
      channels.push({ id: category, type: 4, name: `Group ${n / 10 + 1}`, position: n / 10, parent_id: null, guild_id: id, flags: 0, permission_overwrites: [] });
    }
    const channel = fillerId(index, 1 + n);
    fillerChannels.set(channel, id);
    messages.set(channel, []);
    channels.push({
      id: channel,
      type: 0,
      name: `channel-${n + 1}`,
      position: n % 10,
      parent_id: category,
      guild_id: id,
      flags: 0,
      topic: null,
      nsfw: false,
      rate_limit_per_user: 0,
      last_message_id: null,
      permission_overwrites: [
        { id: role(n % FILLER_ROLES), type: 0, allow: String(bit(11)), deny: "0" },
        { id: role((n + 7) % FILLER_ROLES), type: 0, allow: "0", deny: String(bit(10)) },
      ],
    });
  }
  return {
    id,
    properties: { id, name: `Filler ${index + 2}`, owner_id: users.bob.id, icon: null, banner: null, premium_tier: index % 4 },
    channels,
    roles: [
      { id, name: "@everyone", permissions: String(EVERYONE), position: 0, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
      ...Array.from({ length: FILLER_ROLES - 1 }, (_, n) => ({
        id: role(n + 1),
        name: `Role ${n + 1}`,
        permissions: "0",
        position: n + 1,
        color: (n * 2654435761) % 0xffffff,
        hoist: n % 5 === 0,
        managed: false,
        mentionable: n % 3 === 0,
        flags: 0,
      })),
    ],
    emojis: Array.from({ length: FILLER_EMOJI }, (_, n) => ({
      id: fillerId(index, 600 + n),
      name: `emoji_${n}`,
      animated: n % 7 === 0,
      available: true,
      managed: false,
      require_colons: true,
      roles: [],
    })),
    stickers: [],
    threads: [],
    stage_instances: [],
    guild_scheduled_events: [],
    activity_instances: [],
    member_count: 1000 + index,
    // Joined before "Server", so it lists first.
    joined_at: "2025-01-01T09:00:00.000000+00:00",
    large: true,
    lazy: true,
  };
});
const fillerList = [...fillerChannels.keys()];

// The server list in Alice's settings, GuildFolders (field 14): "Server" on
// its own, then each folder, then the other servers on their own. A folder
// has an id, an Int64Value, and may have a name and a colour, wrappers too.
if (args.folders) {
  const varint = (field: number, value: bigint) => {
    const out = [field << 3];
    for (; value >= 0x80n; value >>= 7n) out.push(Number(value & 0x7fn) | 0x80);
    return new Uint8Array([...out, Number(value)]);
  };
  const entry = (servers: string[], id?: number, name?: string, color?: number) => {
    const packed = new Uint8Array(servers.length * 8);
    servers.forEach((server, index) => new DataView(packed.buffer).setBigUint64(index * 8, BigInt(server), true));
    return protoBytes(1, new Uint8Array([
      ...protoBytes(1, packed),
      ...(id === undefined ? [] : protoBytes(2, varint(1, BigInt(id)))),
      ...(name === undefined ? [] : protoBytes(3, protoBytes(1, text(name)))),
      ...(color === undefined ? [] : protoBytes(4, varint(1, BigInt(color)))),
    ]));
  };
  const ids = fillers.map((filler) => filler.id);
  const list = [
    entry([GUILD]),
    entry(ids.slice(0, 3), 1, "Games", 0xed4245),
    entry(ids.slice(3, 5), 2),
    ...ids.slice(5).map((id) => entry([id])),
  ];
  const folders = new Uint8Array(list.flatMap((bytes) => [...bytes]));
  settings = Buffer.concat([Buffer.from(settings, "base64"), protoBytes(14, folders)]).toString("base64");
}

function ready(ws: Socket) {
  const { user: _, ...alice } = member(users.alice);
  return {
    v: 9,
    user: {
      ...wireUser(users.alice),
      email: "alice@example.org",
      verified: true,
      mfa_enabled: false,
      premium_type: nitro ? 2 : 0,
      flags: 0,
      bot: false,
      phone: null,
      locale: "en-US",
      nsfw_allowed: true,
    },
    users: [...Object.values(users), ...Object.values(strangers), ...fillerPeople].filter((user) => user.id !== ALICE).map(wireUser),
    guilds: [...(leftServer ? [] : [guild()]), ...fillers],
    merged_members: [...(leftServer ? [] : [[alice]]), ...fillers.map((filler) => [{ ...alice, roles: [filler.roles[1]!.id, filler.roles[4]!.id] }])],
    read_state: {
      entries: [...readStates].map(([id, read]) => ({
        id,
        last_message_id: read.last,
        mention_count: read.mentions,
        last_pin_timestamp: "1970-01-01T00:00:00+00:00",
        flags: 0,
      })),
      partial: false,
      version: 1,
    },
    user_guild_settings: {
      entries: [
        wireGuildSettings(GUILD),
        wireGuildSettings("@me"),
      ],
      partial: false,
      version: 1,
    },
    private_channels: [...privates].map(([id, channel]) => wirePrivate(id, channel, "ids")),
    relationships: [...relationships].map(([id, type]) => wireRelationship(id, type)),
    connected_accounts: [],
    guild_join_requests: [],
    experiments: [],
    guild_experiments: [],
    sessions: [
      {
        session_id: ws.data.session,
        status: "online",
        activities: [],
        client_info: { client: "web", os: "linux", version: 0 },
      },
    ],
    session_id: ws.data.session,
    session_type: "normal",
    resume_gateway_url: `${ws.data.origin.replace(/^http/, "ws")}`,
    auth_session_id_hash: "fake",
    country_code: "US",
    analytics_token: "fake",
    api_code_version: 1,
    user_settings_proto: settings,
    notification_settings: { flags: 0 },
    consents: { personalization: { consented: false } },
    tutorial: null,
    auth: {},
  };
}

function readySupplemental() {
  const inServer = [...voice].filter(([, state]) => state.guild === GUILD);
  const inVoice = inServer.map(([id]) => id);
  if (leftServer) {
    return {
      guilds: [],
      merged_members: [],
      merged_presences: {
        guilds: [],
        friends: [users.bob, users.carol, users.dave].map((user) => ({ user_id: user.id, ...presences[user.id] })),
      },
      lazy_private_channels: [],
      disclose: [],
      game_invites: [],
    };
  }
  return {
    guilds: [
      {
        id: GUILD,
        voice_states: inServer.map(([userId, state]) => {
          const { guild_id: _, ...wire } = wireVoiceState(userId, state);
          return wire;
        }),
        embedded_activities: [],
        activity_instances: [],
      },
    ],
    merged_members: [
      inVoice.map((id) => {
        const { user: _, ...profile } = member(userById.get(id)!);
        return profile;
      }),
    ],
    merged_presences: {
      guilds: [[]],
      friends: [users.bob, users.carol, users.dave].map((user) => ({ user_id: user.id, ...presences[user.id] })),
    },
    lazy_private_channels: [],
    disclose: [],
    game_invites: [],
  };
}

// Who is around and what they're doing, as their presence says.
const presences: Record<string, { status: string; activities: unknown[]; client_status?: Record<string, string> }> = {
  [users.alice.id]: { status: "online", activities: [] },
  [users.bob.id]: { status: "dnd", activities: [{ type: 0, name: "Factorio" }] },
  [users.carol.id]: {
    status: "idle",
    activities: [{ type: 4, name: "Custom Status", state: "back later\nafter dinner, probably", emoji: { name: "🌙", id: null } }],
  },
  [users.dave.id]: { status: "offline", activities: [] },
};

// The member list of a channel everyone reads, as Discord groups it: Bob under
// the hoisted Mods role, then who is online, then Dave, who isn't.
/** A channel's member list id, as Discord's client works it out: everyone
 *  when @everyone sees it and no overwrite hides it, else a hash of the
 *  overwrites that allow or deny seeing it. */
function listId(channelId: string): string {
  const view = BigInt(bit(10));
  const overwrites = (guildChannels().find((channel) => channel.id === channelId)?.permission_overwrites ??
    []) as { id: string; allow: string; deny: string }[];
  if (BigInt(EVERYONE) & view && !overwrites.some((overwrite) => BigInt(overwrite.deny) & view)) return "everyone";
  const readers = overwrites.flatMap((overwrite) =>
    BigInt(overwrite.allow) & view ? [`allow:${overwrite.id}`] : BigInt(overwrite.deny) & view ? [`deny:${overwrite.id}`] : [],
  );
  return String(Bun.hash.murmur32v3(readers.sort().join(","), 0));
}

function memberList(id = "everyone") {
  const offline = (user: User) => presences[user.id]!.status === "offline";
  const present = Object.values(users).filter((user) => !removed.has(user.id));
  const mods = present.filter((user) => memberRoles.get(user.id)?.includes(MODS));
  const others = present.filter((user) => !mods.includes(user));
  const groups = [
    { id: MODS, count: mods.length },
    { id: "online", count: others.filter((user) => !offline(user)).length },
    { id: "offline", count: others.filter(offline).length },
  ];
  const row = (user: User) => ({
    member: {
      ...member(user),
      presence: { user: { id: user.id }, client_status: {}, ...presences[user.id] },
    },
  });
  const items = [
    ...(mods.length > 0 ? [{ group: groups[0] }] : []),
    ...mods.map(row),
    { group: groups[1] },
    ...others.filter((user) => !offline(user)).map(row),
    { group: groups[2] },
    ...others.filter(offline).map(row),
  ];
  return {
    guild_id: GUILD,
    id,
    member_count: present.length,
    online_count: groups[0]!.count + groups[1]!.count,
    groups,
    ops: [{ op: "SYNC", range: [0, 99], items }],
  };
}

/** Makes a change to the members, which dispatches its own events, then
 *  tells subscribers how the member list's rows changed, as Discord does:
 *  the rows that went, the rows that came, and those that changed in place. */
function changeMembers(change: () => void) {
  const rows = (list: ReturnType<typeof memberList>) => list.ops[0]!.items as any[];
  const before = rows(memberList());
  change();
  const list = memberList();
  const ops = listOps(before, rows(list));
  if (ops.length > 0) dispatch("GUILD_MEMBER_LIST_UPDATE", { ...list, ops });
}

/** DELETE, INSERT and UPDATE operations that turn the rows `before` into
 *  `after`: those outside their longest common run of rows go and come. */
function listOps(before: any[], after: any[]) {
  const key = (item: any) => (item.group ? `group:${item.group.id}` : `member:${item.member.user.id}`);
  const [a, b] = [before.map(key), after.map(key)];
  const common = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      common[i]![j] = a[i] === b[j] ? common[i + 1]![j + 1]! + 1 : Math.max(common[i + 1]![j]!, common[i]![j + 1]!);
    }
  }
  const kept: [number, number][] = [];
  for (let i = 0, j = 0; i < a.length && j < b.length; ) {
    if (a[i] === b[j]) kept.push([i++, j++]);
    else if (common[i + 1]![j]! >= common[i]![j + 1]!) i++;
    else j++;
  }
  const ops: any[] = [];
  for (let i = a.length - 1; i >= 0; i--) if (!kept.some(([k]) => k === i)) ops.push({ op: "DELETE", index: i });
  for (let j = 0; j < b.length; j++) if (!kept.some(([, k]) => k === j)) ops.push({ op: "INSERT", index: j, item: after[j] });
  for (const [i, j] of kept) {
    if (JSON.stringify(before[i]) !== JSON.stringify(after[j])) ops.push({ op: "UPDATE", index: j, item: after[j] });
  }
  return ops;
}

/** Tells everyone a member of "Server" changed. */
function memberUpdated(user: User) {
  dispatch("GUILD_MEMBER_UPDATE", { guild_id: GUILD, ...member(user), user_id: undefined });
}

// --- Remote login ---------------------------------------------------------
// Linking by QR code: the client sends its RSA public key, proves it can read
// a nonce encrypted to it, and gets a code to show. Scanning sends who
// scanned, and confirming a ticket that the API trades for the login, both
// encrypted to the key.

const REMOTE_AUTH_TIMEOUT_MS = 120_000;
const remoteLogins = new Set<Socket>();
// How many times Alice signed out with POST /auth/logout.
let logouts = 0;
// Each confirmed login's ticket, with the key to encrypt the login to.
const tickets = new Map<string, CryptoKey>();
/** Whether trading a ticket asks for a captcha first. */
let ticketCaptcha = false;
const TICKET_CAPTCHA = {
  captcha_key: ["captcha-required"],
  captcha_service: "hcaptcha",
  captcha_sitekey: "fake-sitekey",
  captcha_rqdata: "fake-rqdata",
  captcha_rqtoken: "fake-rqtoken",
  captcha_session_id: "fake-session",
};

async function seal(key: CryptoKey, data: Uint8Array | string): Promise<string> {
  const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
  return Buffer.from(await crypto.subtle.encrypt({ name: "RSA-OAEP" }, key, bytes)).toString("base64");
}

async function remoteAuthMessage(ws: Socket, text: string) {
  const login = ws.data.remoteAuth!;
  const message = JSON.parse(text);
  switch (message.op) {
    case "heartbeat":
      ws.send(JSON.stringify({ op: "heartbeat_ack" }));
      break;
    case "init":
      login.key = await crypto.subtle.importKey(
        "spki",
        Buffer.from(String(message.encoded_public_key), "base64"),
        { name: "RSA-OAEP", hash: "SHA-256" },
        false,
        ["encrypt"],
      );
      login.nonce = crypto.getRandomValues(new Uint8Array(32));
      ws.send(JSON.stringify({ op: "nonce_proof", encrypted_nonce: await seal(login.key, login.nonce) }));
      break;
    case "nonce_proof": {
      const expected = Buffer.from(await crypto.subtle.digest("SHA-256", login.nonce!)).toString("base64url");
      if (message.proof !== expected) {
        ws.close(4003, "Invalid nonce proof");
        return;
      }
      login.fingerprint = Buffer.from(crypto.getRandomValues(new Uint8Array(24))).toString("base64url");
      ws.send(JSON.stringify({ op: "pending_remote_init", fingerprint: login.fingerprint }));
      break;
    }
  }
}

/** Does `step` to every remote login showing a code, and says how many there were. */
async function eachRemoteLogin(step: (ws: Socket, key: CryptoKey) => Promise<void> | void): Promise<Response> {
  const showing = [...remoteLogins].filter((ws) => ws.data.remoteAuth?.fingerprint && ws.data.remoteAuth.key);
  for (const ws of showing) await step(ws, ws.data.remoteAuth!.key!);
  return new Response(`${showing.length} codes\n`);
}

function gatewayMessage(ws: Socket, text: string) {
  let payload: { op?: number; d?: any };
  try {
    payload = JSON.parse(text);
  } catch {
    console.error("gateway: not JSON:", text.slice(0, 200));
    return;
  }
  const d = payload.d ?? {};
  switch (payload.op) {
    case 1: // Heartbeat
      send(ws, 11, null);
      break;
    case 2: // Identify
      if (d.token !== TOKEN) {
        console.error("gateway: identify with an unexpected token; closing");
        ws.close(4004, "Authentication failed.");
        return;
      }
      ws.data.identified = true;
      sessionCounts.identifies++;
      send(ws, 0, ready(ws), "READY");
      send(ws, 0, readySupplemental(), "READY_SUPPLEMENTAL");
      // Calls going on, as Talk's capabilities ask.
      for (const channel of calls.keys()) send(ws, 0, wireCall(channel), "CALL_CREATE");
      break;
    case 6: {
      // Resume: replay what the session missed, then RESUMED.
      const session = parked.get(d.session_id);
      if (d.token !== TOKEN || !session || d.seq !== session.sequence) {
        console.error(`gateway: can't resume ${d.session_id} at ${d.seq}`);
        send(ws, 9, false);
        return;
      }
      parked.delete(d.session_id);
      ws.data.session = d.session_id;
      ws.data.sequence = session.sequence;
      ws.data.identified = true;
      sessionCounts.resumes++;
      console.log(`gateway: resumed ${d.session_id}, replaying ${session.missed.length} events`);
      for (const event of session.missed) send(ws, 0, event.data, event.name);
      send(ws, 0, {}, "RESUMED");
      break;
    }
    case 3: // Presence
      alicePresence = d;
      console.log(`gateway: presence ${JSON.stringify(d)}`);
      break;
    case 4: {
      // Voice state: Alice shows in the channel or DM call, and gets its voice server.
      const channel: string | null = d.channel_id ?? null;
      const guild: string | null = d.guild_id ?? null;
      const before = voice.get(ALICE);
      if (before && before.channel !== channel) endStream(streamKey(before.guild, before.channel, ALICE), "user_requested");
      if (channel) {
        voice.set(ALICE, {
          guild,
          channel,
          self_mute: !!d.self_mute,
          self_deaf: !!d.self_deaf,
          session: ws.data.session,
          self_stream: before?.channel === channel && before.self_stream,
          self_video: !!d.self_video,
        });
      } else {
        voice.delete(ALICE);
      }
      moved(ALICE, before, ws.data.origin);
      // Joining from another session, as after a client that never left,
      // moves the voice state to this one, which gets a voice server too.
      if (channel && (before?.channel !== channel || before.session !== ws.data.session)) {
        const server = guild
          ? { guild_id: GUILD, token: VOICE_TOKEN }
          : { guild_id: null, channel_id: channel, token: callToken(channel) };
        if (guild) voiceToken = VOICE_TOKEN;
        send(ws, 0, { ...server, endpoint: voiceEndpoint(ws) }, "VOICE_SERVER_UPDATE");
        if (channel === CREATE_ROOM) makeRoom(ws);
      }
      break;
    }
    case 18: {
      // Go Live: only into the voice channel or DM call Alice is in.
      const guild: string | null = d.guild_id ?? null;
      const key = streamKey(guild, d.channel_id, ALICE);
      const state = voice.get(ALICE);
      if (state?.guild !== guild || state?.channel !== d.channel_id || d.type !== (guild ? "guild" : "call")) {
        send(ws, 0, { stream_key: key, unavailable: false, reason: "unauthorized" }, "STREAM_DELETE");
        break;
      }
      // A new stream starts paused until its streamer unpauses it with op 22.
      const stream = { viewers: new Set<string>(), paused: true };
      streams.set(key, stream);
      state.self_stream = true;
      announce(ALICE);
      send(ws, 0, wireStream(key, stream), "STREAM_CREATE");
      send(ws, 0, streamServer(ws, key), "STREAM_SERVER_UPDATE");
      break;
    }
    case 19: // Stop streaming or watching
      if (d.stream_key?.endsWith(`:${ALICE}`)) {
        endStream(d.stream_key, "user_requested");
      } else {
        const stream = streams.get(d.stream_key);
        if (stream?.viewers.delete(ALICE)) send(ws, 0, { stream_key: d.stream_key, unavailable: false, reason: "user_requested" }, "STREAM_DELETE");
      }
      break;
    case 20: {
      // Watch: a running stream in the voice channel Alice is in.
      const key: string = d.stream_key ?? "";
      const stream = streams.get(key);
      const { channel } = streamOf(key);
      if (!stream || voice.get(ALICE)?.channel !== channel) {
        send(ws, 0, { stream_key: key, unavailable: true, reason: "unauthorized" }, "STREAM_DELETE");
        break;
      }
      stream.viewers.add(ALICE);
      send(ws, 0, wireStream(key, stream), "STREAM_CREATE");
      send(ws, 0, streamServer(ws, key), "STREAM_SERVER_UPDATE");
      break;
    }
    case 22: {
      const stream = streams.get(d.stream_key);
      if (stream) {
        stream.paused = !!d.paused;
        dispatch("STREAM_UPDATE", wireStream(d.stream_key, stream));
      }
      break;
    }
    case 6: // Resume: not kept, so start over as Discord does.
      send(ws, 9, false);
      break;
    case 8: {
      // Request guild members: a search by name, or these users.
      if (typeof d.query === "string") {
        const query = d.query.toLowerCase();
        const found = Object.values(users)
          .filter((user) => [user.username, user.global_name].some((name) => name.toLowerCase().startsWith(query)))
          .slice(0, d.limit || 100);
        dispatch("GUILD_MEMBERS_CHUNK", {
          guild_id: GUILD,
          members: found.map((user) => member(user)),
          chunk_index: 0,
          chunk_count: 1,
          ...(d.nonce ? { nonce: d.nonce } : {}),
        });
        break;
      }
      const ids: string[] = d.user_ids ?? [];
      const found = ids.map((id) => userById.get(id)).filter((user) => user !== undefined);
      dispatch("GUILD_MEMBERS_CHUNK", {
        guild_id: GUILD,
        members: found.map((user) => member(user)),
        chunk_index: 0,
        chunk_count: 1,
        not_found: ids.filter((id) => !userById.has(id)),
        ...(d.nonce ? { nonce: d.nonce } : {}),
      });
      break;
    }
    case 37: {
      // Guild subscriptions: messages are sent anyway; a channel's member
      // list is sent when asked for. The list subscribed to before stops,
      // which Discord says with INVALIDATE.
      for (const [guildId, subscription] of Object.entries<any>(d.subscriptions ?? {})) {
        // Subscribing with `threads` syncs the server's active threads Alice
        // may see, with her thread members and each one's newest message.
        if (guildId === GUILD && subscription.threads && !ws.data.threadsSynced) {
          ws.data.threadsSynced = true;
          const active = [...threads.values()].filter((thread) => !thread.archived && visibleThread(thread));
          send(ws, 0, {
            guild_id: GUILD,
            threads: active.map((thread) => {
              const { member: _member, ...wire } = wireThread(thread) as Record<string, unknown>;
              return wire;
            }),
            members: active.filter((thread) => thread.members.has(ALICE)).map(wireThreadMember),
            most_recent_messages: active
              .map((thread) => messages.get(thread.id)!.at(-1))
              .filter((message) => message !== undefined)
              .map((message) => ({ ...wireMessage(message, ws.data.origin), guild_id: GUILD })),
          }, "THREAD_LIST_SYNC");
        }
        const channel = Object.keys(subscription.channels ?? {})[0];
        if (guildId !== GUILD || channel === undefined) continue;
        listRequests++;
        const id = listId(channel);
        if (ws.data.listing !== undefined && ws.data.listing !== id) {
          const invalidate = { op: "INVALIDATE", range: [0, 99] };
          send(ws, 0, { guild_id: GUILD, id: ws.data.listing, ops: [invalidate] }, "GUILD_MEMBER_LIST_UPDATE");
        }
        ws.data.listing = id;
        send(ws, 0, memberList(id), "GUILD_MEMBER_LIST_UPDATE");
      }
      break;
    }
    default:
      console.error(`gateway: unhandled op ${payload.op}`);
  }
}

/** Where a stream is served: the voice server, which carries no video. */
function streamServer(ws: Socket, key: string) {
  return { stream_key: key, guild_id: null, endpoint: voiceEndpoint(ws), token: `stream-token-${key}` };
}

function endStream(key: string, reason: string) {
  if (!streams.delete(key)) return;
  const user = streamOf(key).user!;
  const state = voice.get(user);
  if (state) {
    state.self_stream = false;
    announce(user);
  }
  dispatch("STREAM_DELETE", { stream_key: key, unavailable: false, reason });
}

/** Whether the bot made Alice's room. */
let roomMade = false;

/** The bot moves Alice from Create Room to her own room, making it the first
 *  time, which puts her call on another voice server. */
function makeRoom(ws: Socket) {
  if (!roomMade) {
    roomMade = true;
    dispatch("CHANNEL_CREATE", guild().channels.find((made) => made.id === ALICES_ROOM));
  }
  voice.get(ALICE)!.channel = ALICES_ROOM;
  announce(ALICE);
  voiceToken = MOVED_VOICE_TOKEN;
  send(ws, 0, { guild_id: GUILD, token: MOVED_VOICE_TOKEN, endpoint: voiceEndpoint(ws) }, "VOICE_SERVER_UPDATE");
}

/** Tells everyone where `user` is in voice now. */
function announce(user: string) {
  const state = voice.get(user)!;
  const wire = wireVoiceState(user, state);
  dispatch("VOICE_STATE_UPDATE", state.guild ? { ...wire, member: member(userById.get(user)!) } : wire);
}

/** Tells everyone `user` moved in voice from `before`: leaving a server's
 * channel or a DM call, joining one, or changing their state in it. */
function moved(user: string, before: VoiceState | undefined, origin: string) {
  const now = voice.get(user);
  if (before && (before.guild !== now?.guild || before.channel !== now?.channel)) {
    const wire = wireVoiceState(user, undefined, before.guild);
    dispatch("VOICE_STATE_UPDATE", before.guild ? { ...wire, member: member(userById.get(user)!) } : wire);
    if (!before.guild) leftCall(before.channel, origin);
  }
  if (!now) return;
  announce(user);
  if (!now.guild && before?.channel !== now.channel) joinedCall(now.channel, user, origin);
}

// --- DM calls -----------------------------------------------------------------
// Calls in DMs and group DMs, by channel id: whom each rings, and its message.
// Bob answers when Alice rings him in their DM; in a group nobody does, and
// the ringing stops after RING_MS. POST /_control/call makes Bob call Alice
// (with ?group, in Weekend plans), and POST /_control/hang-up makes everyone
// but Alice leave DM calls, back to the Lounge if they were there. Joining one connects with the call's channel as
// the voice server's server id. Alice may ring only her friends in a DM, as
// GET /channels/{id}/call says; Dave isn't one until he accepts her request.

type Call = { ringing: Set<string>; message: string; rang: number };
const calls = new Map<string, Call>();
const ANSWER_MS = 3000;
const RING_MS = 30_000;
const callToken = (channel: string) => `call-token-${channel}`;
/** Where people in the Lounge were before a DM call, to return after it. */
const returnTo = new Map<string, VoiceState>();

/** Puts `user` in a DM call, leaving wherever they were. */
function joinCall(user: string, channel: string, origin: string) {
  const before = voice.get(user);
  if (before?.guild) returnTo.set(user, before);
  voice.set(user, { guild: null, channel, self_mute: false, self_deaf: false, session: `voice-${user}` });
  moved(user, before, origin);
}

const ringable = (dm: Private) => dm.type === 3 || dm.recipients.every((user) => relationships.get(user.id) === 1);

function wireCall(channel: string) {
  const call = calls.get(channel)!;
  return {
    channel_id: channel,
    message_id: call.message,
    region: "fake",
    ringing: [...call.ringing],
    voice_states: [...voice]
      .filter(([, state]) => state.guild === null && state.channel === channel)
      .map(([user, state]) => wireVoiceState(user, state)),
    embedded_activities: [],
  };
}

/** `user` joined a DM call: starting it, with its message, or answering it. */
function joinedCall(channel: string, user: string, origin: string) {
  const call = calls.get(channel);
  if (call) {
    call.ringing.delete(user);
    const message = byId.get(call.message);
    message?.call?.participants.add(user);
    dispatch("CALL_UPDATE", wireCall(channel));
    return;
  }
  const message = post(channel, userById.get(user)!, "", Date.now(), { type: 3, call: { participants: new Set([user]) } });
  calls.set(channel, { ringing: new Set(), message: message.id, rang: 0 });
  dispatch("MESSAGE_CREATE", dispatched(message, origin));
  dispatch("CALL_CREATE", wireCall(channel));
}

/** Someone left a DM call; the last to leave ends it. */
function leftCall(channel: string, origin: string) {
  const call = calls.get(channel);
  if (!call) return;
  const inCall = [...voice.values()].some((state) => state.guild === null && state.channel === channel);
  if (inCall) {
    dispatch("CALL_UPDATE", wireCall(channel));
    return;
  }
  calls.delete(channel);
  dispatch("CALL_DELETE", { channel_id: channel });
  const message = byId.get(call.message);
  if (message?.call) {
    message.call.ended = Date.now();
    dispatch("MESSAGE_UPDATE", dispatched(message, origin));
  }
}

/** Rings these people for a DM call; Bob answers in his DM with Alice. */
function ring(channel: string, people: string[], origin: string) {
  const call = calls.get(channel)!;
  for (const user of people) call.ringing.add(user);
  const rang = ++call.rang;
  dispatch("CALL_UPDATE", wireCall(channel));
  // Only the latest ringing of this call stops.
  setTimeout(() => {
    if (calls.get(channel) !== call || call.rang !== rang || !call.ringing.size) return;
    call.ringing.clear();
    dispatch("CALL_UPDATE", wireCall(channel));
  }, RING_MS);
  if (channel === DM_BOB && people.includes(users.bob.id)) {
    setTimeout(() => {
      if (calls.get(channel)?.ringing.has(users.bob.id)) joinCall(users.bob.id, channel, origin);
    }, ANSWER_MS);
  }
}

// --- Voice server -------------------------------------------------------------
// Discord's voice gateway, version 8, and UDP address discovery. The session
// has no DAVE and its audio is dropped, which is enough for Talk to connect.

const VOICE_TOKEN = "fake-voice-token";
/** The token of the server POST /_control/voice-move moves Alice's call to. */
const MOVED_VOICE_TOKEN = "fake-voice-token-moved";
/** The token each voice connection identified with, in order, for GET
 *  /_control/voice-identifies. */
const voiceIdentifies: string[] = [];
/** Voice connections, open until they close. */
const voiceSockets = new Set<Socket>();
/** The token Alice's voice session holds now: a connection identifying
 *  with the one before is no longer valid, as on Discord. */
let voiceToken = VOICE_TOKEN;
const voiceEndpoint = (ws: Socket) => `${ws.data.origin.replace(/^http/, "ws")}/voice`;
let ssrcs = 100;

/** What a stream connection sent, by its video SSRC. */
type Received = {
  stream: string;
  key: Buffer;
  mode: string;
  /** The video codec the streamer offered to encode, and its payload type. */
  codec: string;
  payloadType: number;
  announced: unknown;
  packets: number;
  undecryptable: number;
  /** Packets dropped as Discord drops them: without the stream id op 12
   * declared, or with a malformed transport sequence number. */
  unattached: number;
  frames: number;
  keyframes: number;
  width: number;
  height: number;
  /** RTCP sender reports, and the packet count the last one gave. */
  senderReports: number;
  reportedPackets: number;
  /** The stream's sound: Opus packets on its audio SSRC, those that didn't
   * open or weren't Opus, sender reports, and op 5's speaking value. */
  audioPackets: number;
  audioUndecryptable: number;
  audioSenderReports: number;
  speaking: number | null;
  /** The streamer's audio SSRC; video and RTX follow it. */
  ssrc: number;
  rtcpCounter: number;
  /** The video packet the fake reported lost, and the resends that came. */
  nacked: number | null;
  retransmitted: number;
  badRetransmissions: number;
};
const received = new Map<number, Received>();

/** What a call connection sent, by its audio SSRC: Opus packets, those that
 *  didn't open, the silent frames among them and those longer than silence,
 *  and op 5's speaking values. */
type CallAudio = {
  token: string; packets: number; undecryptable: number; silent: number; voiced: number; speaking: number[]; resumed: number;
  /** For receiver reports: the highest sequence number, extended, at the last one, and the packets reported lost. */
  highest?: number; reported?: number; lost: number; rtcpCounter: number;
};
/** What /_control/voice-link set: a heartbeat ACK's delay, and the audio lost. */
const voiceLink = { ping: 0, loss: 0 };
const callAudio = new Map<number, CallAudio>();
/** Each call connection's SSRC by its voice session, for resuming. */
const callSessions = new Map<string, number>();
/** Each closed call connection's key, by SSRC, for its resume. */
const resumedKeys = new Map<number, Buffer>();
/** Bob's voice in calls: Opus packets from testing/fixtures/voice-tone.ogg. */
function oggPackets(file: Buffer): Buffer[] {
  const packets: Buffer[] = [];
  let partial: Buffer[] = [];
  for (let at = 0; at + 27 <= file.length && file.toString("ascii", at, at + 4) === "OggS"; ) {
    const segments = file[at + 26]!;
    let body = at + 27 + segments;
    for (const size of file.subarray(at + 27, at + 27 + segments)) {
      partial.push(file.subarray(body, body + size));
      body += size;
      if (size < 255) {
        packets.push(Buffer.concat(partial));
        partial = [];
      }
    }
    at = body;
  }
  // The first two are the Opus header and tags.
  return packets.slice(2);
}
const BOB_AUDIO_SSRC = 6000;
/** The same records, by the stream's audio SSRC. */
const receivedAudio = new Map<number, Received>();
/** And by its RTX SSRC. */
const receivedRtx = new Map<number, Received>();
/** Previews streamers uploaded: their size, and whether they were JPEGs. */
const previews: { stream: string; bytes: number; jpeg: boolean }[] = [];

// --- Watching: Bob's recorded stream --------------------------------------------

type Recorded = { data: Buffer; keyframe: boolean }[];
const fixtures = new URL("./fixtures/", import.meta.url);

/** VP8 frames from an IVF file: a 32-byte header, then sized frames. */
function ivf(file: Buffer): Recorded {
  const frames: Recorded = [];
  for (let at = file.readUInt16LE(6); at + 12 <= file.length; ) {
    const size = file.readUInt32LE(at);
    const data = file.subarray(at + 12, at + 12 + size);
    frames.push({ data, keyframe: (data[0]! & 1) === 0 });
    at += 12 + size;
  }
  return frames;
}

/** H264 access units from an Annex B stream with access unit delimiters, as lists of NAL units. */
function annexB(file: Buffer): { units: Buffer[]; keyframe: boolean }[] {
  const starts: number[] = [];
  for (let at = 0; at + 3 <= file.length; at++) {
    if (file[at] === 0 && file[at + 1] === 0 && file[at + 2] === 1) starts.push(at + 3);
  }
  const units = starts.map((start, index) => {
    let end = index + 1 < starts.length ? starts[index + 1]! - 3 : file.length;
    if (file[end - 1] === 0) end--;
    return file.subarray(start, end);
  });
  const frames: { units: Buffer[]; keyframe: boolean }[] = [];
  for (const unit of units) {
    const type = unit[0]! & 0x1f;
    if (type === 9 || frames.length === 0) frames.push({ units: [], keyframe: false });
    if (type === 9) continue;
    frames.at(-1)!.units.push(unit);
    if (type === 5) frames.at(-1)!.keyframe = true;
  }
  return frames.filter((frame) => frame.units.length > 0);
}

const voiceTone = oggPackets(readFileSync(new URL("voice-tone.ogg", fixtures)));

/** Bob says the tone in Alice's call on `ws`, 20 ms a packet, then five
 *  silent frames, marking himself speaking with op 5 first. */
function bobSpeaks(ws: Socket) {
  const key = ws.data.callKey;
  const peer = ws.data.ssrc ? peers.get(ws.data.ssrc) : undefined;
  if (!key || !peer) return false;
  ws.send(JSON.stringify({ op: 5, d: { user_id: users.bob.id, ssrc: BOB_AUDIO_SSRC, speaking: 1 } }));
  const frames = [...voiceTone, ...Array(5).fill(OPUS_SILENCE)];
  let index = 0;
  let counter = 1;
  const sequence = 20_000;
  const timer = setInterval(() => {
    if (index >= frames.length || ws.readyState !== 1) {
      clearInterval(timer);
      return;
    }
    const header = Buffer.alloc(12);
    header[0] = 0x80;
    header[1] = 120 | (index === 0 ? 0x80 : 0);
    header.writeUInt16BE((sequence + index) & 0xffff, 2);
    header.writeUInt32BE(index * 960, 4);
    header.writeUInt32BE(BOB_AUDIO_SSRC, 8);
    // As Discord's SFU does, op 15 stops any stream it wants at 0, sound
    // as well as video.
    const wants: any = callWants.get(ws.data.ssrc!) ?? {};
    if (Number(wants[String(BOB_AUDIO_SSRC)] ?? wants.any ?? 100) > 0) {
      udp.send(sealRtp(Buffer.concat([header, frames[index]!]), key, counter++, 12), peer.port, peer.address);
    }
    index++;
  }, 20);
  return true;
}

/** Who can film in calls, by their name in POST /_control/camera: their
 *  audio SSRC, which their video's and RTX's follow. */
const filmers: Record<string, { user: string; ssrc: number }> = {
  bob: { user: users.bob.id, ssrc: BOB_AUDIO_SSRC },
  carol: { user: users.carol.id, ssrc: BOB_AUDIO_SSRC + 10 },
  dave: { user: users.dave.id, ssrc: BOB_AUDIO_SSRC + 20 },
};
/** The video codec each call connection offered to encode, and its payload
 *  type, by its SSRC; a resumed session keeps it. */
const callCodecs = new Map<number, { name: string; payloadType: number }>();
/** What each call connection last asked for with op 15, by its SSRC. */
const callWants = new Map<number, unknown>();
/** Alice's camera in calls, by its video SSRC: what came, as for a stream. */
const callCameras = new Map<number, Received & { stops: number }>();
/** Each camera sent to a call, by the call's SSRC and the camera's video SSRC. */
const cameraWatchers = new Map<string, Watcher & { sending: boolean }>();
/** What calls watching cameras went through; see `repairs`. */
const cameraRepairs = { dropped: 0, reordered: 0, nacks: 0, resent: 0, receiverReports: 0 };
/** A camera recording other than testing/fixtures', for measuring: an H264
 *  Annex B file with access unit delimiters, sent at 30 frames a second. */
const cameraRecording = process.env.FAKE_DISCORD_CAMERA ? annexB(readFileSync(process.env.FAKE_DISCORD_CAMERA)) : undefined;

/** The channel a call connection is for. */
function callChannel(ws: Socket): string | undefined {
  const token = ws.data.voiceToken ?? "";
  return token.startsWith("call-token-") ? token.slice("call-token-".length) : voice.get(ALICE)?.channel;
}

/** Who films in the channel of the call on `ws`. */
function filmingFor(ws: Socket): string[] {
  return Object.keys(filmers).filter((name) => {
    const state = voice.get(filmers[name]!.user);
    return !!state?.self_video && state.channel === callChannel(ws);
  });
}

/** Tells the call on `ws` about `name`'s camera with op 12, as Discord tells
 *  each client of another's, or quietly notes it on a resumed session. */
function offerCamera(ws: Socket, name: string, announce: boolean) {
  (ws.data.cameras ??= new Set()).add(name);
  if (!announce) return;
  const { user, ssrc } = filmers[name]!;
  ws.send(JSON.stringify({
    op: 12,
    d: {
      user_id: user, audio_ssrc: ssrc, video_ssrc: ssrc + 1, rtx_ssrc: ssrc + 2,
      streams: [{ type: "video", rid: "100", ssrc: ssrc + 1, rtx_ssrc: ssrc + 2, active: true, quality: 100 }],
    },
  }));
}

/** `name`'s camera went off: the call on `ws` hears so, and stops getting it. */
function withdrawCamera(ws: Socket, name: string) {
  const { user, ssrc } = filmers[name]!;
  ws.data.cameras?.delete(name);
  stopCamera(ws, ssrc + 1);
  ws.send(JSON.stringify({ op: 12, d: { user_id: user, audio_ssrc: ssrc, video_ssrc: 0, rtx_ssrc: 0, streams: [] } }));
}

function stopCamera(ws: Socket, video: number) {
  const watcher = cameraWatchers.get(`${ws.data.ssrc}:${video}`);
  if (watcher?.sending) {
    clearInterval(watcher.timer);
    watcher.sending = false;
  }
}

/** Sends each camera the call on `ws` asks for with op 15, at any quality,
 *  and stops those it asks for at none. */
function sendCameras(ws: Socket, wants: any) {
  for (const name of ws.data.cameras ?? []) {
    const video = filmers[name]!.ssrc + 1;
    const quality = Number(wants[String(video)] ?? wants.any ?? 0);
    const id = `${ws.data.ssrc}:${video}`;
    let watcher = cameraWatchers.get(id);
    if (quality > 0 && !watcher?.sending) {
      const codec = (callCodecs.get(ws.data.ssrc!)?.name ?? "VP8").toLowerCase();
      watcher ??= { stream: "call", codec, framesSent: 0, soundsSent: 0, keyframeRequests: 0, repairs: cameraRepairs, sending: false };
      cameraWatchers.set(id, watcher);
      watcher.sending = true;
      viewerKeys.set(ws.data.ssrc!, ws.data.callKey!);
      streamTo(ws, ws.data.callKey!, video, watcher, cameraRecording);
    } else if (quality === 0) {
      stopCamera(ws, video);
    }
  }
}

/** Alice announced her camera in a call with op 12, or its end with a zero
 *  video SSRC: what follows on that SSRC is counted as a stream's video is. */
function receiveCallCamera(ws: Socket, d: any) {
  const ssrc = ws.data.ssrc!;
  const video = Number(d.video_ssrc ?? 0);
  if (video === 0) {
    for (const camera of callCameras.values()) if (camera.ssrc === ssrc) camera.stops++;
    return;
  }
  if (callCameras.has(video)) return;
  const codec = callCodecs.get(ssrc) ?? { name: "VP8", payloadType: 105 };
  const record = {
    stream: `call:${ws.data.voiceToken}`, key: ws.data.callKey!, mode: "aead_aes256_gcm_rtpsize",
    announced: d.streams?.[0]?.max_resolution ?? null,
    codec: codec.name, payloadType: codec.payloadType,
    packets: 0, undecryptable: 0, unattached: 0, frames: 0, keyframes: 0, width: 0, height: 0,
    senderReports: 0, reportedPackets: 0,
    audioPackets: 0, audioUndecryptable: 0, audioSenderReports: 0, speaking: null,
    ssrc, rtcpCounter: 1, nacked: null, retransmitted: 0, badRetransmissions: 0, stops: 0,
  };
  callCameras.set(video, record);
  receivedRtx.set(Number(d.rtx_ssrc ?? video + 1), record);
}

const recordings = {
  vp8: ivf(readFileSync(new URL("stream-vp8.ivf", fixtures))),
  h264: annexB(readFileSync(new URL("stream-h264.h264", fixtures))),
};
let streamCodec: "vp8" | "h264" = "vp8";

/** A viewer of Bob's stream and what it was sent. */
type Watcher = {
  stream: string; codec: string; framesSent: number; soundsSent: number; keyframeRequests: number; timer?: Timer;
  /** What its connection went through; see `repairs`. */
  repairs: typeof repairs;
  /** Sends Bob's packet with this sequence number again, as RTX. */
  resend?: (sequence: number) => void;
};
const watchers = new Map<number, Watcher>();
/** Each viewer's transport key, by its SSRC, to read its RTCP. */
const viewerKeys = new Map<number, Buffer>();
/** What viewers' connections went through: a packet lost on the way and
 * one overtaken per stream watched, and what the viewers did about it. */
const repairs = { dropped: 0, reordered: 0, nacks: 0, resent: 0, receiverReports: 0 };
/** Where each voice connection's UDP comes from, by its SSRC. */
const peers = new Map<number, { address: string; port: number }>();
const PLAYOUT_DELAY = Buffer.from([0xbe, 0xde, 0x00, 0x01, 0x52, 0x00, 0x00, 0x00]);
/** 20 ms of Opus silence. */
const OPUS_SILENCE = Buffer.from([0xf8, 0xff, 0xfe]);

/** Seals an RTP packet as "rtpsize" AES-GCM: header and extension preamble in the clear. */
function sealRtp(packet: Buffer, key: Buffer, counter: number, clear = 16): Buffer {
  const nonce = Buffer.alloc(12);
  nonce.writeUInt32BE(counter >>> 0, 0);
  const cipher = createCipheriv("aes-256-gcm", key, nonce);
  cipher.setAAD(packet.subarray(0, clear));
  const sealed = Buffer.concat([cipher.update(packet.subarray(clear)), cipher.final()]);
  return Buffer.concat([packet.subarray(0, clear), sealed, cipher.getAuthTag(), nonce.subarray(0, 4)]);
}

/** Streams the recording to a viewer at 10 frames a second until it
 *  disconnects, or a camera's to a call: `camera`'s frames at 30 a second. */
function streamTo(ws: Socket, key: Buffer, ssrc: number, watcher = watchers.get(ws.data.ssrc!)!, camera?: { units: Buffer[] }[]) {
  let frame = 0;
  let sequence = 1000;
  let soundSequence = 7000;
  let rtxSequence = 3000;
  let counter = 1;
  let picture = 0;
  const packet = (payload: Buffer, marker: boolean, timestamp: number, payloadType: number, from: number, number: number) => {
    const header = Buffer.alloc(12);
    header[0] = 0x90;
    header[1] = (marker ? 0x80 : 0) | payloadType;
    header.writeUInt16BE(number & 0xffff, 2);
    header.writeUInt32BE(timestamp >>> 0, 4);
    header.writeUInt32BE(from, 8);
    return sealRtp(Buffer.concat([header, PLAYOUT_DELAY, payload]), key, counter++);
  };
  // What was sent lately, to resend when the viewer asks.
  const history = new Map<number, { payload: Buffer; marker: boolean; timestamp: number; payloadType: number }>();
  let overtaken: Buffer | undefined;
  const send = (payload: Buffer, marker: boolean, timestamp: number, payloadType: number) => {
    const number = sequence++ & 0xffff;
    history.set(number, { payload, marker, timestamp, payloadType });
    history.delete((number - 1024) & 0xffff);
    const sealed = packet(payload, marker, timestamp, payloadType, ssrc, number);
    // As on the internet: one packet lost on the way, and one overtaken.
    if (number === 1003) return void watcher.repairs.dropped++;
    if (number === 1008) return void (overtaken = sealed);
    toViewer(sealed);
    if (overtaken) {
      toViewer(overtaken);
      overtaken = undefined;
      watcher.repairs.reordered++;
    }
  };
  // RFC 4588: on the RTX SSRC, the original sequence number first.
  watcher.resend = (number) => {
    const sent = history.get(number);
    if (!sent) return;
    const original = Buffer.alloc(2);
    original.writeUInt16BE(number);
    const rtx = sent.payloadType === 101 ? 102 : 106;
    toViewer(packet(Buffer.concat([original, sent.payload]), sent.marker, sent.timestamp, rtx, ssrc + 1, rtxSequence++));
    watcher.repairs.resent++;
  };
  // A viewer that went away refuses packets until its connection closes.
  const toViewer = (packet: Buffer) => {
    const peer = peers.get(ws.data.ssrc!);
    try {
      if (peer) udp.send(packet, peer.port, peer.address);
    } catch {}
  };
  watcher.timer = setInterval(() => {
    const timestamp = frame * (camera ? 3000 : 9000);
    if (watcher.codec === "vp8") {
      const { data } = recordings.vp8[frame % recordings.vp8.length]!;
      for (let at = 0; at < data.length; at += 1100) {
        const descriptor = Buffer.from([at === 0 ? 0x90 : 0x80, 0x80, 0x80 | (picture >> 8), picture & 0xff]);
        send(Buffer.concat([descriptor, data.subarray(at, at + 1100)]), at + 1100 >= data.length, timestamp, 105);
      }
      picture = (picture + 1) & 0x7fff;
    } else {
      const h264 = camera ?? recordings.h264;
      const { units } = h264[frame % h264.length]!;
      units.forEach((unit, index) => {
        const last = index === units.length - 1;
        if (unit.length <= 1100) return send(unit, last, timestamp, 101);
        for (let at = 1; at < unit.length; at += 1100) {
          const start = at === 1 ? 0x80 : 0;
          const end = at + 1100 >= unit.length ? 0x40 : 0;
          const fragment = Buffer.from([(unit[0]! & 0xe0) | 28, start | end | (unit[0]! & 0x1f)]);
          send(Buffer.concat([fragment, unit.subarray(at, at + 1100)]), last && end !== 0, timestamp, 101);
        }
      });
    }
    watcher.framesSent++;
    // 100 ms of sound from Bob's audio SSRC, just before his video's: five
    // Opus frames of silence, plain RTP as a call's audio goes. A camera
    // has no sound of its own.
    for (let index = 0; index < (watcher.stream === "call" ? 0 : 5); index++) {
      const header = Buffer.alloc(12);
      header[0] = 0x80;
      header[1] = 120;
      header.writeUInt16BE(soundSequence++ & 0xffff, 2);
      header.writeUInt32BE((frame * 5 + index) * 960, 4);
      header.writeUInt32BE(ssrc - 1, 8);
      toViewer(sealRtp(Buffer.concat([header, OPUS_SILENCE]), key, counter++, 12));
      watcher.soundsSent++;
    }
    frame++;
  }, camera ? 1000 / 30 : 100);
}

/** Opens an "rtpsize" AES-GCM packet: the header is authenticated, the rest encrypted. */
function openRtp(packet: Buffer, key: Buffer, rtcp = false): Buffer | undefined {
  // RTCP keeps its header and sender SSRC in the clear.
  const clear = rtcp ? 8 : 12 + (packet[0]! & 0x0f) * 4 + (packet[0]! & 0x10 ? 4 : 0);
  if (packet.length < clear + 20) return undefined;
  const nonce = Buffer.alloc(12);
  packet.copy(nonce, 0, packet.length - 4);
  const decipher = createDecipheriv("aes-256-gcm", key, nonce);
  decipher.setAAD(packet.subarray(0, clear));
  decipher.setAuthTag(packet.subarray(packet.length - 20, packet.length - 4));
  try {
    return Buffer.concat([packet.subarray(0, clear), decipher.update(packet.subarray(clear, packet.length - 20)), decipher.final()]);
  } catch {
    return undefined;
  }
}

/** A decrypted RTP packet's one-byte extensions by id, and where its payload starts. */
function rtpExtensions(plain: Buffer): { extensions: Map<number, Buffer>; at: number } {
  let at = 12 + (plain[0]! & 0x0f) * 4;
  const extensions = new Map<number, Buffer>();
  if (plain[0]! & 0x10) {
    const end = at + 4 + plain.readUInt16BE(at + 2) * 4;
    // One-byte elements: id and length minus one, then the value; zeros pad.
    for (let element = at + 4; element < end; ) {
      const id = plain[element]! >> 4;
      if (id === 0) {
        element++;
        continue;
      }
      const length = (plain[element]! & 0x0f) + 1;
      extensions.set(id, plain.subarray(element + 1, element + 1 + length));
      element += 1 + length;
    }
    at = end;
  }
  return { extensions, at };
}

/** Sends the streamer a generic NACK (RTCP 205, format 1) for one video packet. */
function nack(stream: Received, sequence: number) {
  const peer = peers.get(stream.ssrc);
  if (!peer) return;
  stream.nacked = sequence;
  const packet = Buffer.alloc(16);
  packet[0] = 0x81;
  packet[1] = 205;
  packet.writeUInt16BE(3, 2);
  packet.writeUInt32BE(1, 4);
  packet.writeUInt32BE(stream.ssrc + 1, 8);
  packet.writeUInt16BE(sequence, 12);
  udp.send(sealRtp(packet, stream.key, stream.rtcpCounter++, 8), peer.port, peer.address);
}

/** Checks a resent packet: the RTX payload type, the NACKed sequence number
 * before the payload, and the repaired stream id in place of the stream id. */
function receiveRtx(stream: Received, packet: Buffer) {
  const plain = openRtp(packet, stream.key);
  const { extensions, at } = plain ? rtpExtensions(plain) : { extensions: new Map<number, Buffer>(), at: 0 };
  const good =
    plain &&
    (plain[1]! & 0x7f) === stream.payloadType + 1 &&
    extensions.get(12)?.toString() === "100" &&
    !extensions.has(11) &&
    plain.readUInt16BE(at) === stream.nacked;
  if (good) stream.retransmitted++;
  else stream.badRetransmissions++;
}

/** Counts a packet of a stream's sound: Opus, payload type 120. */
function receiveAudio(stream: Received, packet: Buffer) {
  stream.audioPackets++;
  const plain = stream.mode === "aead_aes256_gcm_rtpsize" ? openRtp(packet, stream.key) : undefined;
  if (!plain || (plain[1]! & 0x7f) !== 120) stream.audioUndecryptable++;
}

/** Counts a call's Opus packet, and its silent frames. */
function receiveCallAudio(call: CallAudio, ssrc: number, packet: Buffer) {
  call.packets++;
  const key = [...voiceSockets].find((ws) => ws.data.ssrc === ssrc)?.data.callKey;
  const plain = key ? openRtp(packet, key) : undefined;
  if (!plain || (plain[1]! & 0x7f) !== 120) {
    call.undecryptable++;
    return;
  }
  const opus = plain.subarray(rtpExtensions(plain).at);
  if (opus.equals(OPUS_SILENCE)) call.silent++;
  else if (opus.length > 3) call.voiced++;
  reportCallAudio(call, ssrc, key!, plain.readUInt16BE(2));
}

/** Every 50 packets, a receiver report (RTCP 201) about the call's audio
 * from the server, encrypted as its other RTCP: the packets lost so far,
 * with /_control/voice-link's share of those since the last, and the highest
 * sequence number received, extended. */
function reportCallAudio(call: CallAudio, ssrc: number, key: Buffer, sequence: number) {
  const last = call.highest ?? sequence;
  const highest = last + ((sequence - (last & 0xffff) + 0x10000) % 0x10000);
  call.highest = highest;
  call.reported ??= highest;
  const expected = highest - call.reported;
  if (expected < 50) return;
  const lost = Math.round((expected * voiceLink.loss) / 100);
  call.lost += lost;
  call.reported = highest;
  const peer = peers.get(ssrc);
  if (!peer) return;
  const packet = Buffer.alloc(32);
  packet[0] = 0x81;
  packet[1] = 201;
  packet.writeUInt16BE(7, 2);
  packet.writeUInt32BE(1, 4);
  packet.writeUInt32BE(ssrc, 8);
  packet.writeUInt32BE(((Math.min(255, Math.round((lost * 256) / expected)) << 24) | call.lost) >>> 0, 12);
  packet.writeUInt32BE(highest >>> 0, 16);
  udp.send(sealRtp(packet, key, call.rtcpCounter++, 8), peer.port, peer.address);
}

/** Counts a video packet: frames end with the marker bit; keyframes carry their size. */
function receiveVideo(packet: Buffer) {
  const ssrc = packet.readUInt32BE(8);
  const stream = received.get(ssrc) ?? callCameras.get(ssrc);
  if (!stream) return;
  stream.packets++;
  const plain = stream.mode === "aead_aes256_gcm_rtpsize" ? openRtp(packet, stream.key) : undefined;
  if (!plain || (plain[1]! & 0x7f) !== stream.payloadType) {
    stream.undecryptable++;
    return;
  }
  const { extensions, at } = rtpExtensions(plain);
  const transportSequence = extensions.get(5);
  if (extensions.get(11)?.toString() !== "100" || (transportSequence && transportSequence.length !== 2)) {
    stream.unattached++;
    return;
  }
  // Report the first packet lost, as a viewer would, to see it resent.
  if (stream.nacked === null) nack(stream, plain.readUInt16BE(2));
  const payload = plain.subarray(at);
  const size = stream.codec === "H264" ? h264Keyframe(payload) : vp8Keyframe(payload);
  if (size) {
    stream.keyframes++;
    [stream.width, stream.height] = size;
  }
  if (plain[1]! & 0x80) stream.frames++;
}

/** The size a VP8 keyframe's first packet gives. */
function vp8Keyframe(payload: Buffer): [number, number] | undefined {
  let at = 0;
  const descriptor = payload[at++]!;
  if (descriptor & 0x80) {
    const extended = payload[at++]!;
    if (extended & 0x80) at += payload[at]! & 0x80 ? 2 : 1;
    if (extended & 0x40) at++;
    if (extended & 0x30) at++;
  }
  const frame = payload.subarray(at);
  const start = descriptor & 0x10 && (descriptor & 0x07) === 0;
  if (start && (frame[0]! & 1) === 0 && frame[3] === 0x9d && frame[4] === 0x01 && frame[5] === 0x2a)
    return [frame.readUInt16LE(6) & 0x3fff, frame.readUInt16LE(8) & 0x3fff];
}

/** The size an H264 packet's sequence parameter set gives: whole, or first in a STAP-A. */
function h264Keyframe(payload: Buffer): [number, number] | undefined {
  const type = payload[0]! & 0x1f;
  if (type === 7) return spsSize(payload);
  if (type === 24 && (payload[3]! & 0x1f) === 7) return spsSize(payload.subarray(3, 3 + payload.readUInt16BE(1)));
}

/** Reads the picture size from an SPS (H.264 7.3.2.1.1), as baseline encoders write it. */
function spsSize(nal: Buffer): [number, number] | undefined {
  // Undo emulation prevention: 0 0 3 stands for 0 0.
  const bytes: number[] = [];
  for (let i = 1; i < nal.length; i++) {
    if (i >= 3 && nal[i] === 3 && nal[i - 1] === 0 && nal[i - 2] === 0) continue;
    bytes.push(nal[i]!);
  }
  let bit = 0;
  const read = (count: number) => {
    let value = 0;
    for (let i = 0; i < count; i++, bit++) value = (value << 1) | ((bytes[bit >> 3]! >> (7 - (bit & 7))) & 1);
    return value;
  };
  const golomb = () => {
    let zeros = 0;
    while (read(1) === 0 && zeros < 32) zeros++;
    return (1 << zeros) - 1 + read(zeros);
  };
  const profile = read(8);
  read(16); // constraint flags and level
  golomb(); // seq_parameter_set_id
  if ([100, 110, 122, 244, 44, 83, 86, 118, 128].includes(profile)) return undefined;
  golomb(); // log2_max_frame_num_minus4
  const order = golomb();
  if (order === 0) golomb();
  else if (order === 1) return undefined;
  golomb(); // max_num_ref_frames
  read(1);
  const width = golomb() + 1;
  const height = golomb() + 1;
  const frameOnly = read(1);
  if (!frameOnly) read(1);
  read(1); // direct_8x8_inference_flag
  const crop = read(1) ? [golomb(), golomb(), golomb(), golomb()] : [0, 0, 0, 0];
  return [width * 16 - 2 * (crop[0]! + crop[1]!), (2 - frameOnly) * height * 16 - 2 * (crop[2]! + crop[3]!)];
}

const udp = await Bun.udpSocket({
  hostname: args.host,
  socket: {
    data(socket, data, port, address) {
      // IP discovery: type 1, length 70, SSRC, then the answer fills in the address.
      if (data.length !== 74 || data.readUInt16BE(0) !== 1) {
        if (data.length > 12 && (data[0]! & 0xc0) === 0x80 && !(data[1]! >= 192 && data[1]! <= 223)) {
          const ssrc = data.readUInt32BE(8);
          const sound = receivedAudio.get(ssrc);
          const resent = receivedRtx.get(ssrc);
          const call = callAudio.get(ssrc);
          if (call) receiveCallAudio(call, ssrc, data);
          else if (sound) receiveAudio(sound, data);
          else if (resent) receiveRtx(resent, data);
          else receiveVideo(data);
        }
        // A streamer's RTCP sender report, from its video or audio SSRC.
        if (data.length >= 8 && data[1] === 200) {
          const sender = data.readUInt32BE(4);
          const stream = received.get(sender) ?? callCameras.get(sender) ?? receivedAudio.get(sender);
          const report = stream && openRtp(data, stream.key, true);
          if (stream && report && report.length >= 28) {
            if (received.has(sender) || callCameras.has(sender)) {
              stream.senderReports++;
              stream.reportedPackets = report.readUInt32BE(20);
            } else {
              stream.audioSenderReports++;
            }
          }
        }
        // RTCP feedback with a picture loss indication, from a viewer's SSRC.
        // A call's names the camera after the sender, encrypted.
        const watcherOf = () => {
          const sender = data.readUInt32BE(4);
          const key = viewerKeys.get(sender);
          const plain = !watchers.has(sender) && key ? openRtp(data, key, true) : undefined;
          return watchers.get(sender) ?? (plain && plain.length >= 12 ? cameraWatchers.get(`${sender}:${plain.readUInt32BE(8)}`) : undefined);
        };
        if (data.length >= 8 && data[1] === 206 && (data[0]! & 0x1f) === 1) {
          const watcher = watcherOf();
          if (watcher) watcher.keyframeRequests++;
        }
        // A viewer's generic NACK: the packets it asks for again.
        const asking = data.length >= 8 && data[1] === 205 && (data[0]! & 0x1f) === 1 ? watcherOf() : undefined;
        const viewer = asking && viewerKeys.get(data.readUInt32BE(4));
        const nack = asking && viewer && openRtp(data, viewer, true);
        if (asking && nack) {
          asking.repairs.nacks++;
          for (let at = 12; at + 4 <= nack.length; at += 4) {
            const first = nack.readUInt16BE(at);
            const mask = nack.readUInt16BE(at + 2);
            asking.resend?.(first);
            for (let bit = 0; bit < 16; bit++) if (mask & (1 << bit)) asking.resend?.((first + bit + 1) & 0xffff);
          }
        }
        const reporting = data.length >= 8 && data[1] === 201 ? watchers.get(data.readUInt32BE(4)) : undefined;
        if (reporting) reporting.repairs.receiverReports++;
        return;
      }
      peers.set(data.readUInt32BE(4), { address, port });
      const answer = Buffer.alloc(74);
      answer.writeUInt16BE(2, 0);
      answer.writeUInt16BE(70, 2);
      answer.writeUInt32BE(data.readUInt32BE(4), 4);
      answer.write(address, 8, "ascii");
      answer.writeUInt16BE(port, 72);
      socket.send(answer, port, address);
    },
  },
});

function voiceMessage(ws: Socket, text: string) {
  let payload: { op?: number; d?: any };
  try {
    payload = JSON.parse(text);
  } catch {
    return;
  }
  const d = payload.d ?? {};
  const reply = (op: number, data: unknown) => ws.send(JSON.stringify({ op, d: data }));
  switch (payload.op) {
    case 0: // Identify
      if (d.token !== VOICE_TOKEN && d.token !== MOVED_VOICE_TOKEN && !String(d.token).startsWith("stream-token-") && !String(d.token).startsWith("call-token-")) {
        ws.close(4004, "Authentication failed.");
        return;
      }
      // A DM call identifies with its channel for the server.
      if (String(d.token).startsWith("call-token-") && d.server_id !== String(d.token).slice("call-token-".length)) {
        ws.close(4006, "Session is no longer valid.");
        return;
      }
      if (d.token === VOICE_TOKEN && voiceToken !== VOICE_TOKEN) {
        ws.close(4006, "Session is no longer valid.");
        return;
      }
      ws.data.ssrc = ssrcs += 3;
      ws.data.voiceToken = String(d.token);
      voiceIdentifies.push(String(d.token));
      if (d.token === MOVED_VOICE_TOKEN) {
        for (const old of voiceSockets) if (old.data.voiceToken === VOICE_TOKEN) old.close(4014, "Disconnected.");
      }
      if (String(d.token).startsWith("stream-token-")) ws.data.stream = String(d.token).slice("stream-token-".length);
      else callSessions.set(String(d.session_id), ws.data.ssrc);
      reply(2, {
        ssrc: ws.data.ssrc,
        ip: args.host,
        port: udp.port,
        modes: ["aead_aes256_gcm_rtpsize", "aead_xchacha20_poly1305_rtpsize"],
        ...(d.video
          ? { streams: [{ type: "video", rid: "100", ssrc: ws.data.ssrc + 1, rtx_ssrc: ws.data.ssrc + 2, active: false, quality: 100 }] }
          : {}),
      });
      break;
    case 1: {
      // Select protocol
      const key = Buffer.from(crypto.getRandomValues(new Uint8Array(32)));
      // A streamer gets the video codec it offered to encode, by priority.
      const offered = ((d.codecs ?? []) as { name: string; type: string; priority: number; payload_type: number; encode?: boolean }[])
        .filter((codec) => codec.type === "video" && codec.encode !== false)
        .sort((a, b) => a.priority - b.priority)[0];
      if (ws.data.stream && ws.data.ssrc) {
        const record: Received = {
          stream: ws.data.stream, key, mode: d.data?.mode, announced: null,
          codec: offered?.name ?? "VP8", payloadType: offered?.payload_type ?? 105,
          packets: 0, undecryptable: 0, unattached: 0, frames: 0, keyframes: 0, width: 0, height: 0,
          senderReports: 0, reportedPackets: 0,
          audioPackets: 0, audioUndecryptable: 0, audioSenderReports: 0, speaking: null,
          ssrc: ws.data.ssrc, rtcpCounter: 1, nacked: null, retransmitted: 0, badRetransmissions: 0,
        };
        received.set(ws.data.ssrc + 1, record);
        receivedAudio.set(ws.data.ssrc, record);
        receivedRtx.set(ws.data.ssrc + 2, record);
      }
      if (!ws.data.stream && ws.data.ssrc) {
        ws.data.callKey = key;
        callCodecs.set(ws.data.ssrc, { name: offered?.name ?? "VP8", payloadType: offered?.payload_type ?? 105 });
        callAudio.set(ws.data.ssrc, { token: ws.data.voiceToken ?? "", packets: 0, undecryptable: 0, silent: 0, voiced: 0, speaking: [], resumed: 0, lost: 0, rtcpCounter: 1 << 20 });
      }
      const watching = ws.data.stream && !ws.data.stream.endsWith(`:${ALICE}`);
      const codec = watching ? streamCodec : (offered?.name ?? "VP8").toLowerCase();
      reply(4, { mode: d.data?.mode, secret_key: [...key], dave_protocol_version: 0, audio_codec: "opus", video_codec: codec.toUpperCase() });
      if (watching && ws.data.ssrc) {
        // Bob's SSRCs, as Discord announces another client's video.
        const bob = 5000;
        reply(12, {
          user_id: users.bob.id, audio_ssrc: bob, video_ssrc: bob + 1, rtx_ssrc: bob + 2,
          streams: [{ type: "video", rid: "100", ssrc: bob + 1, rtx_ssrc: bob + 2, active: true, quality: 100 }],
        });
        watchers.set(ws.data.ssrc, { stream: ws.data.stream!, codec, framesSent: 0, soundsSent: 0, keyframeRequests: 0, repairs });
        viewerKeys.set(ws.data.ssrc, key);
        ws.data.waiting = { key, ssrc: bob + 1 };
      }
      if (ws.data.callKey) for (const name of filmingFor(ws)) offerCamera(ws, name, true);
      break;
    }
    case 15: { // Media sink wants: Discord forwards video only to those who ask.
      const waiting = ws.data.waiting;
      if (waiting && Number(d.any) > 0) {
        ws.data.waiting = undefined;
        streamTo(ws, waiting.key, waiting.ssrc);
      }
      if (ws.data.callKey && ws.data.ssrc) {
        callWants.set(ws.data.ssrc, d);
        sendCameras(ws, d);
      }
      break;
    }
    case 5: { // Speaking: a stream's sound is marked as soundshare.
      const stream = ws.data.ssrc ? receivedAudio.get(ws.data.ssrc) : undefined;
      if (stream) stream.speaking = Number(d.speaking);
      const call = ws.data.ssrc ? callAudio.get(ws.data.ssrc) : undefined;
      if (call) call.speaking.push(Number(d.speaking));
      break;
    }
    case 7: { // Resume: the session carries on, with its SSRC and key.
      const ssrc = callSessions.get(String(d.session_id));
      const old = ssrc ? [...voiceSockets].find((other) => other !== ws && other.data.ssrc === ssrc) : undefined;
      const call = ssrc ? callAudio.get(ssrc) : undefined;
      if (!ssrc || !call || String(d.token) !== call.token) {
        ws.close(4006, "Session is no longer valid.");
        return;
      }
      ws.data.ssrc = ssrc;
      ws.data.voiceToken = call.token;
      ws.data.callKey = old?.data.callKey ?? [...voiceSockets].find((other) => other.data.callKey && other.data.ssrc === ssrc)?.data.callKey ?? resumedKeys.get(ssrc);
      call.resumed++;
      reply(9, null);
      // The session still has the cameras on, sent again once asked for.
      for (const name of filmingFor(ws)) offerCamera(ws, name, false);
      break;
    }
    case 12: { // Video attributes
      const stream = received.get(d.video_ssrc);
      if (stream) stream.announced = d.streams?.[0]?.max_resolution ?? null;
      if (ws.data.callKey && ws.data.ssrc) receiveCallCamera(ws, d);
      break;
    }
    case 3: // Heartbeat
      if (voiceLink.ping) setTimeout(() => reply(6, d), voiceLink.ping);
      else reply(6, d);
      break;
    default: // Speaking and client details need no answer.
      break;
  }
}

// --- HTTP -------------------------------------------------------------------

const json = (body: unknown, status = 200) => Response.json(body, { status });
const empty = () => new Response(null, { status: 204 });
const notFound = (message = "404: Not Found", code = 0) => json({ message, code }, 404);

/** The invites Alice made, oldest first, for GET /_control/invites. */
const invites: Record<string, unknown>[] = [];

/** Files uploaded for a message that isn't sent yet, by uploaded filename. */
const uploads = new Map<string, { filename: string; data?: Uint8Array }>();
let uploadCount = 0;

// GIFs for the picker, as Discord's /gifs endpoints give them from its
// provider: testing/fixtures/gifs' pictures, each found by the terms it has.
// Searching "nothing" finds none. The provider's pages are Klipy's; the
// pictures come from the fake, as /_gifs/NAME.webp.
const gifCatalog: { name: string; terms: string[]; width: number; height: number }[] = [
  { name: "cat", terms: ["cat", "happy", "cute"], width: 220, height: 124 },
  { name: "dog", terms: ["dog", "happy", "cute"], width: 220, height: 220 },
  { name: "dance", terms: ["dance", "party", "happy"], width: 220, height: 293 },
  { name: "wave", terms: ["wave", "hello", "hi"], width: 220, height: 165 },
  { name: "party", terms: ["party", "dance", "celebrate"], width: 220, height: 124 },
  { name: "wow", terms: ["wow", "surprised", "omg"], width: 220, height: 220 },
  { name: "sad", terms: ["sad", "cry"], width: 220, height: 165 },
  { name: "laugh", terms: ["laugh", "lol", "happy"], width: 220, height: 293 },
  { name: "thumbs", terms: ["thumbs up", "yes", "ok"], width: 220, height: 124 },
  { name: "spin", terms: ["spin", "dance", "wow"], width: 220, height: 220 },
];
/** The trending terms the picker's home shows, each as a category. */
const gifCategories = ["happy", "dance", "wow", "sad", "cute", "party"];
/** What POST /gifs/select reported, for GET /_control/gif-selects. */
const gifSelects: { id: string; q: string }[] = [];

function wireGif(gif: (typeof gifCatalog)[number], origin: string) {
  const index = gifCatalog.indexOf(gif);
  return {
    id: String(9100 + index), title: "", url: `https://klipy.com/gifs/${gif.name}-${9100 + index}`,
    src: `${origin}/_gifs/${gif.name}.webp`, gif_src: `${origin}/_gifs/${gif.name}.webp`,
    preview: `${origin}/_gifs/${gif.name}.webp`, width: gif.width, height: gif.height,
  };
}

function gifsFor(query: string) {
  const q = query.trim().toLowerCase();
  if (!q || q === "nothing") return [];
  return gifCatalog.filter((gif) => gif.name.startsWith(q) || gif.terms.some((term) => term.startsWith(q)));
}

// Alice's second settings, FrecencyUserSettings: her favourite GIFs, two
// added on another client: one by its page, whose picture is a .gif, and one
// by a direct link to its file, an animated WebP. PATCH replaces them.
function protoNumber(field: number, value: number): Uint8Array {
  const out = [field << 3];
  for (; ; value >>= 7) {
    if (value < 0x80) { out.push(value); break; }
    out.push(value & 0x7f | 0x80);
  }
  return new Uint8Array(out);
}
function favoriteGifs(origin: string): string {
  const entry = (url: string, src: string, width: number, height: number, order: number) => {
    const gif = new Uint8Array([...protoNumber(1, 1), ...protoBytes(2, text(src)),
      ...protoNumber(3, width), ...protoNumber(4, height), ...protoNumber(5, order)]);
    return protoBytes(1, new Uint8Array([...protoBytes(1, text(url)), ...protoBytes(2, gif)]));
  };
  const gifs = new Uint8Array([
    ...entry("https://klipy.com/gifs/hello-9000", `${origin}/_gifs/hello.gif`, 220, 165, 1),
    ...entry(linkedGif("wave", "webp"), `${origin}/_gifs/wave.webp`, 220, 165, 2),
  ]);
  return Buffer.from(protoBytes(2, gifs)).toString("base64");
}

/** A direct link to a GIF provider's file, as a favourite may be kept by. */
function linkedGif(name: string, extension: string): string {
  return `https://static.klipy.com/ii/${name}.${extension}`;
}

// A direct link to a picture that moves unfurls into an image embed: the
// picture as its thumbnail, through the media proxy, flagged IS_ANIMATED.
function linkedGifEmbed(content: string, origin: string) {
  const m = content.trim().match(/^https:\/\/static\.klipy\.com\/ii\/([a-z]+)\.(webp|gif)$/);
  const gif = m && [...gifCatalog, { name: "hello", width: 220, height: 165 }].find((gif) => gif.name === m[1]);
  if (!m || !gif) return null;
  const link = linkedGif(gif.name, m[2]!);
  return {
    type: "image", url: link,
    thumbnail: {
      url: link, proxy_url: `${origin}/external/klipy/https/static.klipy.com/ii/${gif.name}.${m[2]}`,
      width: gif.width, height: gif.height, content_type: `image/${m[2]}`, flags: 1 << 5,
    },
  };
}
let frecencySettings: string | null = null;

async function api(req: Request, path: string, origin: string): Promise<Response> {
  // A confirmed remote login's ticket, traded for the login without one.
  if (req.method === "POST" && path === "/users/@me/remote-auth/login") {
    const body: any = await req.json().catch(() => ({}));
    const key = tickets.get(String(body.ticket));
    if (!key) return json({ message: "Invalid ticket", code: 50035 }, 400);
    if (ticketCaptcha) {
      const solved = req.headers.get("x-captcha-key") === "solved"
        && req.headers.get("x-captcha-rqtoken") === TICKET_CAPTCHA.captcha_rqtoken
        && req.headers.get("x-captcha-session-id") === TICKET_CAPTCHA.captcha_session_id;
      if (!solved) return json(TICKET_CAPTCHA, 400);
      ticketCaptcha = false;
    }
    tickets.delete(String(body.ticket));
    return json({ encrypted_token: await seal(key, TOKEN) });
  }
  if (req.headers.get("authorization") !== TOKEN) {
    return json({ message: "401: Unauthorized", code: 0 }, 401);
  }
  const url = new URL(req.url);
  const method = req.method;
  let m: RegExpMatchArray | null;

  // A live stream's preview, from its streamer: a JPEG as a data URI.
  if (method === "POST" && (m = path.match(/^\/streams\/([^/]+)\/preview$/))) {
    const stream = decodeURIComponent(m[1]!);
    if (!stream.endsWith(`:${ALICE}`)) return json({ message: "Missing Access", code: 50001 }, 403);
    const body: any = await req.json();
    const prefix = "data:image/jpeg;base64,";
    const thumbnail = String(body.thumbnail ?? "");
    const jpeg = thumbnail.startsWith(prefix) ? Buffer.from(thumbnail.slice(prefix.length), "base64") : Buffer.alloc(0);
    previews.push({ stream, bytes: jpeg.length, jpeg: jpeg[0] === 0xff && jpeg[1] === 0xd8 });
    return new Response(null, { status: 204 });
  }

  if (method === "GET" && (m = path.match(/^\/users\/(\d+|@me)\/profile$/))) {
    const user = userById.get(m[1] === "@me" ? ALICE : m[1]!);
    if (!user) return notFound("Unknown User", 10013);
    return json(wireProfile(user, url.searchParams.get("guild_id")));
  }

  // Opening a DM finds the one Alice has with them, or starts one; with
  // several people, it makes a group.
  if (method === "POST" && path === "/users/@me/channels") {
    const body: any = await req.json();
    if ((body.recipients?.length ?? 0) > 1) {
      const people = (body.recipients as unknown[]).map((id) => userById.get(String(id)));
      if (people.some((user) => !user) || people.length > 9) return json({ message: "Invalid Recipient(s)", code: 50033 }, 400);
      const id = snowflake(Date.now(), 78);
      const channel: Private = { type: 3, recipients: people as User[] };
      privates.set(id, channel);
      messages.set(id, []);
      dispatch("CHANNEL_CREATE", wirePrivate(id, channel, "objects"));
      return json(wirePrivate(id, channel, "objects"));
    }
    const user = userById.get(String(body.recipients?.[0]));
    if (!user || (body.recipients?.length ?? 0) !== 1) return json({ message: "Invalid Recipient(s)", code: 50033 }, 400);
    const known = [...privates].find(([, channel]) => channel.type === 1 && channel.recipients[0] === user);
    if (known) return json(wirePrivate(known[0], known[1], "objects"));
    const id = snowflake(Date.now(), 77);
    const channel: Private = { type: 1, recipients: [user] };
    privates.set(id, channel);
    messages.set(id, []);
    dispatch("CHANNEL_CREATE", wirePrivate(id, channel, "objects"));
    return json(wirePrivate(id, channel, "objects"));
  }

  if (method === "GET" && path === "/users/@me") {
    return json({ ...wireUser(users.alice), email: "alice@example.org", verified: true });
  }

  if ((m = path.match(/^\/channels\/(\d+)\/messages$/))) {
    const list = messages.get(m[1]!);
    if (!list) return notFound("Unknown Channel", 10003);
    if (method === "GET") {
      const limit = Math.min(100, Math.max(1, Number(url.searchParams.get("limit") ?? 50)));
      const before = url.searchParams.get("before");
      const after = url.searchParams.get("after");
      const around = url.searchParams.get("around");
      let page = list;
      if (around) {
        // Half before the snowflake and half from it on, whether or not a
        // message has it.
        const older = list.filter((message) => BigInt(message.id) < BigInt(around));
        const newer = list.filter((message) => BigInt(message.id) >= BigInt(around));
        page = [...older.slice(-Math.floor(limit / 2)), ...newer.slice(0, Math.ceil(limit / 2))];
      }
      if (before) page = page.filter((message) => BigInt(message.id) < BigInt(before));
      if (after) page = page.filter((message) => BigInt(message.id) > BigInt(after)).slice(0, limit);
      page = page.slice(-limit).reverse();
      historyRequests.push(url.search);
      return json(page.map((message) => wireMessage(message, origin)));
    }
    if (method === "POST") {
      const body: any = await req.json();
      const thread = threads.get(m[1]!);
      if (thread?.locked) return json({ message: "Thread is locked", code: 50083 }, 400);
      if (thread) {
        joinThread(thread);
        // Writing opens an archived thread again.
        if (thread.archived) {
          thread.archived = false;
          thread.archiveChanged = Date.now();
          dispatch("THREAD_UPDATE", wireThread(thread));
        }
      }
      const reply = body.message_reference?.message_id;
      const attached: File[] = [];
      for (const attachment of body.attachments ?? []) {
        const upload = uploads.get(attachment.uploaded_filename);
        if (!upload?.data) return json({ message: "Invalid Form Body", code: 50035 }, 400);
        uploads.delete(attachment.uploaded_filename);
        attached.push(
          addFile({
            id: nextId(Date.now()),
            channel: m[1]!,
            filename: upload.filename,
            contentType: contentType(upload.filename),
            data: upload.data,
            ...pngSize(upload.data),
          }),
        );
      }
      const message = post(m[1]!, users.alice, body.content ?? "", Date.now(), {
        nonce: body.nonce ? String(body.nonce) : undefined,
        reply: reply && byId.has(reply) ? reply : undefined,
        files: attached,
      });
      readStates.set(m[1]!, { last: message.id, mentions: 0 });
      dispatch("MESSAGE_CREATE", dispatched(message, origin));
      // A GIF the picker sent gets its embed a moment later, as Discord's
      // embeds come: the provider's picture, without a video here.
      const sentGif = gifCatalog.find((gif) => wireGif(gif, origin).url === message.content.trim());
      const linked = linkedGifEmbed(message.content, origin);
      if (linked) {
        setTimeout(() => {
          message.embeds = () => [linked];
          dispatch("MESSAGE_UPDATE", dispatched(message, origin));
        }, 300);
      } else if (sentGif) {
        setTimeout(() => {
          const wired = wireGif(sentGif, origin);
          message.embeds = () => [{
            type: "gifv", url: wired.url, provider: { name: "Klipy", url: "https://klipy.com" },
            thumbnail: { url: `https://static.klipy.com/${sentGif.name}.webp`, proxy_url: wired.src, width: sentGif.width, height: sentGif.height, content_type: "image/webp" },
          }];
          dispatch("MESSAGE_UPDATE", dispatched(message, origin));
        }, 300);
      }
      return json(wireMessage(message, origin));
    }
  }

  if ((m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)$/))) {
    const message = byId.get(m[2]!);
    if (!message || message.channel !== m[1]) return notFound("Unknown Message", 10008);
    if (message.author.id !== ALICE) {
      return json({ message: "Cannot edit a message authored by another user", code: 50005 }, 403);
    }
    if (method === "PATCH") {
      const body: any = await req.json();
      message.content = body.content ?? message.content;
      message.edited = Date.now();
      dispatch("MESSAGE_UPDATE", dispatched(message, origin));
      return json(wireMessage(message, origin));
    }
    if (method === "DELETE") {
      const list = messages.get(message.channel)!;
      list.splice(list.indexOf(message), 1);
      byId.delete(message.id);
      dispatch("MESSAGE_DELETE", { id: message.id, channel_id: message.channel, ...guildOf(message.channel) });
      return empty();
    }
  }

  // A moderator removing someone's reaction: /{emoji}/{type}/{user}.
  if (method === "DELETE" && (m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)\/reactions\/([^/]+)\/[01]\/(\d+)$/))) {
    const message = byId.get(m[2]!);
    if (!message || message.channel !== m[1]) return notFound("Unknown Message", 10008);
    const [name, id = null] = decodeURIComponent(m[3]!).split(":");
    const reaction = message.reactions.find((r) => r.name === name && r.id === id);
    const user = m[4]!;
    if (!reaction?.users.delete(user)) return empty();
    if (reaction.users.size === 0) message.reactions.splice(message.reactions.indexOf(reaction), 1);
    reactorRemovals.push(`${message.id} ${name} ${user}`);
    dispatch("MESSAGE_REACTION_REMOVE", {
      user_id: user, channel_id: message.channel, message_id: message.id, ...guildOf(message.channel),
      emoji: { id, name }, burst: false, type: 0,
    });
    return empty();
  }

  // Who reacted with an emoji, as Discord pages them: by user id, from
  // after ?after, up to ?limit (25 unless it says, 100 at most). Burst
  // reactions (?type=1) are none here.
  if (method === "GET" && (m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)\/reactions\/([^/]+)$/))) {
    const message = byId.get(m[2]!);
    if (!message || message.channel !== m[1]) return notFound("Unknown Message", 10008);
    const [name, id = null] = decodeURIComponent(m[3]!).split(":");
    const reaction = message.reactions.find((r) => r.name === name && r.id === id);
    if (!reaction) return notFound("Unknown Emoji", 10014);
    reactorPages.push(`${m[2]} ${decodeURIComponent(m[3]!)} ${url.search}`);
    if (url.searchParams.get("type") === "1") return json([]);
    const limit = Math.min(100, Math.max(1, Number(url.searchParams.get("limit") ?? 25)));
    const after = BigInt(url.searchParams.get("after") ?? 0);
    const page = [...reaction.users]
      .map((user) => userById.get(user)!)
      .filter((user) => BigInt(user.id) > after)
      .sort((a, b) => (BigInt(a.id) < BigInt(b.id) ? -1 : 1))
      .slice(0, limit);
    return json(page.map(wireUser));
  }

  if ((m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)\/reactions\/([^/]+)\/@me$/))) {
    const message = byId.get(m[2]!);
    if (!message || message.channel !== m[1]) return notFound("Unknown Message", 10008);
    const [name, id = null] = decodeURIComponent(m[3]!).split(":");
    let reaction = message.reactions.find((r) => r.name === name && r.id === id);
    const add = method === "PUT";
    if (!add && method !== "DELETE") return notFound();
    if (add && !reaction) message.reactions.push((reaction = { name: name!, id, users: new Set() }));
    if (!reaction || reaction.users.has(ALICE) === add) return empty();
    if (add) reaction.users.add(ALICE);
    else reaction.users.delete(ALICE);
    if (reaction.users.size === 0) message.reactions.splice(message.reactions.indexOf(reaction), 1);
    dispatch(add ? "MESSAGE_REACTION_ADD" : "MESSAGE_REACTION_REMOVE", {
      user_id: ALICE,
      channel_id: message.channel,
      message_id: message.id,
      ...guildOf(message.channel),
      emoji: { id, name },
      burst: false,
      type: 0,
      ...(add && !privates.has(message.channel) ? { member: member(users.alice) } : {}),
      ...(add ? { message_author_id: message.author.id } : {}),
    });
    return empty();
  }

  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/typing$/))) {
    if (!messages.has(m[1]!)) return notFound("Unknown Channel", 10003);
    typed.push(m[1]!);
    return empty();
  }

  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)\/ack$/))) {
    if (!messages.has(m[1]!)) return notFound("Unknown Channel", 10003);
    readStates.set(m[1]!, { last: m[2]!, mentions: 0 });
    acks.push([m[1]!]);
    dispatch("MESSAGE_ACK", { channel_id: m[1], message_id: m[2], version: 1, mention_count: 0 });
    return json({ token: null });
  }

  if (method === "POST" && path === "/read-states/ack-bulk") {
    const body: any = await req.json();
    const reads: any[] = body.read_states;
    if (!Array.isArray(reads) || reads.length === 0 || reads.length > 100
        || reads.some((read) => typeof read.channel_id !== "string" || typeof read.message_id !== "string")) {
      return json({ message: "Invalid Form Body", code: 50035 }, 400);
    }
    for (const read of reads) {
      if (!messages.has(read.channel_id)) continue;
      readStates.set(read.channel_id, { last: read.message_id, mentions: 0 });
      dispatch("MESSAGE_ACK", { channel_id: read.channel_id, message_id: read.message_id, version: 1, mention_count: 0 });
    }
    acks.push(reads.map((read) => read.channel_id));
    return empty();
  }

  // Adding someone to a group DM.
  if (method === "PUT" && (m = path.match(/^\/channels\/(\d+)\/recipients\/(\d+)$/))) {
    const channel = privates.get(m[1]!);
    const user = userById.get(m[2]!);
    if (channel?.type !== 3) return notFound("Unknown Channel", 10003);
    if (!user) return notFound("Unknown User", 10013);
    if (channel.recipients.length >= 9) return json({ message: "Maximum number of recipients reached (10)", code: 30014 }, 400);
    if (!channel.recipients.includes(user)) {
      channel.recipients.push(user);
      dispatch("CHANNEL_RECIPIENT_ADD", { channel_id: m[1]!, user: wireUser(user) });
    }
    return new Response(null, { status: 204 });
  }

  if (method === "GET" && (m = path.match(/^\/channels\/(\d+)\/messages\/pins$/))) {
    if (!messages.has(m[1]!)) return notFound("Unknown Channel", 10003);
    const items = (pins.get(m[1]!) ?? []).map((pin) => ({ pinned_at: iso(pin.at), message: wireMessage(pin.message, origin) }));
    return json({ items, has_more: false });
  }

  if (method === "GET" && (m = path.match(/^\/channels\/(\d+)\/messages\/search$/))) {
    const list = messages.get(m[1]!);
    if (!list || !privates.has(m[1]!)) return notFound("Unknown Channel", 10003);
    if (!indexed.has(m[1]!)) {
      indexed.add(m[1]!);
      return json({ message: "Index not yet available. Try again later", code: 110000, documents_indexed: 0, retry_after: 0 }, 202);
    }
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const found = list.filter(searchFilter(url)).reverse();
    return json({
      analytics_id: "fake",
      doing_deep_historical_index: false,
      total_results: found.length,
      messages: found.slice(offset, offset + 25).map((message) => [wireMessage(message, origin)]),
    });
  }

  // Starting a thread from a message: a public one, or an announcement
  // thread in an announcement channel, with the message's id. Its first
  // message points back at the message, which gains it, and its channel
  // hears of it if the message wasn't among its ten newest.
  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)\/threads$/))) {
    const [, parent, id] = m as unknown as [string, string, string];
    if (!messages.has(parent) || privates.has(parent) || threads.has(parent)) return notFound("Unknown Channel", 10003);
    const original = byId.get(id);
    if (!original || original.channel !== parent) return notFound("Unknown Message", 10008);
    const body: any = await req.json();
    threadCreates.push({ channel: parent, message: id, ...body });
    const invalid = threadNameError(body.name);
    if (invalid) return invalid;
    if (threads.has(id)) return json({ message: "A thread has already been created for this message", code: 160004 }, 400);
    const now = Date.now();
    const thread = makeThread(id, body.name.trim(), users.alice, now, [ALICE], { parent, autoArchive: body.auto_archive_duration ?? 4320 });
    post(thread.id, users.alice, "", now, { type: 21, starter: id });
    announceThread(thread);
    dispatch("MESSAGE_UPDATE", dispatched(original, origin));
    if (!messages.get(parent)!.slice(-10).includes(original)) noticeThread(thread, origin);
    return json(wireThread(thread), 201);
  }

  // Starting a thread without a message: public or private in a text
  // channel, as `type` says. A public one's channel hears of it.
  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/threads$/))) {
    const parent = m[1]!;
    if (!messages.has(parent) || privates.has(parent) || threads.has(parent)) return notFound("Unknown Channel", 10003);
    const body: any = await req.json();
    threadCreates.push({ channel: parent, ...body });
    const invalid = threadNameError(body.name);
    if (invalid) return invalid;
    if (body.type !== 11 && body.type !== 12) {
      return json({ message: "Invalid Form Body", code: 50035, errors: { type: { _errors: [{ code: "BASE_TYPE_CHOICES", message: "Value must be one of {11, 12}." }] } } }, 400);
    }
    const now = Date.now();
    const thread = makeThread(nextId(now), body.name.trim(), users.alice, now, [ALICE], { parent, type: body.type, autoArchive: body.auto_archive_duration ?? 4320 });
    announceThread(thread);
    if (thread.type !== 12) noticeThread(thread, origin);
    return json(wireThread(thread), 201);
  }

  // A channel's threads that Alice may see, as Discord's client lists them:
  // `archived` picks a kind, `name` finds them by the words of their names,
  // the newest message first, `limit` from the `offset`th. The first search
  // of each channel answers 202, as Discord's does while it indexes it.
  if (method === "GET" && (m = path.match(/^\/channels\/(\d+)\/threads\/search$/))) {
    const parent = m[1]!;
    if (!messages.has(parent) || privates.has(parent) || threads.has(parent)) return notFound("Unknown Channel", 10003);
    threadSearches.push(url.search.slice(1));
    if (!indexed.has(`threads:${parent}`)) {
      indexed.add(`threads:${parent}`);
      return json({ message: "Index not yet available. Try again later", code: 110000, documents_indexed: 0, retry_after: 0 }, 202);
    }
    const archived = url.searchParams.get("archived");
    const words = (url.searchParams.get("name") ?? "").toLowerCase().split(/\s+/).filter(Boolean);
    const limit = Math.min(25, Math.max(1, Number(url.searchParams.get("limit") ?? 25)));
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const found = [...threads.values()]
      .filter((thread) => thread.parent === parent && visibleThread(thread))
      .filter((thread) => archived === null || thread.archived === (archived === "true"))
      .filter((thread) => words.every((word) => thread.name.toLowerCase().includes(word)))
      .sort((a, b) => (threadActivity(b) > threadActivity(a) ? 1 : -1));
    const page = found.slice(offset, offset + limit);
    return json({
      threads: page.map((thread) => {
        const { member: _member, ...wire } = wireThread(thread) as Record<string, unknown>;
        return wire;
      }),
      members: page.filter((thread) => thread.members.has(ALICE)).map(wireThreadMember),
      has_more: offset + limit < found.length,
      total_results: found.length,
    });
  }

  // A server's messages, in the channels `channel_id` names or in all of
  // them, found by their text; the first search of each server answers 202,
  // as Discord's does while it indexes it.
  if (method === "GET" && (m = path.match(/^\/guilds\/(\d+)\/messages\/search$/))) {
    const guild = m[1]!;
    const channels = [...messages.keys()].filter((id) => !privates.has(id) && (fillerChannels.get(id) ?? GUILD) === guild);
    if (!channels.length) return notFound("Unknown Guild", 10004);
    const asked = url.searchParams.getAll("channel_id");
    if (asked.some((id) => !channels.includes(id))) return notFound("Unknown Channel", 10003);
    if (!indexed.has(guild)) {
      indexed.add(guild);
      return json({ message: "Index not yet available. Try again later", code: 110000, documents_indexed: 0, retry_after: 0 }, 202);
    }
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const found = (asked.length ? asked : channels)
      .flatMap((id) => messages.get(id)!)
      .filter((message) => message.type === 0 || message.type === 19)
      .filter(searchFilter(url))
      .sort((a, b) => (BigInt(b.id) > BigInt(a.id) ? 1 : -1));
    return json({
      analytics_id: "fake",
      doing_deep_historical_index: false,
      total_results: found.length,
      messages: found.slice(offset, offset + 25).map((message) => [{ ...wireMessage(message, origin), guild_id: guild }]),
    });
  }

  // Accepting a message request makes it a DM; ignoring it closes the DM.
  if ((m = path.match(/^\/channels\/(\d+)\/recipients\/@me$/))) {
    const id = m[1]!;
    const channel = privates.get(id);
    if (!channel?.request) return notFound("Unknown Channel", 10003);
    if (method === "PUT") {
      const body: any = await req.json();
      if (body.consent_status !== 2) return json({ message: "Invalid Form Body", code: 50035 }, 400);
      channel.request = false;
      channel.spam = false;
      const wire = wirePrivate(id, channel, "objects");
      dispatch("CHANNEL_UPDATE", wire);
      return json(wire);
    }
    if (method === "DELETE") {
      privates.delete(id);
      const wire = { ...wirePrivate(id, channel, "objects"), is_message_request: false };
      const last = messages.get(id)!.at(-1)!.id;
      readStates.set(id, { last, mentions: 0 });
      dispatch("CHANNEL_UPDATE", wire);
      dispatch("MESSAGE_ACK", { channel_id: id, message_id: last, version: 1, mention_count: 0 });
      dispatch("CHANNEL_DELETE", wire);
      return json(wire);
    }
  }

  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/typing$/))) {
    return messages.has(m[1]!) ? empty() : notFound("Unknown Channel", 10003);
  }

  // Signing out ends the session; the fake counts them.
  if (method === "POST" && path === "/auth/logout") {
    logouts++;
    return empty();
  }

  // Alice's settings: Talk changes the status or the custom status, which
  // every session hears of. The fake keeps only the latest change.
  if (method === "PATCH" && path === "/users/@me/settings-proto/1") {
    const body: any = await req.json().catch(() => ({}));
    if (typeof body.settings !== "string") return json({ message: "Invalid Form Body", code: 50035 }, 400);
    settings = body.settings;
    dispatch("USER_SETTINGS_PROTO_UPDATE", { settings: { type: 1, proto: settings }, partial: true });
    return json({ settings });
  }

  // The second settings: favourite GIFs.
  if (method === "GET" && path === "/users/@me/settings-proto/2") {
    return json({ settings: frecencySettings ??= favoriteGifs(origin) });
  }
  if (method === "PATCH" && path === "/users/@me/settings-proto/2") {
    const body: any = await req.json().catch(() => ({}));
    if (typeof body.settings !== "string") return json({ message: "Invalid Form Body", code: 50035 }, 400);
    frecencySettings = body.settings;
    dispatch("USER_SETTINGS_PROTO_UPDATE", { settings: { type: 2, proto: frecencySettings }, partial: true });
    return json({ settings: frecencySettings });
  }

  // The GIF picker's searches.
  if (method === "GET" && path === "/gifs/trending") {
    return json({
      categories: gifCategories.map((name) => ({ name, src: wireGif(gifsFor(name)[0]!, origin).src })),
      gifs: [wireGif(gifCatalog[5]!, origin)],
    });
  }
  if (method === "GET" && path === "/gifs/trending-gifs") {
    return json(gifCatalog.map((gif) => wireGif(gif, origin)));
  }
  if (method === "GET" && path === "/gifs/search") {
    return json(gifsFor(url.searchParams.get("q") ?? "").map((gif) => wireGif(gif, origin)));
  }
  if (method === "GET" && path === "/gifs/suggest") {
    const q = (url.searchParams.get("q") ?? "").toLowerCase();
    const limit = Number(url.searchParams.get("limit") ?? 20);
    const terms = [...new Set(gifCatalog.flatMap((gif) => gif.terms))];
    return json(terms.filter((term) => term !== q).slice(0, limit));
  }
  if (method === "POST" && path === "/gifs/select") {
    const body: any = await req.json().catch(() => ({}));
    gifSelects.push({ id: String(body.id), q: String(body.q ?? "") });
    return new Response(null, { status: 204 });
  }

  // Friend requests: sending one, or accepting one, from where Discord's
  // client does it, which it says in X-Context-Properties.
  const where = () => {
    try {
      return JSON.parse(atob(req.headers.get("x-context-properties") ?? "")).location as string | undefined;
    } catch {
      return undefined;
    }
  };
  const befriend = (user: User, type: number) => {
    relationships.set(user.id, type);
    dispatch("RELATIONSHIP_ADD", {
      id: user.id, type, user: wireUser(user), nickname: null, since: new Date().toISOString(), should_notify: type === 3,
    });
  };
  // Sends Alice's request, or accepts theirs; Dave accepts hers a moment later.
  const request = (user: User) => {
    const before = relationships.get(user.id);
    if (before === 1 || before === 4) return;
    befriend(user, before === 3 ? 1 : 4);
    if (before !== 3 && user.id === users.dave.id) {
      setTimeout(() => relationships.get(user.id) === 4 && befriend(user, 1), FRIEND_ACCEPT_MS);
    }
  };
  if (method === "POST" && path === "/users/@me/relationships") {
    if (where() !== "Add Friend") return json({ message: "Invalid Form Body", code: 50035 }, 400);
    const body: any = await req.json().catch(() => ({}));
    const user = [...userById.values()].find((user) => user.username === body.username && user.id !== ALICE);
    if (!user) return json({ message: "Unknown User", code: 80004 }, 400);
    if (relationships.get(user.id) === 1) return json({ message: "You're already friends with that user.", code: 80007 }, 400);
    if (user.id === strangers.frank!.id) return json({ message: "Friend request failed", code: 80000 }, 400);
    request(user);
    return empty();
  }
  if ((m = path.match(/^\/users\/@me\/relationships\/(\d+)$/)) && (method === "PUT" || method === "DELETE")) {
    const id = m[1]!;
    const user = userById.get(id);
    if (!user || id === ALICE) return notFound("Unknown User", 10013);
    if (!where()) return json({ message: "Invalid Form Body", code: 50035 }, 400);
    const before = relationships.get(id);
    if (method === "DELETE") {
      if (before === undefined) return notFound("Unknown Relationship", 10011);
      relationships.delete(id);
      nicknames.delete(id);
      dispatch("RELATIONSHIP_REMOVE", { id, type: before, nickname: null });
      return empty();
    }
    const body: any = await req.json().catch(() => ({}));
    if (body.type === 2) {
      nicknames.delete(id);
      befriend(user, 2);
      return empty();
    }
    request(user);
    return empty();
  }
  if ((m = path.match(/^\/users\/@me\/relationships\/(\d+)\/ignore$/)) && (method === "PUT" || method === "DELETE")) {
    const id = m[1]!;
    if (!userById.get(id) || id === ALICE) return notFound("Unknown User", 10013);
    if (method === "PUT") ignored.add(id);
    else ignored.delete(id);
    dispatch("RELATIONSHIP_UPDATE", wireRelationship(id, relationships.get(id) ?? 0));
    return empty();
  }
  if (method === "PATCH" && (m = path.match(/^\/users\/@me\/relationships\/(\d+)$/))) {
    const id = m[1]!;
    if (relationships.get(id) !== 1) return json({ message: "Invalid Form Body", code: 50035 }, 400);
    const body: any = await req.json().catch(() => ({}));
    if (typeof body.nickname === "string" && body.nickname.length > 32) {
      return json({ message: "Invalid Form Body", code: 50035 }, 400);
    }
    if (body.nickname) nicknames.set(id, body.nickname);
    else nicknames.delete(id);
    dispatch("RELATIONSHIP_UPDATE", wireRelationship(id, 1));
    return empty();
  }

  // Alice's notes on people: Discord answers 404 for someone she has none on.
  if ((m = path.match(/^\/users\/@me\/notes\/(\d+)$/))) {
    const id = m[1]!;
    if (method === "GET") {
      const note = notes.get(id);
      if (note === undefined) return notFound("Unknown User", 10013);
      return json({ note, note_user_id: id, user_id: ALICE });
    }
    if (method === "PUT") {
      const body: any = await req.json().catch(() => ({}));
      if (typeof body.note === "string" && body.note.length > 256) {
        return json({ message: "Invalid Form Body", code: 50035 }, 400);
      }
      if (body.note) notes.set(id, body.note);
      else notes.delete(id);
      dispatch("USER_NOTE_UPDATE", { id, note: body.note ?? "" });
      return empty();
    }
  }

  // "Server"'s channels and categories, which Crew, and so Alice, may
  // manage: making one, reading, changing and deleting it, as Discord does.
  if (method === "POST" && path === `/guilds/${GUILD}/channels`) {
    const body: any = await req.json().catch(() => ({}));
    const type = body.type ?? 0;
    const known = guildChannels();
    const parent = body.parent_id ?? null;
    const invalid =
      typeof body.name !== "string" || body.name.length < 1 || body.name.length > 100 || ![0, 2, 4].includes(type) ||
      (parent !== null && (type === 4 || !known.some((channel) => channel.id === parent && channel.type === 4))) ||
      (body.topic != null && String(body.topic).length > 4096) ||
      (body.rate_limit_per_user != null && !(body.rate_limit_per_user >= 0 && body.rate_limit_per_user <= 21600));
    if (invalid) return json({ message: "Invalid Form Body", code: 50035 }, 400);
    const id = nextId(Date.now());
    // Discord makes text channel names lower case, with dashes for spaces.
    const name = type === 0 ? body.name.trim().toLowerCase().replace(/\s+/g, "-") : body.name.trim();
    const siblings = known.filter((channel) => channel.parent_id === parent && (channel.type === 4) === (type === 4));
    const channel = guildChannel(id, name, type, body.position ?? siblings.length, parent);
    channel.permission_overwrites = body.permission_overwrites ?? [];
    if (type === 0) {
      messages.set(id, []);
      Object.assign(channel, { topic: body.topic ?? null, nsfw: body.nsfw === true, rate_limit_per_user: body.rate_limit_per_user ?? 0 });
    }
    madeChannels.set(id, channel);
    dispatch("CHANNEL_CREATE", channel);
    return json(channel, 201);
  }
  if (method === "GET" && (m = path.match(/^\/channels\/(\d+)$/)) && threads.has(m[1]!)) {
    return json(wireThread(threads.get(m[1]!)!));
  }
  if ((m = path.match(/^\/channels\/(\d+)$/)) && guildChannels().some((channel) => channel.id === m![1])) {
    const id = m[1]!;
    const channel = () => guildChannels().find((known) => known.id === id)!;
    if (method === "GET") return json(channel());
    if (method === "PATCH") {
      const body: any = await req.json().catch(() => ({}));
      const before = channel();
      if (("name" in body && (typeof body.name !== "string" || body.name.length < 1 || body.name.length > 100)) ||
        (body.topic != null && String(body.topic).length > 4096) ||
        (body.rate_limit_per_user != null && !(body.rate_limit_per_user >= 0 && body.rate_limit_per_user <= 21600))) {
        return json({ message: "Invalid Form Body", code: 50035 }, 400);
      }
      const edit: Partial<GuildChannel> = { ...channelEdits.get(id) };
      if ("name" in body) edit.name = before.type === 0 ? body.name.trim().toLowerCase().replace(/\s+/g, "-") : body.name.trim();
      for (const key of ["position", "parent_id"] as const) if (key in body) (edit as any)[key] = body[key];
      if (before.type === 0) {
        for (const key of ["topic", "nsfw", "rate_limit_per_user"] as const) if (key in body) (edit as any)[key] = body[key];
      }
      channelEdits.set(id, edit);
      dispatch("CHANNEL_UPDATE", channel());
      return json(channel());
    }
    if (method === "DELETE") {
      const gone = channel();
      deletedChannels.add(id);
      // A category's channels stay, outside any category.
      if (gone.type === 4) {
        for (const child of guildChannels().filter((known) => known.parent_id === id)) {
          channelEdits.set(child.id, { ...channelEdits.get(child.id), parent_id: null });
          dispatch("CHANNEL_UPDATE", { ...child, parent_id: null });
        }
      }
      dispatch("CHANNEL_DELETE", gone);
      return json(gone);
    }
  }

  // Leaving "Server", which Bob owns, as a member rather than lurking.
  if (method === "DELETE" && path === `/users/@me/guilds/${GUILD}`) {
    if (leftServer) return json({ message: "Unknown Guild", code: 10004 }, 404);
    leftServer = true;
    dispatch("GUILD_DELETE", { id: GUILD });
    dispatch("GUILD_MEMBER_REMOVE", { guild_id: GUILD, user: wireUser(users.alice) });
    return new Response(null, { status: 204 });
  }

  // Closing a DM takes it out of her list until either of them writes.
  if (method === "DELETE" && (m = path.match(/^\/channels\/(\d+)$/))) {
    const dm = privates.get(m[1]!);
    if (!dm) return notFound("Unknown Channel", 10003);
    privates.delete(m[1]!);
    const wire = wirePrivate(m[1]!, dm, "objects");
    dispatch("CHANNEL_DELETE", wire);
    return json(wire);
  }

  // Alice's notification settings for a server or her DMs: what the body
  // gives changes, and the rest stays. Channels' overrides come by id.
  const knownGuild = (guild: string) => guild === GUILD || fillers.some((filler) => filler.id === guild);
  const changeGuildSettings = (guild: string, body: any) => {
    const settings = settingsOf(guild);
    for (const key of ["muted", "message_notifications", "suppress_everyone", "suppress_roles", "hide_muted_channels"] as const) {
      if (key in body) (settings as any)[key] = body[key];
    }
    if ("mute_config" in body) settings.mute_config = body.mute_config;
    if (!settings.muted) settings.mute_config = null;
    for (const [channel, change] of Object.entries<any>(body.channel_overrides ?? {})) {
      const override = settings.overrides.get(channel) ?? { muted: false, mute_config: null, message_notifications: 3, collapsed: false, flags: 0 };
      for (const key of ["muted", "mute_config", "message_notifications", "collapsed", "flags"] as const) {
        if (key in change) (override as any)[key] = change[key];
      }
      if (!override.muted) override.mute_config = null;
      settings.overrides.set(channel, override);
    }
    const wire = wireGuildSettings(guild);
    dispatch("USER_GUILD_SETTINGS_UPDATE", wire);
    return wire;
  };
  if (method === "PATCH" && (m = path.match(/^\/users\/@me\/guilds\/(\d+|@me)\/settings$/))) {
    const guild = m[1]!;
    if (guild !== "@me" && !knownGuild(guild)) return notFound("Unknown Guild", 10004);
    const body: any = await req.json().catch(() => ({}));
    return json(changeGuildSettings(guild, body));
  }
  // Several servers' at once, never her DMs': a list of each one's
  // settings, and an update for each.
  if (method === "PATCH" && path === "/users/@me/guilds/settings") {
    const body: any = await req.json().catch(() => ({}));
    const guilds = Object.entries<any>(body.guilds ?? {});
    if (guilds.length === 0 || guilds.some(([guild]) => !knownGuild(guild))) {
      return json({ message: "Invalid Form Body", code: 50035 }, 400);
    }
    return json(guilds.map(([guild, change]) => changeGuildSettings(guild, change)));
  }

  // Alice's own profile in "Server": banner, bio and pronouns, as the
  // per-server profile editor sends them; see "Server members" above.
  if (method === "PATCH" && (m = path.match(/^\/guilds\/(\d+)\/profile\/@me$/))) {
    if (m[1] !== GUILD) return notFound("Unknown Guild", 10004);
    const body: any = await req.json().catch(() => ({}));
    const profile = serverProfile(ALICE);
    if (typeof body.pronouns === "string" && body.pronouns.length > 40) return tooLong("pronouns", 40);
    if (typeof body.bio === "string" && body.bio.length > 190) return tooLong("bio", 190);
    if (body.bio && !nitro) return premiumRequired("bio");
    if (body.banner && !nitro) return premiumRequired("banner");
    let banner: string | null | undefined;
    if ("banner" in body) {
      const kept = body.banner === null ? { hash: null } : keepPicture(body.banner);
      if ("error" in kept) return json({ message: "Invalid Form Body", code: 50035, errors: { banner: { _errors: [{ code: "IMAGE_INVALID", message: kept.error }] } } }, 400);
      banner = kept.hash;
    }
    if (typeof body.pronouns === "string" || body.pronouns === null) profile.pronouns = body.pronouns ?? "";
    if (typeof body.bio === "string" || body.bio === null) profile.bio = body.bio ?? "";
    if (banner !== undefined) profile.banner = banner;
    return json(guildMemberProfile(ALICE));
  }

  // A member's menu in "Server": nicknames, roles, timeouts, kicks and
  // bans, as far as Alice's roles allow; see "Server members" above.
  if ((m = path.match(/^\/guilds\/(\d+)\/(members|bans)\/(\d+|@me)(?:\/roles\/(\d+))?$/))) {
    if (m[1] !== GUILD) return notFound("Unknown Guild", 10004);
    const [kind, who, role] = [m[2]!, m[3]!, m[4]];
    const user = userById.get(who === "@me" ? ALICE : who);
    if (!user || (removed.has(user.id) && kind === "members")) return notFound("Unknown Member", 10007);
    const refused = () => json({ message: "Missing Permissions", code: 50013 }, 403);
    const header = req.headers.get("x-audit-log-reason");
    const audit = (action: string) =>
      auditLog.push({ action, user: user.id, reason: header === null ? null : decodeURIComponent(header) });
    if (kind === "members" && role !== undefined && (method === "PUT" || method === "DELETE")) {
      const known = serverRoles.find((found) => found.id === role);
      if (!known) return notFound("Unknown Role", 10011);
      // Below her highest role, and not @everyone or a managed one.
      if (!aliceMay(28) || known.id === GUILD || known.managed || known.position >= rank(ALICE)) return refused();
      changeMembers(() => {
        const roles = (memberRoles.get(user.id) ?? []).filter((held) => held !== role);
        memberRoles.set(user.id, method === "PUT" ? [...roles, role] : roles);
        memberUpdated(user);
      });
      return empty();
    }
    if (kind === "members" && role === undefined && method === "PATCH") {
      const body: any = await req.json().catch(() => ({}));
      if ("nick" in body && !(who === "@me" ? aliceMay(26) : aliceMay(27, user.id))) return refused();
      if ("nick" in body && (body.nick ?? "").length > 32) return tooLong("nick", 32);
      let avatar: string | null | undefined;
      if (who === "@me" && "avatar" in body) {
        if (body.avatar !== null && !nitro) return premiumRequired("avatar");
        const kept = body.avatar === null ? { hash: null } : keepPicture(body.avatar);
        if ("error" in kept) return json({ message: "Invalid Form Body", code: 50035, errors: { avatar: { _errors: [{ code: "IMAGE_INVALID", message: kept.error }] } } }, 400);
        avatar = kept.hash;
      }
      // Muting, deafening and moving need only the permission, not rank,
      // and someone in the server's voice; a null channel disconnects.
      const inVoice = voice.get(user.id)?.guild === GUILD ? voice.get(user.id) : undefined;
      const voiceFields = ["mute", "deaf", "channel_id"].filter((field) => field in body);
      if (voiceFields.length) {
        if (("mute" in body && !aliceMay(22)) || ("deaf" in body && !aliceMay(23)) || ("channel_id" in body && !aliceMay(24))) return refused();
        if (!inVoice) return json({ message: "Target user is not connected to voice.", code: 40032 }, 400);
        const to = body.channel_id;
        if (to && !guild().channels.some((channel) => channel.id === to && channel.type === 2)) {
          return json({ message: "Invalid Form Body", code: 50035 }, 400);
        }
        const before = { ...inVoice };
        if ("channel_id" in body && to !== inVoice.channel) endStream(streamKey(GUILD, inVoice.channel, user.id), "user_requested");
        if ("mute" in body) inVoice.mute = !!body.mute;
        if ("deaf" in body) inVoice.deaf = !!body.deaf;
        if ("channel_id" in body) {
          if (to) inVoice.channel = to;
          else voice.delete(user.id);
        }
        moved(user.id, before, origin);
        voiceChanges.push({ user: user.id, ...Object.fromEntries(voiceFields.map((field) => [field, body[field]])) });
      }
      if ("communication_disabled_until" in body) {
        const until = body.communication_disabled_until;
        const admin = (serverPermissions(user.id) & (1n << 3n)) !== 0n;
        if (!aliceMay(40, user.id) || admin) return refused();
        if (until !== null && Date.parse(until) > Date.now() + 28 * 86_400_000) {
          return json({ message: "Invalid Form Body", code: 50035 }, 400);
        }
      }
      changeMembers(() => {
        if ("nick" in body) {
          if (body.nick) memberNicks.set(user.id, body.nick);
          else memberNicks.delete(user.id);
        }
        if (avatar !== undefined) serverProfile(user.id).avatar = avatar;
        if ("communication_disabled_until" in body) {
          const until = body.communication_disabled_until;
          if (until === null) timeouts.delete(user.id);
          else timeouts.set(user.id, until);
          audit(until === null ? "timeout-removed" : "timeout");
        }
        memberUpdated(user);
      });
      return json({ ...member(user), guild_id: undefined });
    }
    const removing = (kind === "members" && role === undefined && method === "DELETE") || (kind === "bans" && method === "PUT");
    if (removing) {
      if (!aliceMay(kind === "bans" ? 2 : 1, user.id)) return refused();
      const body: any = kind === "bans" ? await req.json().catch(() => ({})) : {};
      audit(kind === "bans" ? "ban" : "kick");
      changeMembers(() => {
        removed.add(user.id);
        if (kind === "bans") {
          dispatch("GUILD_BAN_ADD", { guild_id: GUILD, user: wireUser(user), delete_message_secs: Number(body.delete_message_seconds ?? 0) });
        }
        dispatch("GUILD_MEMBER_REMOVE", { guild_id: GUILD, user: wireUser(user) });
      });
      return empty();
    }
  }

  // Alice may ring her friends in a DM, and anyone in a group.
  if (method === "GET" && (m = path.match(/^\/channels\/(\d+)\/call$/))) {
    const dm = privates.get(m[1]!);
    if (!dm) return notFound("Unknown Channel", 10003);
    return json({ ringable: ringable(dm) });
  }

  // Ringing for a DM call: everyone not in it, or these people; stopping
  // rings no more, by default this account, which declines.
  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/call\/(ring|stop-ringing)$/))) {
    const channel = m[1]!;
    const call = calls.get(channel);
    const dm = privates.get(channel);
    if (!call || !dm) return notFound("Unknown Channel", 10003);
    const body: any = await req.json();
    const recipients: string[] | null = body.recipients ?? null;
    if (m[2] === "ring") {
      if (!ringable(dm)) return json({ message: "Missing Permissions", code: 50013 }, 403);
      const inCall = (user: string) => voice.get(user)?.channel === channel;
      ring(channel, (recipients ?? dm.recipients.map((user) => user.id)).filter((user) => !inCall(user)), origin);
    } else {
      for (const user of recipients ?? [ALICE]) call.ringing.delete(user);
      dispatch("CALL_UPDATE", wireCall(channel));
    }
    return empty();
  }

  // An invite to a channel of one of Alice's servers, as Discord makes
  // them: it lasts max_age seconds (a day unless asked, never with 0) and
  // works max_uses times (any number with 0). `unique` asks for a new one
  // rather than a similar one reused, which the fake never does anyway.
  // Each keeps where Alice's client said it was made, as `location`.
  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/invites$/))) {
    const server = [guild(), ...fillers].find((known) => known.channels.some((channel) => channel.id === m![1]));
    const channel = server?.channels.find((known) => known.id === m![1] && known.type !== 4);
    if (!server || !channel) return notFound("Unknown Channel", 10003);
    const body: any = await req.json().catch(() => ({}));
    if (body.unique !== undefined && typeof body.unique !== "boolean") {
      return json({ message: "Invalid Form Body", code: 50035 }, 400);
    }
    const maxAge = body.max_age ?? 86400;
    const maxUses = body.max_uses ?? 0;
    if (!Number.isInteger(maxAge) || maxAge < 0 || maxAge > 5184000 || !Number.isInteger(maxUses) || maxUses < 0 || maxUses > 100) {
      return json({ message: "Invalid Form Body", code: 50035 }, 400);
    }
    const now = Date.now();
    const code = Array.from({ length: 8 }, () => "abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789"[Math.floor(Math.random() * 57)]).join("");
    const invite = {
      type: 0,
      code,
      inviter: wireUser(users.alice),
      max_age: maxAge,
      created_at: iso(now),
      expires_at: maxAge === 0 ? null : iso(now + maxAge * 1000),
      guild: { id: server.id, name: server.properties.name, icon: server.properties.icon, splash: null, banner: null, description: null, features: [], verification_level: 0, vanity_url_code: null, nsfw_level: 0, premium_subscription_count: 0 },
      guild_id: server.id,
      channel: { id: channel.id, type: channel.type, name: channel.name },
      uses: 0,
      max_uses: maxUses,
      temporary: body.temporary === true,
    };
    invites.push({ ...invite, location: where() });
    const { guild: _, channel: wire, ...rest } = invite;
    dispatch("INVITE_CREATE", { ...rest, channel_id: wire.id, guild_id: server.id });
    return json(invite);
  }

  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/attachments$/))) {
    const body: any = await req.json();
    const attachments = (body.files ?? []).map((file: any) => {
      const name = `uploads/${++uploadCount}/${file.filename}`;
      uploads.set(name, { filename: file.filename });
      return {
        id: file.id,
        upload_url: `${origin}/_upload/${name}`,
        upload_filename: name,
      };
    });
    return json({ attachments });
  }

  if (method === "DELETE" && path.startsWith("/attachments/")) {
    uploads.delete(decodeURIComponent(path.slice("/attachments/".length)));
    return empty();
  }

  return unknown(req, path);
}

function unknown(req: Request, path: string): Response {
  console.error(`unhandled: ${req.method} ${path}`);
  return notFound();
}

function serveFile(path: string, url: URL): Response {
  const file = files.get(decodeURIComponent(path));
  if (!file) return notFound();
  let data = file.data;
  const width = Number(url.searchParams.get("width"));
  const height = Number(url.searchParams.get("height"));
  if (file.draw && width > 0 && height > 0 && width <= 4096 && height <= 4096) {
    const key = `${path}@${width}x${height}`;
    data = scaled.get(key) ?? file.draw(width, height);
    scaled.set(key, data);
  }
  return new Response(data, { headers: { "content-type": file.contentType } });
}

// --- Embeds -----------------------------------------------------------------

// A bot that posts cards; it shares no server member list with anyone.
const statusBot: User = { id: "1400000000000000007", username: "statusbot", global_name: "Status Bot", avatar: null, bot: true };

/** An embed picture as Discord's proxy gives it: drawn here at any size. */
function proxied(origin: string, name: string, width: number, height: number) {
  return {
    url: `https://example.com/${name}.png`,
    proxy_url: `${origin}/external/${name}/${width}x${height}.png`,
    width,
    height,
    content_type: "image/png",
  };
}

// A GIF as Tenor serves it, an MP4: three seconds into the Mandelbrot set.
const gifVideo = readFileSync(new URL("gif-mandelbrot.mp4", fixtures));

// Serves /external/{name}/{width}x{height}.png at its size, or at the
// width and height asked for, as the media proxy scales, and a GIF's video
// at /external/{name}/video.mp4, the linked post's clip at
// /external/post/video.mp4, and a GIF provider's file linked directly at
// /external/klipy/https/static.klipy.com/ii/{name}.{webp,gif}.
function serveExternal(path: string, url: URL): Response | undefined {
  const linked = path.match(/^\/external\/klipy\/https\/static\.klipy\.com\/ii\/([a-z]+)\.(webp|gif)$/);
  if (linked) {
    try {
      const data = readFileSync(new URL(`gifs/${linked[1]}.${linked[2]}`, fixtures));
      return new Response(data, { headers: { "content-type": `image/${linked[2]}` } });
    } catch {
      return undefined;
    }
  }
  if (/^\/external\/[\w-]+\/video\.mp4$/.test(path)) {
    const data = path === "/external/post/video.mp4" ? clip : gifVideo;
    return new Response(data, { headers: { "content-type": "video/mp4" } });
  }
  const m = path.match(/^\/external\/([\w-]+)\/(\d+)x(\d+)\.png$/);
  if (!m) return undefined;
  let [width, height] = [Number(m[2]), Number(m[3])];
  const [askedWidth, askedHeight] = [Number(url.searchParams.get("width")), Number(url.searchParams.get("height"))];
  if (askedWidth > 0 && askedHeight > 0) [width, height] = [askedWidth, askedHeight];
  if (width > 4096 || height > 4096) return notFound();
  const key = `${path}@${width}x${height}`;
  let data = scaled.get(key);
  if (!data) {
    const hue = crc32(new TextEncoder().encode(m[1]!));
    const light = [(hue >>> 16) & 255, (hue >>> 8) & 255, hue & 255].map((c) => 96 + (c % 160));
    const dark = light.map((c) => c >> 1);
    data = png(width, height, (x, y) => [...((x / width) * 0.6 + (y / height) * 0.4 < 0.5 ? light : dark), 255]);
    scaled.set(key, data);
  }
  return new Response(data, { headers: { "content-type": "image/png" } });
}

const ARTICLE = "https://example.com/news/bridges";
function articleEmbed(origin: string) {
  return {
    type: "article",
    url: ARTICLE,
    title: "City opens its first bridge for bikes only",
    description: "The 300-metre crossing links the old town to the station, and **cars** are kept off it for good.",
    color: 0x2b6cb0,
    provider: { name: "Example News" },
    thumbnail: proxied(origin, "bridge", 1200, 630),
  };
}

function postEmbeds(origin: string) {
  const now = Date.now();
  const article = post(GENERAL, users.bob, `Have you seen this? ${ARTICLE}`, now);
  dispatch("MESSAGE_CREATE", dispatched(article, origin));
  // Discord fetches the page after the message is sent, then updates it.
  setTimeout(() => {
    article.embeds = (origin) => [articleEmbed(origin)];
    dispatch("MESSAGE_UPDATE", dispatched(article, origin));
  }, 800);
  const video = post(GENERAL, users.bob, "This one too https://www.youtube.com/watch?v=talk", now + 1, {
    embeds: (origin) => [
      {
        type: "video",
        url: "https://www.youtube.com/watch?v=talk",
        title: "Building a bike bridge in 90 seconds",
        description: "A time-lapse of the whole build.",
        color: 0xff0000,
        provider: { name: "YouTube", url: "https://www.youtube.com" },
        author: { name: "Example City", url: "https://www.youtube.com/@example" },
        thumbnail: proxied(origin, "timelapse", 1280, 720),
        video: { url: "https://www.youtube.com/embed/talk", width: 1280, height: 720 },
      },
    ],
  });
  dispatch("MESSAGE_CREATE", dispatched(video, origin));
  const gif = post(GENERAL, users.bob, "https://tenor.com/view/bike-gif-1", now + 2, {
    embeds: (origin) => [
      {
        type: "gifv",
        url: "https://tenor.com/view/bike-gif-1",
        provider: { name: "Tenor", url: "https://tenor.co" },
        thumbnail: proxied(origin, "bike-gif", 498, 280),
        video: { url: "https://media.tenor.com/bike.mp4", proxy_url: `${origin}/external/bike-gif/video.mp4`, width: 498, height: 280 },
      },
    ],
  });
  dispatch("MESSAGE_CREATE", dispatched(gif, origin));
  const card = post(GENERAL, statusBot, "", now + 3, {
    embeds: (origin) => [
      {
        type: "rich",
        title: "Server status",
        url: "https://status.example.com",
        description: "Everything is running. Ask <@" + users.bob.id + "> if something looks off.",
        color: 0x57f287,
        fields: [
          { name: "Uptime", value: "**99.98%**", inline: true },
          { name: "Players", value: "42", inline: true },
          { name: "Region", value: "Europe", inline: true },
          { name: "Notes", value: "Maintenance on Sunday, see https://status.example.com/maintenance", inline: false },
        ],
        footer: { text: "Status Bot" },
        timestamp: iso(now),
        thumbnail: proxied(origin, "status", 256, 256),
      },
    ],
  });
  dispatch("MESSAGE_CREATE", dispatched(card, origin));
}

/** A custom emoji or sticker: a ring of colour picked from its id, transparent around it. */
function badge(id: string, size: number): Uint8Array {
  const hue = crc32(new TextEncoder().encode(id));
  const color = [(hue >>> 16) & 255, (hue >>> 8) & 255, hue & 255].map((c) => 96 + (c % 160));
  return png(size, size, (x, y) => {
    const distance = Math.hypot(x + 0.5 - size / 2, y + 0.5 - size / 2) / (size / 2);
    if (distance > 1) return [0, 0, 0, 0];
    if (distance > 0.8) return [...color.map((c) => c >> 1), 255];
    return distance < 0.35 ? [255, 255, 255, 255] : [...color, 255];
  });
}

function postShowcase(origin: string) {
  const now = Date.now();
  const send = (message: Message) => dispatch("MESSAGE_CREATE", dispatched(message, origin));
  send(post(GENERAL, users.bob, "The ending: ||the butler did it||, and ||**nobody** saw it coming||.", now));
  send(post(GENERAL, users.bob, `Ship it <:talk:${EMOJI_TALK}> today <a:party:${EMOJI_PARTY}>`, now + 1));
  const jumbo = post(GENERAL, users.bob, `<:talk:${EMOJI_TALK}><a:party:${EMOJI_PARTY}>`, now + 2);
  jumbo.reactions.push({ name: "talk", id: EMOJI_TALK, users: new Set([users.carol.id]) });
  send(jumbo);
  send(post(GENERAL, users.bob, "", now + 3, {
    stickers: [
      { id: STICKER_WAVE, name: "Wave", format_type: 1 },
      { id: STICKER_BOUNCE, name: "Bounce", format_type: 4 },
    ],
  }));
  const dance = addFile({
    id: snowflake(now + 3, 2),
    channel: GENERAL,
    filename: "dance.gif",
    contentType: "image/gif",
    data: bouncing(120, 12, [255, 191, 71]),
    width: 120,
    height: 120,
  });
  send(post(GENERAL, users.bob, "It moves", now + 3, { files: [dance] }));
  const hidden = addFile({
    id: snowflake(now + 4, 1),
    channel: GENERAL,
    filename: "SPOILER_sunset.png",
    contentType: "image/png",
    data: sunset(800, 500),
    width: 800,
    height: 500,
    draw: sunset,
  });
  send(post(GENERAL, users.bob, "Don't look", now + 4, { files: [hidden] }));
  send(post(GENERAL, users.bob, "As the welcome said", now + 5, { reply: messages.get(GENERAL)![0]!.id }));
  relationships.set(users.dave.id, 2);
  dispatch("RELATIONSHIP_ADD", { id: users.dave.id, type: 2, nickname: null, user: wireUser(users.dave), since: iso(now) });
  send(post(GENERAL, users.dave, "Alice, you can't see this", now + 6));
}

// A clip with sound, as people attach them: six seconds of test pattern and
// beeps. Its poster, which Discord's proxy makes from the first frame, is
// drawn here instead.
const clip = readFileSync(new URL("video-clip.mp4", fixtures));

function postVideo(origin: string) {
  const now = Date.now();
  const video = addFile({
    id: snowflake(now, 1),
    channel: GENERAL,
    filename: "clip.mp4",
    contentType: "video/mp4",
    data: new Uint8Array(clip),
    width: 480,
    height: 270,
    draw: sunset,
  });
  dispatch("MESSAGE_CREATE", dispatched(post(GENERAL, users.bob, "Watch this", now, { files: [video] }), origin));
  // vxtwitter's card for a post with a video: rich, with the video the
  // proxy has beside a thumbnail.
  const link = "https://vxtwitter.com/bob/status/1";
  const linked = post(GENERAL, users.bob, link, now + 1, {
    embeds: (origin) => [
      {
        type: "rich",
        url: link,
        description: "Six seconds of test pattern",
        color: 0x1da1f2,
        author: { name: "Bob (@bob)", url: link },
        footer: { text: "vxTwitter / fixvx" },
        thumbnail: proxied(origin, "post", 1280, 720),
        video: { url: "https://vxtwitter.com/tvid/1", proxy_url: `${origin}/external/post/video.mp4`, width: 480, height: 270 },
      },
    ],
  });
  dispatch("MESSAGE_CREATE", dispatched(linked, origin));
}

// --- Control ----------------------------------------------------------------

let chat: ReturnType<typeof setInterval> | undefined;
let busy: ReturnType<typeof setInterval> | undefined;
let busyCount = 0;
// Channels Alice said she was typing in, oldest first.
const typed: string[] = [];
// Each read acknowledgement's channels, oldest first.
const acks: string[][] = [];
let chatCount = 0;

function chatTick(origin: string) {
  const message = post(GENERAL, users.bob, `Live ${sample(chatCount)}`, Date.now());
  chatCount++;
  dispatch("MESSAGE_CREATE", dispatched(message, origin));
}

/** API requests that sent back the device cookie, and how often it was set. */
const cookieCounts = { set: 0, returned: 0 };

/** Sets a device cookie on the first API answer, as Discord does, and counts
 *  the requests that send it back. */
function withDeviceCookie(req: Request, response: Response) {
  if (/(^|;\s*)__dcfduid=fake-device(;|$)/.test(req.headers.get("cookie") ?? "")) {
    cookieCounts.returned++;
    return response;
  }
  cookieCounts.set++;
  const headers = new Headers(response.headers);
  headers.append("set-cookie", "__dcfduid=fake-device; Max-Age=31536000; Path=/; HttpOnly");
  return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
}

// Private test diagnostics: how much media the UI actually asks for.
const mediaRequests = { avatars: 0, previews: 0 };
// The query of each request for a channel's messages, oldest first.
const historyRequests: string[] = [];
const server = Bun.serve<SocketData>({
  hostname: args.host,
  port: Number(args.port),
  async fetch(req, server) {
    const url = new URL(req.url);
    const path = url.pathname;
    const origin = url.origin;
    if (path === "/_control/media-requests") {
      if (req.method === "POST") Object.assign(mediaRequests, { avatars: 0, previews: 0 });
      return Response.json(mediaRequests);
    }
    if (path === "/_control/history") {
      if (req.method === "POST") historyRequests.length = 0;
      return Response.json(historyRequests);
    }
    if (req.method === "GET") {
      if (/^\/(icons|avatars|banners|embed\/avatars|guilds)\//.test(path)) mediaRequests.avatars++;
      if (/^\/(attachments|media\/attachments|external)\//.test(path)) mediaRequests.previews++;
    }
    // Slowed, so loading states can be seen: history, attachments and links' pictures.
    if (slowMs > 0 && req.method === "GET" && (/\/channels\/\d+\/messages$/.test(path) || path.startsWith("/attachments/") || path.startsWith("/external/"))) {
      await Bun.sleep(slowMs);
    }
    if (path === "/" && req.headers.get("upgrade")?.toLowerCase() === "websocket") {
      if (url.searchParams.get("compress")) {
        return new Response("compression isn't supported", { status: 400 });
      }
      const session = `fake-session-${++sessions}`;
      if (server.upgrade(req, { data: { sequence: 0, identified: false, session, origin } })) {
        return undefined;
      }
      return new Response("upgrade failed", { status: 400 });
    }
    if (path === "/remote-auth/" && req.headers.get("upgrade")?.toLowerCase() === "websocket") {
      if (req.headers.get("origin") !== "https://discord.com") return new Response("bad origin", { status: 403 });
      if (server.upgrade(req, { data: { sequence: 0, identified: false, session: "", origin, remoteAuth: {} } })) {
        return undefined;
      }
      return new Response("upgrade failed", { status: 400 });
    }
    if (path === "/voice/" && req.headers.get("upgrade")?.toLowerCase() === "websocket") {
      if (server.upgrade(req, { data: { sequence: 0, identified: false, session: "", origin, voice: true } })) {
        return undefined;
      }
      return new Response("upgrade failed", { status: 400 });
    }
    if (path.startsWith("/api/v9/")) {
      const response = await api(req, path.slice("/api/v9".length), origin);
      return withDeviceCookie(req, response);
    }
    if (req.method === "GET" && path.startsWith("/attachments/")) {
      return serveFile(path.slice("/attachments/".length), url);
    }
    // The GIF provider's pictures.
    const gif = req.method === "GET" ? path.match(/^\/_gifs\/([a-z]+)\.(webp|gif)$/) : null;
    if (gif) {
      const file = Bun.file(new URL(`gifs/${gif[1]}.${gif[2]}`, fixtures));
      if (!(await file.exists())) return notFound();
      return new Response(file, { headers: { "content-type": `image/${gif[2]}` } });
    }
    if (path === "/_control/gif-selects") return Response.json(gifSelects);
    if (req.method === "GET" && path.startsWith("/media/attachments/")) {
      return serveFile(path.slice("/media/attachments/".length), url);
    }
    // Discord's CDN: server icons, avatars and banners, and those uploaded
    // as they came.
    if (req.method === "GET" && /^\/(icons\/\d+|avatars\/\d+|banners\/\d+|embed\/avatars|guilds\/\d+\/users\/\d+\/(avatars|banners))\/[^/]+\.png$/.test(path)) {
      const uploaded = uploadedPictures.get(path.slice(path.lastIndexOf("/") + 1, -".png".length));
      if (uploaded) return new Response(uploaded.data, { headers: { "content-type": uploaded.type } });
      return new Response(picture(path), { headers: { "content-type": "image/png" } });
    }
    if (req.method === "POST" && path === "/_control/nitro") {
      nitro = !url.searchParams.has("off");
      dispatch("USER_UPDATE", { ...wireUser(users.alice), premium_type: nitro ? 2 : 0 });
      return new Response(nitro ? "Alice has Nitro\n" : "Alice has no Nitro\n");
    }
    if (path === "/_control/server-profile") {
      return Response.json({ nick: memberNicks.get(ALICE) ?? null, ...serverProfile(ALICE) });
    }
    if (req.method === "PUT" && path.startsWith("/_upload/")) {
      const upload = uploads.get(decodeURIComponent(path.slice("/_upload/".length)));
      if (!upload) return notFound();
      upload.data = new Uint8Array(await req.arrayBuffer());
      return new Response(null, { status: 200 });
    }
    if (path === "/_control/health") return new Response("ok\n");
    if (path === "/_control/presence") return Response.json(alicePresence);
    if (req.method === "POST" && path === "/_control/bob-away") {
      // Bob goes idle with a custom status, as his friends hear it.
      presences[users.bob.id] = {
        status: "idle",
        activities: [{ type: 4, name: "Custom Status", state: "brb, making tea", emoji: { name: "🍵" } }],
      };
      dispatch("PRESENCE_UPDATE", { user: { id: users.bob.id }, client_status: {}, ...presences[users.bob.id] });
      return new Response("Bob is away\n");
    }
    if (req.method === "POST" && path === "/_control/bob-mobile") {
      presences[users.bob.id] = { status: "online", activities: [], client_status: { mobile: "online" } };
      dispatch("PRESENCE_UPDATE", { user: { id: users.bob.id }, ...presences[users.bob.id] });
      return new Response("Bob is on his phone\n");
    }
    if (req.method === "POST" && path === "/_control/bob-offline") {
      presences[users.bob.id] = { status: "offline", activities: [] };
      dispatch("PRESENCE_UPDATE", { user: { id: users.bob.id }, client_status: {}, ...presences[users.bob.id] });
      return new Response("Bob is offline\n");
    }
    if (path === "/_control/logouts") return Response.json(logouts);
    if (path === "/_control/voice-identifies") return Response.json(voiceIdentifies);
    if (path === "/_control/voice-changes") return Response.json(voiceChanges);
    if (req.method === "POST" && path === "/_control/voice-reset") {
      // Bob, Carol and Dave back in the Lounge as they started, after a
      // test moved or disconnected them.
      for (const [user, state] of startingVoice()) {
        if (user === users.bob.id) continue;
        const before = voice.get(user);
        voice.set(user, state);
        moved(user, before, origin);
      }
      return new Response("Carol and Dave are back in the Lounge\n");
    }
    if (path === "/_control/call-audio") return Response.json(Object.fromEntries(callAudio));
    if (path === "/_control/call-video") {
      return json({
        cameras: [...callCameras.values()].map(({ key: _, ...camera }) => camera),
        wants: Object.fromEntries(callWants),
        watchers: [...cameraWatchers.values()].map(({ timer: _, resend: _resend, repairs: _repairs, ...watcher }) => watcher),
        repairs: cameraRepairs,
      });
    }
    if (req.method === "POST" && path === "/_control/camera") {
      const query = new URL(req.url).searchParams;
      const name = query.get("who") ?? "bob";
      const filmer = filmers[name];
      const state = filmer && voice.get(filmer.user);
      if (!filmer || !state) return new Response(`${name} isn't in a call\n`, { status: 409 });
      state.self_video = !query.has("off");
      announce(filmer.user);
      const calls = [...voiceSockets].filter((ws) => !ws.data.stream && ws.data.callKey && callChannel(ws) === state.channel);
      for (const ws of calls) {
        if (state.self_video) offerCamera(ws, name, true);
        else withdrawCamera(ws, name);
      }
      return new Response(`${name}'s camera is ${state.self_video ? "on" : "off"} in ${calls.length} call(s)\n`);
    }
    if (req.method === "POST" && path === "/_control/bob-speak") {
      const calls = [...voiceSockets].filter((ws) => !ws.data.stream && ws.data.callKey);
      const spoke = calls.filter((ws) => bobSpeaks(ws)).length;
      return new Response(`Bob speaks in ${spoke} call(s)\n`, { status: spoke ? 200 : 409 });
    }
    if (req.method === "POST" && path === "/_control/voice-link") {
      voiceLink.ping = Number(url.searchParams.get("ping") ?? 0);
      voiceLink.loss = Number(url.searchParams.get("loss") ?? 0);
      return new Response(`heartbeat ACKs ${voiceLink.ping} ms late, ${voiceLink.loss}% of call audio lost\n`);
    }
    if (req.method === "POST" && path === "/_control/voice-drop") {
      const calls = [...voiceSockets].filter((ws) => !ws.data.stream && ws.data.callKey);
      for (const ws of calls) ws.close(4015, "Voice server crashed.");
      return new Response(`dropped ${calls.length} call connection(s)\n`, { status: calls.length ? 200 : 409 });
    }
    if (req.method === "POST" && path === "/_control/voice-move") {
      // Discord moves Alice's call in the server to another voice server: it
      // names the new one, and the old one closes with 4014 once she's there.
      const state = voice.get(ALICE);
      if (!state?.guild) return new Response("Alice isn't in a server's voice channel\n", { status: 409 });
      voiceToken = MOVED_VOICE_TOKEN;
      for (const ws of sockets) {
        if (ws.data.session !== state.session) continue;
        send(ws, 0, { guild_id: GUILD, token: MOVED_VOICE_TOKEN, endpoint: voiceEndpoint(ws) }, "VOICE_SERVER_UPDATE");
      }
      return new Response("moved Alice's call\n");
    }
    if (req.method === "POST" && path === "/_control/elsewhere") {
      // Alice joins the Lounge on another client, such as her phone, or
      // with ?leave leaves it there. Discord gives that client her voice
      // state, and the voice server ends calls of hers with 4014.
      const before = voice.get(ALICE);
      if (url.searchParams.has("leave")) {
        if (before?.session !== OTHER_CLIENT) return new Response("Alice isn't in voice on another client\n", { status: 409 });
        voice.delete(ALICE);
      } else {
        if (before) endStream(streamKey(before.guild, before.channel, ALICE), "user_requested");
        voice.set(ALICE, { guild: GUILD, channel: LOUNGE, self_mute: false, self_deaf: false, session: OTHER_CLIENT });
        for (const ws of voiceSockets) if (!ws.data.stream && ws.data.callKey) ws.close(4014, "Disconnected.");
      }
      moved(ALICE, before, origin);
      return new Response(url.searchParams.has("leave") ? "Alice left on another client\n" : "Alice joined the Lounge on another client\n");
    }
    if (req.method === "POST" && path === "/_control/stream-viewer") {
      // Someone starts watching, or with ?stop stops watching, a stream:
      // ?user=bob watches Alice's, or with &of=bob, Bob's.
      const viewer = users[url.searchParams.get("user") ?? ""];
      const streamer = users[url.searchParams.get("of") ?? "alice"];
      if (!viewer || !streamer) return new Response("name a user and a streamer\n", { status: 400 });
      const found = [...streams.entries()].find(([key]) => key.endsWith(`:${streamer.id}`));
      if (!found) return new Response("no such stream\n", { status: 409 });
      const [key, stream] = found;
      if (url.searchParams.has("stop")) stream.viewers.delete(viewer.id);
      else stream.viewers.add(viewer.id);
      dispatch("STREAM_UPDATE", wireStream(key, stream));
      return new Response(`${[...stream.viewers].length} watching ${key}\n`);
    }
    if (req.method === "POST" && path === "/_control/stream-move") {
      // Discord moves a stream Alice is in to another server, named by a new
      // STREAM_CREATE, then STREAM_SERVER_UPDATE.
      const key = (await req.text()).trim();
      const stream = streams.get(key);
      const state = voice.get(ALICE);
      if (!stream || !state) return new Response("no such stream\n", { status: 409 });
      for (const ws of sockets) {
        if (ws.data.session !== state.session) continue;
        send(ws, 0, { ...wireStream(key, stream), rtc_server_id: "fake-rtc-moved" }, "STREAM_CREATE");
        send(ws, 0, streamServer(ws, key), "STREAM_SERVER_UPDATE");
      }
      return new Response(`moved ${key}\n`);
    }
    if (path === "/_control/sessions") return Response.json(sessionCounts);
    if (path === "/_control/cookies") return Response.json(cookieCounts);
    if (path === "/_control/list-requests") return Response.json(listRequests);
    if (path === "/_control/audit-log") return Response.json(auditLog);
    if (req.method === "POST" && path === "/_control/members-reset") {
      changeMembers(() => {
        const changed = [...memberRoles.keys(), ...memberNicks.keys(), ...timeouts.keys()];
        resetMembers();
        for (const id of new Set(changed)) memberUpdated(userById.get(id)!);
      });
      return new Response("members as they were\n");
    }
    if (req.method === "POST" && path === "/_control/slow") {
      slowMs = Number(await req.text()) || 0;
      return new Response(`history and media take ${slowMs} ms\n`);
    }
    if (req.method === "POST" && path === "/_control/gateway-reconnect") {
      // Discord's op 7: the client should drop the connection and resume.
      for (const ws of sockets) if (ws.data.identified) send(ws, 7, null);
      return new Response("asked gateways to reconnect\n");
    }
    if (req.method === "POST" && path === "/_control/qr-scan") {
      return eachRemoteLogin(async (ws, key) => {
        ws.data.remoteAuth!.scanned = true;
        const payload = `${ALICE}:0:${users.alice.avatar}:${users.alice.username}`;
        ws.send(JSON.stringify({ op: "pending_ticket", encrypted_user_payload: await seal(key, payload) }));
      });
    }
    if (req.method === "POST" && path === "/_control/qr-confirm") {
      return eachRemoteLogin((ws, key) => {
        if (!ws.data.remoteAuth!.scanned) return;
        const ticket = `fake-ticket-${crypto.randomUUID()}`;
        tickets.set(ticket, key);
        ws.send(JSON.stringify({ op: "pending_login", ticket }));
      });
    }
    if (req.method === "POST" && path === "/_control/qr-captcha") {
      ticketCaptcha = true;
      return new Response("the next ticket asks for a captcha\n");
    }
    if (req.method === "POST" && path === "/_control/qr-cancel") {
      return eachRemoteLogin((ws) => {
        ws.send(JSON.stringify({ op: "cancel" }));
        ws.close(1000);
      });
    }
    if (req.method === "POST" && path === "/_control/qr-expire") {
      return eachRemoteLogin((ws) => ws.close(1000));
    }
    if (path === "/_control/voice-audio") {
      return json(Object.fromEntries([...callAudio].map(([ssrc, { speaking, ...call }]) => [ssrc, { ...call, speaking: speaking.at(-1) ?? null, spoke: speaking.includes(1) }])));
    }
    if (path === "/_control/video") {
      return json({
        received: [...received.values()].map(({ key: _, ...stream }) => stream),
        watchers: [...watchers.values()].map(({ timer: _, resend: _resend, repairs: _repairs, ...watcher }) => watcher),
        repairs,
        previews,
      });
    }
    if (req.method === "POST" && path === "/_control/stream-codec") {
      const codec = (await req.text()).trim();
      if (codec !== "vp8" && codec !== "h264") return new Response("vp8 or h264\n", { status: 400 });
      streamCodec = codec;
      return new Response(`streams are ${codec}\n`);
    }
    if (req.method === "POST" && path === "/_control/chat-start") {
      clearInterval(chat);
      chat = setInterval(() => chatTick(origin), CHAT_INTERVAL);
      return new Response("chat started\n");
    }
    if (req.method === "GET" && path.startsWith("/external/")) {
      return serveExternal(path, url) ?? notFound();
    }
    // Discord's CDN: custom emoji and stickers.
    const emoji = req.method === "GET" ? path.match(/^\/(emojis|stickers)\/(\d+)\.(png|gif|webp)$/) : null;
    if (emoji) {
      if (cdnDelay > 0) await Bun.sleep(cdnDelay);
      const size = Math.min(Number(url.searchParams.get("size")) || 128, 512);
      if (emoji[3] === "gif") {
        return new Response(bouncing(size, 10, [124, 196, 255]), { headers: { "content-type": "image/gif" } });
      }
      return new Response(badge(emoji[2]!, size), { headers: { "content-type": "image/png" } });
    }
    if (req.method === "POST" && path === "/_control/showcase") {
      postShowcase(origin);
      return new Response("showcase posted\n");
    }
    if (req.method === "POST" && path === "/_control/clip") {
      postVideo(origin);
      return new Response("video posted\n");
    }
    if (req.method === "POST" && path === "/_control/embeds") {
      postEmbeds(origin);
      return new Response("embeds posted\n");
    }
    if (req.method === "POST" && path === "/_control/gallery") {
      const now = Date.now();
      const shades: [string, number, number, number[]][] = [
        ["dawn.png", 1600, 1000, [240, 160, 90]],
        ["noon.png", 1000, 1600, [90, 170, 230]],
        ["dusk.png", 1200, 1200, [140, 90, 200]],
      ];
      const files = shades.map(([filename, width, height, [r, g, b]], index) => {
        const draw = (w: number, h: number) => png(w, h, (x, y) => [(r! * (w - x)) / w, (g! * (h - y)) / h, b!, 255].map(Math.round));
        return addFile({ id: snowflake(now, 10 + index), channel: GENERAL, filename, contentType: "image/png", data: draw(width, height), width, height, draw });
      });
      const message = post(GENERAL, users.bob, "Three from today", now, { files });
      dispatch("MESSAGE_CREATE", dispatched(message, origin));
      return new Response("gallery posted\n");
    }
    if (req.method === "POST" && path === "/_control/dm") {
      if (!privates.has(DM_DAVE)) {
        const channel: Private = { type: 1, recipients: [users.dave] };
        privates.set(DM_DAVE, channel);
        messages.set(DM_DAVE, []);
        dispatch("CHANNEL_CREATE", wirePrivate(DM_DAVE, channel, "objects"));
      }
      const message = post(DM_DAVE, users.dave, `Hi Alice, it's Dave (${messages.get(DM_DAVE)!.length + 1})`, Date.now());
      dispatch("MESSAGE_CREATE", dispatched(message, origin));
      return new Response("Dave wrote\n");
    }
    if (req.method === "POST" && path === "/_control/call") {
      const channel = url.searchParams.has("group") ? WEEKEND : DM_BOB;
      joinCall(users.bob.id, channel, origin);
      const others = privates.get(channel)!.recipients.map((user) => user.id).filter((user) => user !== users.bob.id);
      ring(channel, [ALICE, ...others], origin);
      return new Response("Bob is calling\n");
    }
    if (req.method === "POST" && path === "/_control/hang-up") {
      for (const [user, state] of [...voice]) {
        if (user === ALICE || state.guild) continue;
        const back = returnTo.get(user);
        returnTo.delete(user);
        if (back) voice.set(user, back);
        else voice.delete(user);
        moved(user, state, origin);
      }
      return new Response("everyone hung up\n");
    }
    if (req.method === "POST" && path === "/_control/hide-from-bots") {
      // The channel in the body is hidden from the Bots role, which gives it
      // a member list of its own; an empty body shows it again.
      const id = await req.text();
      const hidden = { id: BOTS, type: 0, allow: "0", deny: String(bit(10)) };
      for (const channel of guildChannels()) {
        const overwrites = channel.id === id ? [hidden] : [];
        if (JSON.stringify(channel.permission_overwrites) === JSON.stringify(overwrites)) continue;
        channelEdits.set(channel.id, { ...channelEdits.get(channel.id), permission_overwrites: overwrites });
        dispatch("CHANNEL_UPDATE", { ...channel, permission_overwrites: overwrites });
      }
      return new Response("hidden from Bots\n");
    }
    if (req.method === "POST" && path === "/_control/topic") {
      generalTopic = (await req.text()) || null;
      dispatch("CHANNEL_UPDATE", guild().channels.find((channel) => channel.id === GENERAL));
      return new Response("#general's topic changed\n");
    }
    if (req.method === "POST" && path === "/_control/thread-reply") {
      const message = post(plans.id, users.carol, "Bringing the map", Date.now());
      dispatch("MESSAGE_CREATE", dispatched(message, url.origin));
      return Response.json(message.id);
    }
    if (req.method === "POST" && path === "/_control/thread-start") {
      const now = Date.now();
      const thread = makeThread(nextId(now), "Movie night", users.bob, now, [users.bob.id]);
      dispatch("THREAD_CREATE", { ...wireThread(thread), newly_created: true });
      const notice = post(GENERAL, users.bob, thread.name, now, { type: 18, notice: thread.id });
      dispatch("MESSAGE_CREATE", dispatched(notice, url.origin));
      const first = post(thread.id, users.bob, "Friday at 8?", now);
      dispatch("MESSAGE_CREATE", dispatched(first, url.origin));
      return Response.json(thread.id);
    }
    if (req.method === "GET" && path === "/_control/reactor-pages") {
      return Response.json(reactorPages);
    }
    if (req.method === "GET" && path === "/_control/reactor-removals") {
      return Response.json(reactorRemovals);
    }
    if (req.method === "GET" && path === "/_control/thread-searches") {
      return Response.json(threadSearches);
    }
    if (req.method === "GET" && path === "/_control/thread-creates") {
      return Response.json(threadCreates);
    }
    if (req.method === "POST" && path === "/_control/thread-archive") {
      plans.archived = !url.searchParams.has("open");
      plans.archiveChanged = Date.now();
      dispatch("THREAD_UPDATE", wireThread(plans));
      return new Response("ok\n");
    }
    // Plans goes quiet in ?in milliseconds, as if opened again an hour ago
    // with an hour's archive duration, though Discord doesn't archive it;
    // ?reset opens it again for its three days.
    if (req.method === "POST" && path === "/_control/thread-quiet") {
      const reset = url.searchParams.has("reset");
      plans.autoArchive = reset ? 4320 : 60;
      plans.archiveChanged = reset ? Date.now() : Date.now() - 3_600_000 + Number(url.searchParams.get("in") ?? 0);
      dispatch("THREAD_UPDATE", wireThread(plans));
      return new Response("ok\n");
    }
    if (req.method === "POST" && path === "/_control/mention") {
      const message = post(GENERAL, users.bob, `<@${users.alice.id}> are you in for co-op tonight?`, Date.now());
      dispatch("MESSAGE_CREATE", dispatched(message, origin));
      return new Response("Bob mentioned Alice\n");
    }
    if (req.method === "POST" && path === "/_control/mentions") {
      const now = Date.now();
      let alices = messages.get(GENERAL)!.findLast((message) => message.author.id === ALICE);
      if (!alices) {
        alices = post(GENERAL, users.alice, "I'll bring the map", now - 1000);
        dispatch("MESSAGE_CREATE", dispatched(alices, origin));
      }
      const said = [
        `<@${ALICE}> can you check the route?`,
        `<@&${CREW}> meet at the bridge at nine`,
        "@everyone the ferry is cancelled",
        "@here anyone still online?",
        "",
        `<@&${MODS}> please pin the plan`,
        "See you all there",
      ];
      said.forEach((content, index) => {
        const message = post(GENERAL, users.bob, content || "Good idea, bring the big one", now + index, content ? {} : { reply: alices!.id });
        dispatch("MESSAGE_CREATE", dispatched(message, origin));
      });
      return new Response("Bob mentioned Alice\n");
    }
    if (req.method === "POST" && path === "/_control/typing") {
      const dm = url.searchParams.has("dm");
      dispatch("TYPING_START", {
        channel_id: dm ? DM_BOB : GENERAL,
        ...(dm ? {} : { guild_id: GUILD, member: member(users.bob) }),
        user_id: users.bob.id,
        timestamp: Math.floor(Date.now() / 1000),
      });
      return new Response("Bob is typing\n");
    }
    if (req.method === "POST" && path === "/_control/react") {
      const message = messages.get(GENERAL)!.at(-1)!;
      let reaction = message.reactions.find((r) => r.name === "🎉" && r.id === null);
      if (!reaction) message.reactions.push((reaction = { name: "🎉", id: null, users: new Set() }));
      const add = !reaction.users.has(users.carol.id);
      if (add) reaction.users.add(users.carol.id);
      else reaction.users.delete(users.carol.id);
      if (reaction.users.size === 0) message.reactions.splice(message.reactions.indexOf(reaction), 1);
      dispatch(add ? "MESSAGE_REACTION_ADD" : "MESSAGE_REACTION_REMOVE", {
        user_id: users.carol.id,
        channel_id: GENERAL,
        message_id: message.id,
        guild_id: GUILD,
        emoji: { id: null, name: "🎉" },
        burst: false,
        type: 0,
        ...(add ? { member: member(users.carol), message_author_id: message.author.id } : {}),
      });
      return new Response(add ? "Carol reacted\n" : "Carol took her reaction back\n");
    }
    if (req.method === "GET" && path === "/_control/typed") {
      return Response.json(typed);
    }
    if (req.method === "GET" && path === "/_control/acks") {
      return Response.json(acks);
    }
    if (req.method === "GET" && path === "/_control/invites") {
      return Response.json(invites);
    }
    if (req.method === "POST" && path === "/_control/rejoin") {
      if (!leftServer) return new Response("Alice is in Server\n", { status: 400 });
      leftServer = false;
      dispatch("GUILD_CREATE", {
        ...guild(),
        joined_at: new Date().toISOString(),
        members: [member(users.alice)],
        voice_states: [],
        presences: [],
        threads: [],
        stage_instances: [],
        guild_scheduled_events: [],
      });
      return new Response("Alice joined Server again\n");
    }
    if (req.method === "POST" && path === "/_control/busy-start") {
      if (fillerList.length === 0) return new Response("no filler servers; start with --servers N\n", { status: 400 });
      clearInterval(busy);
      busy = setInterval(() => {
        const channel = fillerList[(busyCount * 7919) % fillerList.length]!;
        busyCount++;
        dispatch("MESSAGE_CREATE", dispatched(post(channel, users.bob, `Busy ${busyCount}`, Date.now()), origin));
      }, 100);
      return new Response("busy started\n");
    }
    if (req.method === "POST" && path === "/_control/busy-stop") {
      clearInterval(busy);
      busy = undefined;
      return new Response("busy stopped\n");
    }
    if (req.method === "POST" && path === "/_control/crowd-start") {
      clearInterval(crowd);
      crowd = setInterval(() => {
        const n = crowdCount++;
        const person: User = {
          id: (1_800_000_000_000_000_000n + BigInt(n)).toString(),
          username: `crowd${n}`,
          global_name: `Crowd ${n}`,
          avatar: `c${n}`,
        };
        const message = post(rooms.at(-1) ?? GENERAL, person, `Hello from ${person.global_name}`, Date.now());
        dispatch("MESSAGE_CREATE", dispatched(message, origin));
      }, 20);
      return new Response("crowd started\n");
    }
    if (req.method === "POST" && path === "/_control/crowd-stop") {
      clearInterval(crowd);
      crowd = undefined;
      return new Response(`crowd stopped after ${crowdCount}\n`);
    }
    if (req.method === "POST" && path === "/_control/chat-stop") {
      clearInterval(chat);
      chat = undefined;
      return new Response("chat stopped\n");
    }
    return unknown(req, path);
  },
  websocket: {
    open(ws) {
      if (ws.data.remoteAuth) {
        remoteLogins.add(ws);
        ws.data.remoteAuth.timer = setTimeout(() => ws.close(1000), REMOTE_AUTH_TIMEOUT_MS);
        ws.send(JSON.stringify({ op: "hello", heartbeat_interval: HEARTBEAT_INTERVAL, timeout_ms: REMOTE_AUTH_TIMEOUT_MS }));
        return;
      }
      if (ws.data.voice) {
        voiceSockets.add(ws);
        ws.send(JSON.stringify({ op: 8, d: { heartbeat_interval: 13_750 } }));
        return;
      }
      sockets.add(ws);
      ws.send(
        JSON.stringify({
          op: 10,
          d: { heartbeat_interval: HEARTBEAT_INTERVAL, _trace: ['["fake-gateway",{"micros":0}]'] },
          s: null,
          t: null,
        }),
      );
    },
    message(ws, message) {
      if (ws.data.remoteAuth) {
        const text = typeof message === "string" ? message : new TextDecoder().decode(message);
        remoteAuthMessage(ws, text).catch((error) => {
          console.error(`remote login: ${error}`);
          ws.close(4000, "bad message");
        });
        return;
      }
      if (ws.data.voice) {
        if (typeof message === "string") voiceMessage(ws, message);
        return;
      }
      gatewayMessage(ws, typeof message === "string" ? message : new TextDecoder().decode(message));
    },
    close(ws, code) {
      if (ws.data.remoteAuth) {
        clearTimeout(ws.data.remoteAuth.timer);
        remoteLogins.delete(ws);
        return;
      }
      if (ws.data.voice && ws.data.ssrc) clearInterval(watchers.get(ws.data.ssrc)?.timer);
      if (ws.data.voice && ws.data.ssrc) for (const name of ws.data.cameras ?? []) stopCamera(ws, filmers[name]!.ssrc + 1);
      if (ws.data.voice && ws.data.ssrc && ws.data.callKey) resumedKeys.set(ws.data.ssrc, ws.data.callKey);
      voiceSockets.delete(ws);
      sockets.delete(ws);
      if (!ws.data.voice && !ws.data.stream && ws.data.identified && code !== 1000 && code !== 1001) {
        parked.set(ws.data.session, { sequence: ws.data.sequence, missed: [] });
      }
    },
  },
});

console.error(
  `fake Discord on http://${server.hostname}:${server.port} ` +
    `(${general.length} messages in #general ${GENERAL}, server ${GUILD})`,
);
