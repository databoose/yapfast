// A fake Discord for tests and memory measurements: the gateway and the parts
// of the HTTP API Talk uses, with one deterministic server, "Server", that
// mirrors the Matrix fixture (Alice reads #general, Bob posts, three people sit
// in the Lounge voice channel). Run `bun testing/fake_discord.ts --port 18400`,
// then start Talk with TALK_DISCORD_TEST_SERVER=http://127.0.0.1:18400.
// `--extra N` sets how many history messages follow the four fixture ones
// (default 56, 60 in all). POST /_control/chat-start makes Bob post in
// #general every 2 s; POST /_control/chat-stop stops him. Voice media isn't
// faked: joining voice shows Alice in the channel but never connects.

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

type User = { id: string; username: string; global_name: string };
const users: Record<string, User> = {
  alice: { id: ALICE, username: "alice", global_name: "Alice" },
  bob: { id: "1400000000000000002", username: "bob", global_name: "Bob" },
  carol: { id: "1400000000000000003", username: "carol", global_name: "Carol" },
  dave: { id: "1400000000000000004", username: "dave", global_name: "Dave" },
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
    avatar: null,
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
    avatar: null,
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
  const raw = new Uint8Array((width * 4 + 1) * height);
  for (let y = 0; y < height; y++) {
    const row = y * (width * 4 + 1);
    const sy = Math.floor((y * 500) / height);
    for (let x = 0; x < width; x++) {
      const sx = Math.floor((x * 800) / width);
      raw.set([Math.floor(sx / 4) & 255, Math.floor(sy / 3) & 255, 160, 255], row + 1 + x * 4);
    }
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

type VoiceState = { channel: string; self_mute: boolean; self_deaf: boolean; session: string };
const voice = new Map<string, VoiceState>([
  [users.bob.id, { channel: LOUNGE, self_mute: false, self_deaf: false, session: "voice-bob" }],
  [users.carol.id, { channel: LOUNGE, self_mute: true, self_deaf: false, session: "voice-carol" }],
  [users.dave.id, { channel: LOUNGE, self_mute: true, self_deaf: true, session: "voice-dave" }],
]);

function wireVoiceState(userId: string, state: VoiceState | undefined) {
  return {
    guild_id: GUILD,
    channel_id: state?.channel ?? null,
    user_id: userId,
    session_id: state?.session ?? "voice-left",
    self_mute: state?.self_mute ?? false,
    self_deaf: state?.self_deaf ?? false,
    self_video: false,
    self_stream: false,
    mute: false,
    deaf: false,
    suppress: false,
    request_to_speak_timestamp: null,
  };
}

// --- Gateway ----------------------------------------------------------------

type Socket = ServerWebSocket<{ sequence: number; identified: boolean; session: string; origin: string }>;
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
    icon: null,
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
      if (channel) {
        voice.set(ALICE, {
          channel,
          self_mute: !!d.self_mute,
          self_deaf: !!d.self_deaf,
          session: ws.data.session,
        });
      } else {
        voice.delete(ALICE);
      }
      dispatch("VOICE_STATE_UPDATE", { ...wireVoiceState(ALICE, voice.get(ALICE)), member: member(users.alice) });
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
    case 37: // Guild subscriptions: everything is sent anyway.
      break;
    default:
      console.error(`gateway: unhandled op ${payload.op}`);
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

const server = Bun.serve<{ sequence: number; identified: boolean; session: string; origin: string }>({
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
    if (path.startsWith("/api/v9/")) return api(req, path.slice("/api/v9".length), origin);
    if (req.method === "GET" && path.startsWith("/attachments/")) {
      return serveFile(path.slice("/attachments/".length), url);
    }
    if (req.method === "GET" && path.startsWith("/media/attachments/")) {
      return serveFile(path.slice("/media/attachments/".length), url);
    }
    if (req.method === "PUT" && path.startsWith("/_upload/")) {
      const upload = uploads.get(decodeURIComponent(path.slice("/_upload/".length)));
      if (!upload) return notFound();
      upload.data = new Uint8Array(await req.arrayBuffer());
      return new Response(null, { status: 200 });
    }
    if (path === "/_control/health") return new Response("ok\n");
    if (req.method === "POST" && path === "/_control/chat-start") {
      clearInterval(chat);
      chat = setInterval(() => chatTick(origin), CHAT_INTERVAL);
      return new Response("chat started\n");
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
      gatewayMessage(ws, typeof message === "string" ? message : new TextDecoder().decode(message));
    },
    close(ws) {
      sockets.delete(ws);
    },
  },
});

console.error(
  `fake Discord on http://${server.hostname}:${server.port} ` +
    `(${general.length} messages in #general ${GENERAL}, server ${GUILD})`,
);
