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
// drops the audio. A Go Live stream's video is decrypted and its VP8 frames
// counted; GET /_control/video lists what each stream received. POST
// /_control/voice-move moves Alice's call in the server to another voice
// server, and GET /_control/voice-identifies lists the tokens voice
// connections identified with. Joining "Create Room" has a bot move Alice to
// "Alice's room", made the first time, on another voice server; POST /_control/stream-move with a stream key
// moves that stream to another server. Watching
// Bob's stream gets testing/fixtures' recording once the viewer asks for video
// with op 15: VP8 or, after POST /_control/stream-codec with "h264", H264.
// Alice has a DM with Bob, whose last two messages she hasn't read, and a
// group DM, "Weekend plans", with Bob and Carol. POST /_control/dm makes Dave
// write to her, opening a DM with her the first time, and POST
// /_control/mention makes Bob mention her in #general. Erin, who shares no
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
// a time, and GET /guilds/{id}/messages/search?channel_id=... a server
// channel's; the first search of each DM or server answers 202, as
// Discord's does while it indexes it.
// POST /_control/typing makes Bob start typing in #general, or with ?dm in
// his DM; GET /_control/typed lists the channels Alice said she was typing in.
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
// server and its channels, and USER_GUILD_SETTINGS_UPDATE says so.
// Linking by QR code goes through a remote login gateway at /remote-auth/, as
// remote-auth-gateway.discord.gg speaks it. POST /_control/qr-scan scans the
// code shown as Alice, /_control/qr-confirm confirms it, /_control/qr-cancel
// says no, and /_control/qr-expire ends the code as the real one does after
// two minutes. Start Talk with TALK_DISCORD_TEST_UNLINKED=1 to link. POST
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
// category's override. PATCH /users/@me/guilds/{id}/settings changes them.
type MuteConfig = { end_time: string | null; selected_time_window: number };
type ChannelOverride = { muted: boolean; mute_config: MuteConfig | null; message_notifications: number; collapsed: boolean };
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
// View, send, embed, attach, read history, react, connect, speak, voice activity.
const EVERYONE = [10, 11, 14, 15, 16, 6, 20, 21, 25].reduce((sum, n) => sum + bit(n), 0);

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

