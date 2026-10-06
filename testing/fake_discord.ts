// A fake Discord for tests and memory measurements: the gateway and the parts
// of the HTTP API Talk uses, with one deterministic server, "Server", that
// mirrors the Matrix fixture (Alice reads #general, Bob posts, three people sit
// in the Lounge voice channel). Run `bun testing/fake_discord.ts --port 18400`,
// then start Talk with TALK_DISCORD_TEST_SERVER=http://127.0.0.1:18400.
// `--extra N` sets how many history messages follow the four fixture ones
// (default 56, 60 in all). POST /_control/chat-start makes Bob post in
// #general every 2 s; POST /_control/chat-stop stops him. POST
// /_control/gallery makes him post three photos in one message. Joining voice
// connects to a voice server here that answers the handshake without DAVE and
// drops the audio. A Go Live stream's video is decrypted and its VP8 frames
// counted; GET /_control/video lists what each stream received. Watching
// Bob's stream gets testing/fixtures' recording once the viewer asks for video
// with op 15: VP8 or, after POST /_control/stream-codec with "h264", H264.

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
const MODS = "1400000000000000110";
const ALICE = "1400000000000000001";

// Dave has no avatar, so he gets a default one; Carol set another for the server.
type User = { id: string; username: string; global_name: string; avatar: string | null };
const users: Record<string, User> = {
  alice: { id: ALICE, username: "alice", global_name: "Alice", avatar: "alice1" },
  bob: { id: "1400000000000000002", username: "bob", global_name: "Bob", avatar: "bob1" },
  carol: { id: "1400000000000000003", username: "carol", global_name: "Carol", avatar: "carol1" },
  dave: { id: "1400000000000000004", username: "dave", global_name: "Dave", avatar: null },
};
const userById = new Map(Object.values(users).map((user) => [user.id, user]));

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

function member(user: User, roles: string[] = []) {
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
function picture(path: string): Uint8Array {
  const hue = crc32(new TextEncoder().encode(path));
  const light = [(hue >>> 16) & 255, (hue >>> 8) & 255, hue & 255].map((c) => 96 + (c % 160));
  const dark = light.map((c) => c >> 1);
  return png(128, 128, (x, y) => [...(x + y < 128 ? light : dark), 255]);
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
};
const messages = new Map<string, Message[]>([
  [GENERAL, []],
  [LOUNGE, []],
]);
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
  const wire: Record<string, unknown> = {
    id: message.id,
    type: message.type,
    content: message.content,
    channel_id: message.channel,
    author: wireUser(message.author),
    attachments: message.files.map((file) => wireAttachment(file, origin)),
    embeds: [],
    mentions,
    mention_roles: [],
    pinned: false,
    mention_everyone: false,
    tts: false,
    timestamp: iso(timeOf(message.id)),
    edited_timestamp: message.edited ? iso(message.edited) : null,
    flags: 0,
    components: [],
  };
  if (message.nonce) wire.nonce = message.nonce;
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
      guild_id: GUILD,
    };
    wire.referenced_message = original ? wireMessage(original, origin, true) : null;
  }
  return wire;
}