// Bob is a Mod everywhere his member object goes, so his name takes its colour.
function member(user: User, roles: string[] = user.id === users.bob.id ? [MODS] : user.id === ALICE ? [CREW] : []) {
  return {
    user: wireUser(user),
    user_id: user.id,
    roles,
    nick: null,
    avatar: user.id === users.carol.id ? "carol-server" : null,
    joined_at: JOINED_AT,
    premium_since: null,
    deaf: false,
    mute: false,
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
  const profile = {
    bio: bob ? "Plays bass in a **ska** band. Ask me about https://example.com/gigs" : "",
    pronouns: bob ? "he/him" : "",
    banner: bob ? "bob-banner" : null,
    accent_color: carol ? 0x5865f2 : null,
  };
  return {
    user: { ...wireUser(user), banner: profile.banner, accent_color: profile.accent_color, bio: profile.bio },
    user_profile: profile,
    ...(guild === GUILD && inServer
      ? {
          guild_member_profile: { guild_id: GUILD, bio: bob ? "Keeps #general tidy." : "", pronouns: "", banner: null, accent_color: null },
          guild_member: member(user),
        }
      : {}),
    mutual_guilds: inServer ? [{ id: GUILD, nick: null }] : [],
    mutual_friends: bob ? [wireUser(users.carol)] : [],
    badges: [],
    premium_type: null,
  };
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
    flags: 0,
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
  else post(GENERAL, users.bob, sample(index), ms, { files });
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
// DMs searched once, which Discord has indexed.
const indexed = new Set<string>();
const readBob = post(DM_BOB, users.alice, "Yes, bringing snacks", DM_START + 180_000);
post(DM_BOB, users.bob, "Great", DM_START + 240_000);
post(DM_BOB, users.bob, "See you at nine", DM_START + 300_000);
privates.set(REQUEST_ERIN, { type: 1, recipients: [strangers.erin!], request: true });
privates.set(SPAM_FRANK, { type: 1, recipients: [strangers.frank!], request: true, spam: true });
post(SPAM_FRANK, strangers.frank!, "Claim your free Nitro at nitro-gift.example", DM_START + 360_000);
post(REQUEST_ERIN, strangers.erin!, "Hi Alice, Bob gave me your name. Can I ask about the trip?", DM_START + 420_000);

// Alice has read all but the last `--unread` messages of the fixture, and Bob's
// last two in their DM, which count as mentions as in Discord.
const general = messages.get(GENERAL)!;
const readStates = new Map<string, { last: string; mentions: number }>([
  [GENERAL, { last: general[Math.max(0, general.length - 1 - Number(args.unread))]!.id, mentions: 0 }],
  [DM_BOB, { last: readBob.id, mentions: 2 }],
  [REQUEST_ERIN, { last: readBob.id, mentions: 1 }],
  [SPAM_FRANK, { last: readBob.id, mentions: 1 }],
]);

// --- Voice ------------------------------------------------------------------

// Bob shares his screen with Go Live.
// A voice state's guild is null in a DM or group DM call.
type VoiceState = { guild: string | null; channel: string; self_mute: boolean; self_deaf: boolean; session: string; self_stream?: boolean };
const voice = new Map<string, VoiceState>([
  [users.bob.id, { guild: GUILD, channel: LOUNGE, self_mute: false, self_deaf: false, session: "voice-bob", self_stream: true }],
  [users.carol.id, { guild: GUILD, channel: LOUNGE, self_mute: true, self_deaf: false, session: "voice-carol" }],
  [users.dave.id, { guild: GUILD, channel: LOUNGE, self_mute: true, self_deaf: true, session: "voice-dave" }],
]);

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
    self_video: false,
    self_stream: state?.self_stream ?? false,
    mute: false,
    deaf: false,
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

function guild() {
  const channel = (id: string, name: string, type: number, position: number, parent: string | null) => ({
    id,
    type,
    name,
    position,
    parent_id: parent,
    guild_id: GUILD,
    flags: 0,
    permission_overwrites: [],
    ...(type === 0
      ? { topic: null, nsfw: false, rate_limit_per_user: 0, last_message_id: messages.get(id)!.at(-1)?.id ?? null }
      : {}),
    ...(type === 2 ? { bitrate: 64000, user_limit: 0, rtc_region: null, last_message_id: null } : {}),
  });
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
    channels: [
      channel(CATEGORY, "Channels", 4, 0, null),
      channel(GENERAL, "general", 0, 0, CATEGORY),
      channel(LOUNGE, "Lounge", 2, 1, CATEGORY),
      channel(CREATE_ROOM, "Create Room", 2, 2, CATEGORY),
      ...(roomMade ? [channel(ALICES_ROOM, "Alice's room", 2, 3, CATEGORY)] : []),
      ...rooms.map((id, n) => channel(id, `room-${n + 1}`, 0, 2 + n, CATEGORY)),
    ],
    roles: [
      { id: GUILD, name: "@everyone", permissions: String(EVERYONE), position: 0, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
      { id: MODS, name: "Mods", permissions: String(bit(13)), position: 1, color: 3447003, hoist: true, managed: false, mentionable: true, flags: 0 },
      { id: BOTS, name: "Bots", permissions: "0", position: 2, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
      { id: CREW, name: "Crew", permissions: "0", position: 3, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
    ],
    emojis: [
      { id: EMOJI_TALK, name: "talk", animated: false, available: true, managed: false, require_colons: true, roles: [] },
      { id: EMOJI_PARTY, name: "party", animated: true, available: true, managed: false, require_colons: true, roles: [] },
    ],
    stickers: [],
    threads: [],
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
      premium_type: args.nitro ? 2 : 0,
      flags: 0,
      bot: false,
      phone: null,
      locale: "en-US",
      nsfw_allowed: true,
    },
    users: [...Object.values(users), ...Object.values(strangers), ...fillerPeople].filter((user) => user.id !== ALICE).map(wireUser),
    guilds: [guild(), ...fillers],
    merged_members: [[alice], ...fillers.map((filler) => [{ ...alice, roles: [filler.roles[1]!.id, filler.roles[4]!.id] }])],
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
function memberList() {
  const offline = (user: User) => presences[user.id]!.status === "offline";
  const mods = [users.bob];
  const others = Object.values(users).filter((user) => !mods.includes(user));
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
    { group: groups[0] },
    ...mods.map(row),
    { group: groups[1] },
    ...others.filter((user) => !offline(user)).map(row),
    { group: groups[2] },
    ...others.filter(offline).map(row),
  ];
  return {
    guild_id: GUILD,
    id: "everyone",
    member_count: Object.keys(users).length,
    online_count: groups[0]!.count + groups[1]!.count,
    groups,
    ops: [{ op: "SYNC", range: [0, 99], items }],
  };
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
        });
      } else {
        voice.delete(ALICE);
      }
      moved(ALICE, before, ws.data.origin);
      if (channel && before?.channel !== channel) {
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
      // list is sent when asked for.
      for (const [guildId, subscription] of Object.entries<any>(d.subscriptions ?? {})) {
        if (guildId === GUILD && Object.keys(subscription.channels ?? {}).length > 0) {
          listRequests++;
          send(ws, 0, memberList(), "GUILD_MEMBER_LIST_UPDATE");
        }
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

const recordings = {
  vp8: ivf(readFileSync(new URL("stream-vp8.ivf", fixtures))),
  h264: annexB(readFileSync(new URL("stream-h264.h264", fixtures))),
};
let streamCodec: "vp8" | "h264" = "vp8";

/** A viewer of Bob's stream and what it was sent. */
type Watcher = {
  stream: string; codec: string; framesSent: number; soundsSent: number; keyframeRequests: number; timer?: Timer;
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

/** Streams the recording to a viewer at 10 frames a second until it disconnects. */
function streamTo(ws: Socket, key: Buffer, ssrc: number) {
  const watcher = watchers.get(ws.data.ssrc!)!;
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
    if (number === 1003) return void repairs.dropped++;
    if (number === 1008) return void (overtaken = sealed);
    toViewer(sealed);
    if (overtaken) {
      toViewer(overtaken);
      overtaken = undefined;
      repairs.reordered++;
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
    repairs.resent++;
  };
  // A viewer that went away refuses packets until its connection closes.
  const toViewer = (packet: Buffer) => {
    const peer = peers.get(ws.data.ssrc!);
    try {
      if (peer) udp.send(packet, peer.port, peer.address);
    } catch {}
  };
  watcher.timer = setInterval(() => {
    const timestamp = frame * 9000;
    if (watcher.codec === "vp8") {
      const { data } = recordings.vp8[frame % recordings.vp8.length]!;
      for (let at = 0; at < data.length; at += 1100) {
        const descriptor = Buffer.from([at === 0 ? 0x90 : 0x80, 0x80, 0x80 | (picture >> 8), picture & 0xff]);
        send(Buffer.concat([descriptor, data.subarray(at, at + 1100)]), at + 1100 >= data.length, timestamp, 105);
      }
      picture = (picture + 1) & 0x7fff;
    } else {
      const { units } = recordings.h264[frame % recordings.h264.length]!;
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
    // Opus frames of silence, plain RTP as a call's audio goes.
    for (let index = 0; index < 5; index++) {
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
  }, 100);
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

/** Counts a video packet: frames end with the marker bit; keyframes carry their size. */
function receiveVideo(packet: Buffer) {
  const stream = received.get(packet.readUInt32BE(8));
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
          if (sound) receiveAudio(sound, data);
          else if (resent) receiveRtx(resent, data);
          else receiveVideo(data);
        }
        // A streamer's RTCP sender report, from its video or audio SSRC.
        if (data.length >= 8 && data[1] === 200) {
          const sender = data.readUInt32BE(4);
          const stream = received.get(sender) ?? receivedAudio.get(sender);
          const report = stream && openRtp(data, stream.key, true);
          if (stream && report && report.length >= 28) {
            if (received.has(sender)) {
              stream.senderReports++;
              stream.reportedPackets = report.readUInt32BE(20);
            } else {
              stream.audioSenderReports++;
            }
          }
        }
        // RTCP feedback with a picture loss indication, from a viewer's SSRC.
        if (data.length >= 8 && data[1] === 206 && (data[0]! & 0x1f) === 1) {
          const watcher = watchers.get(data.readUInt32BE(4));
          if (watcher) watcher.keyframeRequests++;
        }
        // A viewer's generic NACK: the packets it asks for again.
        const asking = data.length >= 8 && data[1] === 205 && (data[0]! & 0x1f) === 1 ? watchers.get(data.readUInt32BE(4)) : undefined;
        const viewer = asking && viewerKeys.get(data.readUInt32BE(4));
        const nack = asking && viewer && openRtp(data, viewer, true);
        if (asking && nack) {
          repairs.nacks++;
          for (let at = 12; at + 4 <= nack.length; at += 4) {
            const first = nack.readUInt16BE(at);
            const mask = nack.readUInt16BE(at + 2);
            asking.resend?.(first);
            for (let bit = 0; bit < 16; bit++) if (mask & (1 << bit)) asking.resend?.((first + bit + 1) & 0xffff);
          }
        }
        if (data.length >= 8 && data[1] === 201 && watchers.has(data.readUInt32BE(4))) repairs.receiverReports++;
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
        watchers.set(ws.data.ssrc, { stream: ws.data.stream!, codec, framesSent: 0, soundsSent: 0, keyframeRequests: 0 });
        viewerKeys.set(ws.data.ssrc, key);
        ws.data.waiting = { key, ssrc: bob + 1 };
      }
      break;
    }
    case 15: { // Media sink wants: Discord forwards video only to those who ask.
      const waiting = ws.data.waiting;
      if (waiting && Number(d.any) > 0) {
        ws.data.waiting = undefined;
        streamTo(ws, waiting.key, waiting.ssrc);
      }
      break;
    }
    case 5: { // Speaking: a stream's sound is marked as soundshare.
      const stream = ws.data.ssrc ? receivedAudio.get(ws.data.ssrc) : undefined;
      if (stream) stream.speaking = Number(d.speaking);
      break;
    }
    case 12: { // Video attributes
      const stream = received.get(d.video_ssrc);
      if (stream) stream.announced = d.streams?.[0]?.max_resolution ?? null;
      break;
    }
    case 3: // Heartbeat
      reply(6, d);
      break;
    default: // Speaking and client details need no answer.
      break;
  }
}

// --- HTTP -------------------------------------------------------------------

const json = (body: unknown, status = 200) => Response.json(body, { status });
const empty = () => new Response(null, { status: 204 });
const notFound = (message = "404: Not Found", code = 0) => json({ message, code }, 404);

/** Files uploaded for a message that isn't sent yet, by uploaded filename. */
const uploads = new Map<string, { filename: string; data?: Uint8Array }>();
let uploadCount = 0;

async function api(req: Request, path: string, origin: string): Promise<Response> {
  // A confirmed remote login's ticket, traded for the login without one.
  if (req.method === "POST" && path === "/users/@me/remote-auth/login") {
    const body: any = await req.json().catch(() => ({}));
    const key = tickets.get(String(body.ticket));
    if (!key) return json({ message: "Invalid ticket", code: 50035 }, 400);
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

  if (method === "GET" && (m = path.match(/^\/users\/(\d+)\/profile$/))) {
    const user = userById.get(m[1]!);
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
      let page = list;
      if (before) page = page.filter((message) => BigInt(message.id) < BigInt(before));
      if (after) page = page.filter((message) => BigInt(message.id) > BigInt(after)).slice(0, limit);
      page = page.slice(-limit).reverse();
      return json(page.map((message) => wireMessage(message, origin)));
    }
    if (method === "POST") {
      const body: any = await req.json();
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
    const content = (url.searchParams.get("content") ?? "").toLowerCase();
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const found = list.filter((message) => message.content.toLowerCase().includes(content)).reverse();
    return json({
      analytics_id: "fake",
      doing_deep_historical_index: false,
      total_results: found.length,
      messages: found.slice(offset, offset + 25).map((message) => [wireMessage(message, origin)]),
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
    const content = (url.searchParams.get("content") ?? "").toLowerCase();
    const offset = Number(url.searchParams.get("offset") ?? 0);
    const found = (asked.length ? asked : channels)
      .flatMap((id) => messages.get(id)!)
      .filter((message) => (message.type === 0 || message.type === 19) && message.content.toLowerCase().includes(content))
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
  if (method === "PATCH" && (m = path.match(/^\/users\/@me\/guilds\/(\d+|@me)\/settings$/))) {
    const guild = m[1]!;
    if (guild !== "@me" && guild !== GUILD && !fillers.some((filler) => filler.id === guild)) {
      return notFound("Unknown Guild", 10004);
    }
    const body: any = await req.json().catch(() => ({}));
    const settings = settingsOf(guild);
    for (const key of ["muted", "message_notifications", "suppress_everyone", "suppress_roles", "hide_muted_channels"] as const) {
      if (key in body) (settings as any)[key] = body[key];
    }
    if ("mute_config" in body) settings.mute_config = body.mute_config;
    if (!settings.muted) settings.mute_config = null;
    for (const [channel, change] of Object.entries<any>(body.channel_overrides ?? {})) {
      const override = settings.overrides.get(channel) ?? { muted: false, mute_config: null, message_notifications: 3, collapsed: false };
      for (const key of ["muted", "mute_config", "message_notifications", "collapsed"] as const) {
        if (key in change) (override as any)[key] = change[key];
      }
      if (!override.muted) override.mute_config = null;
      settings.overrides.set(channel, override);
    }
    const wire = wireGuildSettings(guild);
    dispatch("USER_GUILD_SETTINGS_UPDATE", wire);
    return json(wire);
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
// /external/post/video.mp4.
function serveExternal(path: string, url: URL): Response | undefined {
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
    if (req.method === "GET" && path.startsWith("/media/attachments/")) {
      return serveFile(path.slice("/media/attachments/".length), url);
    }
    // Discord's CDN: server icons, avatars and banners.
    if (req.method === "GET" && /^\/(icons\/\d+|avatars\/\d+|banners\/\d+|embed\/avatars|guilds\/\d+\/users\/\d+\/(avatars|banners))\/[^/]+\.png$/.test(path)) {
      return new Response(picture(path), { headers: { "content-type": "image/png" } });
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
    if (req.method === "POST" && path === "/_control/qr-cancel") {
      return eachRemoteLogin((ws) => {
        ws.send(JSON.stringify({ op: "cancel" }));
        ws.close(1000);
      });
    }
    if (req.method === "POST" && path === "/_control/qr-expire") {
      return eachRemoteLogin((ws) => ws.close(1000));
    }
    if (path === "/_control/video") {
      return json({
        received: [...received.values()].map(({ key: _, ...stream }) => stream),
        watchers: [...watchers.values()].map(({ timer: _, resend: _resend, ...watcher }) => watcher),
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