/** A message as the gateway dispatches it: with the server and the author's member. */
function dispatched(message: Message, origin: string) {
  const { user: _, ...profile } = member(message.author);
  return { ...wireMessage(message, origin), guild_id: GUILD, member: profile };
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
for (let index = 0; index < extra; index++) {
  post(GENERAL, users.bob, sample(index), HISTORY_START + (4 + index) * 60_000);
}

// Alice has read all but the last five messages of the fixture.
const general = messages.get(GENERAL)!;
const readStates = new Map<string, { last: string; mentions: number }>([
  [GENERAL, { last: general[Math.max(0, general.length - 6)]!.id, mentions: 0 }],
]);

// --- Voice ------------------------------------------------------------------

// Bob shares his screen with Go Live.
type VoiceState = { channel: string; self_mute: boolean; self_deaf: boolean; session: string; self_stream?: boolean };
const voice = new Map<string, VoiceState>([
  [users.bob.id, { channel: LOUNGE, self_mute: false, self_deaf: false, session: "voice-bob", self_stream: true }],
  [users.carol.id, { channel: LOUNGE, self_mute: true, self_deaf: false, session: "voice-carol" }],
  [users.dave.id, { channel: LOUNGE, self_mute: true, self_deaf: true, session: "voice-dave" }],
]);

// Go Live streams by stream key; Bob's is running from the start.
type Stream = { viewers: Set<string>; paused: boolean };
const streamKey = (channel: string, user: string) => `guild:${GUILD}:${channel}:${user}`;
const streams = new Map<string, Stream>([[streamKey(LOUNGE, users.bob.id), { viewers: new Set(), paused: false }]]);

function wireStream(key: string, stream: Stream) {
  return { stream_key: key, rtc_server_id: "fake-rtc", region: "fake", viewer_ids: [...stream.viewers], paused: stream.paused };
}

function wireVoiceState(userId: string, state: VoiceState | undefined) {
  return {
    guild_id: GUILD,
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
  /** A stream connection: its key, from the token it identified with. */
  stream?: string;
  ssrc?: number;
  /** A viewer's stream, which starts once it asks for video with op 15. */
  waiting?: { key: Buffer; ssrc: number };
};
type Socket = ServerWebSocket<SocketData>;
const sockets = new Set<Socket>();
let sessions = 0;

function send(ws: Socket, op: number, d: unknown, t: string | null = null) {
  const s = op === 0 ? ++ws.data.sequence : null;
  ws.send(JSON.stringify({ op, d, s, t }));
}

function dispatch(name: string, data: unknown) {
  for (const ws of sockets) if (ws.data.identified) send(ws, 0, data, name);
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
    ],
    roles: [
      { id: GUILD, name: "@everyone", permissions: String(EVERYONE), position: 0, color: 0, hoist: false, managed: false, mentionable: false, flags: 0 },
      { id: MODS, name: "Mods", permissions: String(bit(13)), position: 1, color: 3447003, hoist: true, managed: false, mentionable: true, flags: 0 },
    ],
    emojis: [],
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

function ready(ws: Socket) {
  const { user: _, ...alice } = member(users.alice);
  return {
    v: 9,
    user: {
      ...wireUser(users.alice),
      email: "alice@example.org",
      verified: true,
      mfa_enabled: false,
      premium_type: 0,
      flags: 0,
      bot: false,
      phone: null,
      locale: "en-US",
      nsfw_allowed: true,
    },
    users: Object.values(users).filter((user) => user.id !== ALICE).map(wireUser),
    guilds: [guild()],
    merged_members: [[alice]],
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
        {
          guild_id: GUILD,
          muted: false,
          mute_config: null,
          suppress_everyone: false,
          suppress_roles: false,
          message_notifications: 1,
          mobile_push: true,
          hide_muted_channels: false,
          notify_highlights: 0,
          flags: 0,
          version: 1,
          channel_overrides: [],
        },
      ],
      partial: false,
      version: 1,
    },
    private_channels: [],
    relationships: [],
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
    user_settings_proto: "",
    notification_settings: { flags: 0 },
    consents: { personalization: { consented: false } },
    tutorial: null,
    auth: {},
  };
}

function readySupplemental() {
  const inVoice = [...voice.keys()];
  return {
    guilds: [
      {
        id: GUILD,
        voice_states: [...voice].map(([userId, state]) => {
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
    merged_presences: { guilds: [[]], friends: [] },
    lazy_private_channels: [],
    disclose: [],
    game_invites: [],
  };
}

// Who is around and what they're doing, as their presence says.
const presences: Record<string, { status: string; activities: unknown[] }> = {
  [users.alice.id]: { status: "online", activities: [] },
  [users.bob.id]: { status: "dnd", activities: [{ type: 0, name: "Factorio" }] },
  [users.carol.id]: {
    status: "idle",
    activities: [{ type: 4, name: "Custom Status", state: "back later", emoji: { name: "🌙", id: null } }],
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
      ...member(user, mods.includes(user) ? [MODS] : []),
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
      send(ws, 0, ready(ws), "READY");
      send(ws, 0, readySupplemental(), "READY_SUPPLEMENTAL");
      break;
    case 3: // Presence
      break;
    case 4: {
      // Voice state: Alice shows in the channel; no voice server is assigned.
      const channel: string | null = d.channel_id ?? null;
      const before = voice.get(ALICE);
      if (before && before.channel !== channel) endStream(streamKey(before.channel, ALICE), "user_requested");
      if (channel) {
        voice.set(ALICE, {
          channel,
          self_mute: !!d.self_mute,
          self_deaf: !!d.self_deaf,
          session: ws.data.session,
          self_stream: before?.channel === channel && before.self_stream,
        });
      } else {
        voice.delete(ALICE);
      }
      dispatch("VOICE_STATE_UPDATE", { ...wireVoiceState(ALICE, voice.get(ALICE)), member: member(users.alice) });
      if (channel && before?.channel !== channel) {
        send(ws, 0, { guild_id: GUILD, token: VOICE_TOKEN, endpoint: voiceEndpoint(ws) }, "VOICE_SERVER_UPDATE");
      }
      break;
    }
    case 18: {
      // Go Live: only into the voice channel Alice is in.
      const key = streamKey(d.channel_id, ALICE);
      const state = voice.get(ALICE);
      if (d.guild_id !== GUILD || state?.channel !== d.channel_id) {
        send(ws, 0, { stream_key: key, unavailable: false, reason: "unauthorized" }, "STREAM_DELETE");
        break;
      }
      // A new stream starts paused until its streamer unpauses it with op 22.
      const stream = { viewers: new Set<string>(), paused: true };
      streams.set(key, stream);
      state.self_stream = true;
      dispatch("VOICE_STATE_UPDATE", { ...wireVoiceState(ALICE, state), member: member(users.alice) });
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
      const [, , channel] = key.split(":");
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
      // Request guild members
      const ids: string[] = d.user_ids ?? [];
      const found = ids.map((id) => userById.get(id)).filter((user) => user !== undefined);
      dispatch("GUILD_MEMBERS_CHUNK", {
        guild_id: GUILD,
        members: found.map((user) => member(user)),
        chunk_index: 0,
        chunk_count: 1,
        not_found: ids.filter((id) => !userById.has(id)),
      });
      break;
    }
    case 37: {
      // Guild subscriptions: messages are sent anyway; a channel's member
      // list is sent when asked for.
      for (const [guildId, subscription] of Object.entries<any>(d.subscriptions ?? {})) {
        if (guildId === GUILD && Object.keys(subscription.channels ?? {}).length > 0) {
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
  const user = key.split(":")[3]!;
  const state = voice.get(user);
  if (state) {
    state.self_stream = false;
    dispatch("VOICE_STATE_UPDATE", { ...wireVoiceState(user, state), member: member(userById.get(user)!) });
  }
  dispatch("STREAM_DELETE", { stream_key: key, unavailable: false, reason });
}

// --- Voice server -------------------------------------------------------------
// Discord's voice gateway, version 8, and UDP address discovery. The session
// has no DAVE and its audio is dropped, which is enough for Talk to connect.

const VOICE_TOKEN = "fake-voice-token";
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
};
const received = new Map<number, Received>();
/** The same records, by the stream's audio SSRC. */
const receivedAudio = new Map<number, Received>();

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
type Watcher = { stream: string; codec: string; framesSent: number; keyframeRequests: number; timer?: Timer };
const watchers = new Map<number, Watcher>();
/** Where each voice connection's UDP comes from, by its SSRC. */
const peers = new Map<number, { address: string; port: number }>();
const PLAYOUT_DELAY = Buffer.from([0xbe, 0xde, 0x00, 0x01, 0x52, 0x00, 0x00, 0x00]);

/** Seals an RTP packet as "rtpsize" AES-GCM: header and extension preamble in the clear. */
function sealRtp(packet: Buffer, key: Buffer, counter: number): Buffer {
  const clear = 16;
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
  let counter = 1;
  let picture = 0;
  const send = (payload: Buffer, marker: boolean, timestamp: number, payloadType: number) => {
    const header = Buffer.alloc(12);
    header[0] = 0x90;
    header[1] = (marker ? 0x80 : 0) | payloadType;
    header.writeUInt16BE(sequence++ & 0xffff, 2);
    header.writeUInt32BE(timestamp >>> 0, 4);
    header.writeUInt32BE(ssrc, 8);
    const peer = peers.get(ws.data.ssrc!);
    if (peer) udp.send(sealRtp(Buffer.concat([header, PLAYOUT_DELAY, payload]), key, counter++), peer.port, peer.address);
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
  const transportSequence = extensions.get(5);
  if (extensions.get(11)?.toString() !== "100" || (transportSequence && transportSequence.length !== 2)) {
    stream.unattached++;
    return;
  }
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
          const sound = receivedAudio.get(data.readUInt32BE(8));
          if (sound) receiveAudio(sound, data);
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
      if (d.token !== VOICE_TOKEN && !String(d.token).startsWith("stream-token-")) {
        ws.close(4004, "Authentication failed.");
        return;
      }
      ws.data.ssrc = ssrcs += 3;
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
        };
        received.set(ws.data.ssrc + 1, record);
        receivedAudio.set(ws.data.ssrc, record);
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
        watchers.set(ws.data.ssrc, { stream: ws.data.stream!, codec, framesSent: 0, keyframeRequests: 0 });
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
  if (req.headers.get("authorization") !== TOKEN) {
    return json({ message: "401: Unauthorized", code: 0 }, 401);
  }
  const url = new URL(req.url);
  const method = req.method;
  let m: RegExpMatchArray | null;

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
      dispatch("MESSAGE_DELETE", { id: message.id, channel_id: message.channel, guild_id: GUILD });
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
      guild_id: GUILD,
      emoji: { id, name },
      burst: false,
      type: 0,
      ...(add ? { member: member(users.alice), message_author_id: message.author.id } : {}),
    });
    return empty();
  }

  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/messages\/(\d+)\/ack$/))) {
    if (!messages.has(m[1]!)) return notFound("Unknown Channel", 10003);
    readStates.set(m[1]!, { last: m[2]!, mentions: 0 });
    dispatch("MESSAGE_ACK", { channel_id: m[1], message_id: m[2], version: 1, mention_count: 0 });
    return json({ token: null });
  }

  if (method === "POST" && (m = path.match(/^\/channels\/(\d+)\/typing$/))) {
    return messages.has(m[1]!) ? empty() : notFound("Unknown Channel", 10003);
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

// --- Control ----------------------------------------------------------------

let chat: ReturnType<typeof setInterval> | undefined;
let chatCount = 0;

function chatTick(origin: string) {
  const message = post(GENERAL, users.bob, `Live ${sample(chatCount)}`, Date.now());
  chatCount++;
  dispatch("MESSAGE_CREATE", dispatched(message, origin));
}

const server = Bun.serve<SocketData>({
  hostname: args.host,
  port: Number(args.port),
  async fetch(req, server) {
    const url = new URL(req.url);
    const path = url.pathname;
    const origin = url.origin;
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
    if (path === "/voice/" && req.headers.get("upgrade")?.toLowerCase() === "websocket") {
      if (server.upgrade(req, { data: { sequence: 0, identified: false, session: "", origin, voice: true } })) {
        return undefined;
      }
      return new Response("upgrade failed", { status: 400 });
    }
    if (path.startsWith("/api/v9/")) return api(req, path.slice("/api/v9".length), origin);
    if (req.method === "GET" && path.startsWith("/attachments/")) {
      return serveFile(path.slice("/attachments/".length), url);
    }
    if (req.method === "GET" && path.startsWith("/media/attachments/")) {
      return serveFile(path.slice("/media/attachments/".length), url);
    }
    // Discord's CDN: server icons and avatars.
    if (req.method === "GET" && /^\/(icons\/\d+|avatars\/\d+|embed\/avatars|guilds\/\d+\/users\/\d+\/avatars)\/[^/]+\.png$/.test(path)) {
      return new Response(picture(path), { headers: { "content-type": "image/png" } });
    }
    if (req.method === "PUT" && path.startsWith("/_upload/")) {
      const upload = uploads.get(decodeURIComponent(path.slice("/_upload/".length)));
      if (!upload) return notFound();
      upload.data = new Uint8Array(await req.arrayBuffer());
      return new Response(null, { status: 200 });
    }
    if (path === "/_control/health") return new Response("ok\n");
    if (path === "/_control/video") {
      return json({
        received: [...received.values()].map(({ key: _, ...stream }) => stream),
        watchers: [...watchers.values()].map(({ timer: _, ...watcher }) => watcher),
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
    if (req.method === "POST" && path === "/_control/chat-stop") {
      clearInterval(chat);
      chat = undefined;
      return new Response("chat stopped\n");
    }
    return unknown(req, path);
  },
  websocket: {
    open(ws) {
      if (ws.data.voice) {
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
      if (ws.data.voice) {
        if (typeof message === "string") voiceMessage(ws, message);
        return;
      }
      gatewayMessage(ws, typeof message === "string" ? message : new TextDecoder().decode(message));
    },
    close(ws) {
      if (ws.data.voice && ws.data.ssrc) clearInterval(watchers.get(ws.data.ssrc)?.timer);
      sockets.delete(ws);
    },
  },
});

console.error(
  `fake Discord on http://${server.hostname}:${server.port} ` +
    `(${general.length} messages in #general ${GENERAL}, server ${GUILD})`,
);
