//! Talk's Discord connection: one gateway session kept open while Talk runs.
//! It tracks the account's servers, their voice channels and who is in them,
//! and hands the voice server details for a join to the voice worker.

use std::{collections::HashMap, time::Duration};

use anyhow::{Result, anyhow};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};

use crate::{
    gateway::{Closed, Gateway},
    ready::{self, Server},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Connecting,
    Online,
    /// Disconnected, with a reason a person can act on.
    Offline(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceMember {
    pub user_id: String,
    pub name: String,
    pub muted: bool,
    pub deafened: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub status: Status,
    pub user_id: Option<String>,
    pub servers: Vec<Server>,
    /// Who is in each voice channel, by channel id and then by name.
    pub voice: HashMap<String, Vec<VoiceMember>>,
}

/// What songbird needs to open the voice connection Discord assigned.
#[derive(Clone)]
pub struct VoiceServer {
    pub user_id: String,
    pub session_id: String,
    pub token: String,
    pub endpoint: String,
}

enum Request {
    Join {
        guild: String,
        channel: String,
        reply: oneshot::Sender<Result<VoiceServer>>,
    },
    Leave {
        guild: String,
    },
    SelfState {
        guild: String,
        channel: String,
        mute: bool,
        deaf: bool,
    },
}

/// A handle to the connection. It ends when every handle is dropped.
#[derive(Clone)]
pub struct Discord {
    requests: mpsc::UnboundedSender<Request>,
    snapshots: watch::Receiver<Snapshot>,
}

impl Discord {
    /// Connects on the current Tokio runtime and keeps reconnecting until
    /// Discord rejects the token.
    pub fn start(token: String) -> Self {
        let (requests, receiver) = mpsc::unbounded_channel();
        let (snapshots, watcher) = watch::channel(Snapshot::default());
        tokio::spawn(run(token, receiver, snapshots));
        Self {
            requests,
            snapshots: watcher,
        }
    }

    pub fn snapshots(&self) -> watch::Receiver<Snapshot> {
        self.snapshots.clone()
    }

    /// Asks Discord to join a voice channel, muted like every Talk call at
    /// first, and waits for the voice server it assigns.
    pub async fn join(&self, guild: &str, channel: &str) -> Result<VoiceServer> {
        let (reply, answer) = oneshot::channel();
        self.requests
            .send(Request::Join {
                guild: guild.into(),
                channel: channel.into(),
                reply,
            })
            .map_err(|_| anyhow!("Discord isn't connected. Restart Talk and try again."))?;
        answer
            .await
            .map_err(|_| anyhow!("Discord disconnected while joining voice."))?
    }

    pub fn leave(&self, guild: &str) {
        let _ = self.requests.send(Request::Leave {
            guild: guild.into(),
        });
    }

    /// Shows others whether this account is muted or deafened.
    pub fn set_self(&self, guild: &str, channel: &str, mute: bool, deaf: bool) {
        let _ = self.requests.send(Request::SelfState {
            guild: guild.into(),
            channel: channel.into(),
            mute,
            deaf,
        });
    }
}

const JOIN_TIMEOUT: Duration = Duration::from_secs(15);
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LAST_RETRY: Duration = Duration::from_secs(60);

async fn run(
    token: String,
    mut requests: mpsc::UnboundedReceiver<Request>,
    snapshots: watch::Sender<Snapshot>,
) {
    let mut retry = FIRST_RETRY;
    loop {
        snapshots.send_modify(|snapshot| snapshot.status = Status::Connecting);
        let connected = tokio::select! {
            connected = Gateway::connect(&token) => connected,
            () = snapshots.closed() => return,
        };
        let error = match connected {
            Ok((gateway, ready)) => {
                retry = FIRST_RETRY;
                match connected_session(gateway, &ready, &mut requests, &snapshots).await {
                    Ended::Dropped => return,
                    Ended::Failed(error) => error,
                }
            }
            Err(error) => error,
        };
        let closed = error.downcast_ref::<Closed>();
        tracing::warn!(%error, "Discord connection ended");
        if closed.is_some_and(Closed::is_final) {
            snapshots.send_modify(|snapshot| {
                snapshot.status = Status::Offline(
                    "Discord refused the saved login. Link your Discord account again.".into(),
                );
                snapshot.voice.clear();
            });
            // Keep answering until Talk drops the handles.
            while let Some(request) = requests.recv().await {
                refuse(request);
            }
            return;
        }
        snapshots.send_modify(|snapshot| {
            snapshot.status = Status::Offline("Can't reach Discord. Reconnecting...".into());
            snapshot.voice.clear();
        });
        let wait = tokio::time::sleep(retry);
        tokio::pin!(wait);
        loop {
            tokio::select! {
                () = &mut wait => break,
                request = requests.recv() => match request {
                    Some(request) => refuse(request),
                    None => return,
                },
            }
        }
        retry = (retry * 2).min(LAST_RETRY);
    }
}

fn refuse(request: Request) {
    if let Request::Join { reply, .. } = request {
        let _ = reply.send(Err(anyhow!(
            "Discord isn't connected right now. Try again in a moment."
        )));
    }
}

enum Ended {
    /// Every handle was dropped.
    Dropped,
    Failed(anyhow::Error),
}

struct Pending {
    guild: String,
    reply: oneshot::Sender<Result<VoiceServer>>,
    session_id: Option<String>,
    server: Option<(String, String)>,
}

async fn connected_session(
    mut gateway: Gateway,
    ready: &Value,
    requests: &mut mpsc::UnboundedReceiver<Request>,
    snapshots: &watch::Sender<Snapshot>,
) -> Ended {
    let Some(me) = ready::user_id(ready).map(str::to_owned) else {
        return Ended::Failed(anyhow!("Discord's READY had no user"));
    };
    let mut tracker = Tracker::from_ready(me.clone(), ready);
    snapshots.send_replace(tracker.snapshot());
    let mut pending: Option<Pending> = None;
    let deadline = tokio::time::sleep(JOIN_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else {
                    gateway.close().await;
                    return Ended::Dropped;
                };
                let sent = match request {
                    Request::Join { guild, channel, reply } => {
                        let sent = gateway.update_voice_state(Some(&guild), Some(&channel), true, false).await;
                        deadline.as_mut().reset(tokio::time::Instant::now() + JOIN_TIMEOUT);
                        if let Some(previous) = pending.replace(Pending { guild, reply, session_id: None, server: None }) {
                            let _ = previous.reply.send(Err(anyhow!("Another voice join replaced this one.")));
                        }
                        sent
                    }
                    Request::Leave { guild } => {
                        if pending.as_ref().is_some_and(|pending| pending.guild == guild) {
                            pending = None;
                        }
                        gateway.update_voice_state(Some(&guild), None, false, false).await
                    }
                    Request::SelfState { guild, channel, mute, deaf } => {
                        gateway.update_voice_state(Some(&guild), Some(&channel), mute, deaf).await
                    }
                };
                if let Err(error) = sent {
                    return Ended::Failed(error);
                }
            }
            () = &mut deadline, if pending.is_some() => {
                if let Some(pending) = pending.take() {
                    let _ = pending.reply.send(Err(anyhow!("Discord didn't assign a voice server. Try again.")));
                }
            }
            event = gateway.next() => {
                let event = match event {
                    Ok(event) => event,
                    Err(error) => {
                        if let Some(pending) = pending.take() {
                            let _ = pending.reply.send(Err(anyhow!("Discord disconnected while joining voice.")));
                        }
                        return Ended::Failed(error);
                    }
                };
                if let Some(waiting) = pending.as_mut() {
                    match event.name.as_str() {
                        "VOICE_STATE_UPDATE" if event.data["user_id"] == me.as_str() && event.data["guild_id"] == waiting.guild.as_str() => {
                            waiting.session_id = event.data["session_id"].as_str().map(str::to_owned);
                        }
                        // The endpoint is null while Discord is still allocating a server.
                        "VOICE_SERVER_UPDATE" if event.data["guild_id"] == waiting.guild.as_str() => {
                            if let (Some(token), Some(endpoint)) = (event.data["token"].as_str(), event.data["endpoint"].as_str()) {
                                waiting.server = Some((token.into(), endpoint.into()));
                            }
                        }
                        _ => {}
                    }
                    if let (Some(session_id), Some((token, endpoint))) = (&waiting.session_id, &waiting.server) {
                        let server = VoiceServer { user_id: me.clone(), session_id: session_id.clone(), token: token.clone(), endpoint: endpoint.clone() };
                        if let Some(done) = pending.take() {
                            let _ = done.reply.send(Ok(server));
                        }
                    }
                }
                if tracker.apply(&event.name, &event.data) {
                    snapshots.send_replace(tracker.snapshot());
                }
            }
        }
    }
}

struct VoiceEntry {
    channel: String,
    muted: bool,
    deafened: bool,
}

/// The account's servers and voice channels, kept current from gateway events.
struct Tracker {
    me: String,
    servers: Vec<Server>,
    /// Display names by user id.
    names: HashMap<String, String>,
    /// Server nicknames by server and user id.
    nicknames: HashMap<(String, String), String>,
    /// Where each user is in voice, by server and user id.
    voice: HashMap<(String, String), VoiceEntry>,
    /// This account's roles, by server id.
    my_roles: HashMap<String, Vec<String>>,
}

impl Tracker {
    fn from_ready(me: String, ready: &Value) -> Self {
        let mut tracker = Self {
            me,
            servers: ready::servers(ready),
            names: HashMap::new(),
            nicknames: HashMap::new(),
            voice: HashMap::new(),
            my_roles: HashMap::new(),
        };
        tracker.learn_user(&ready["user"]);
        for user in ready["users"].as_array().into_iter().flatten() {
            tracker.learn_user(user);
        }
        tracker.learn_members(&ready["guilds"], &ready["merged_members"]);
        tracker
    }

    /// Applies one dispatched event; returns whether anything visible changed.
    fn apply(&mut self, name: &str, data: &Value) -> bool {
        match name {
            "READY_SUPPLEMENTAL" => {
                for guild in data["guilds"].as_array().into_iter().flatten() {
                    let Some(id) = guild["id"].as_str() else {
                        continue;
                    };
                    for state in guild["voice_states"].as_array().into_iter().flatten() {
                        self.voice_state(id, state);
                    }
                }
                self.learn_members(&data["guilds"], &data["merged_members"]);
                true
            }
            "VOICE_STATE_UPDATE" => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                self.learn_member(guild, &data["member"]);
                self.voice_state(guild, data);
                true
            }
            "GUILD_CREATE" => {
                let Some(server) = ready::server(data) else {
                    return false;
                };
                for state in data["voice_states"].as_array().into_iter().flatten() {
                    self.voice_state(&server.id, state);
                }
                for member in data["members"].as_array().into_iter().flatten() {
                    self.learn_member(&server.id, member);
                }
                self.servers.retain(|known| known.id != server.id);
                self.servers.push(server);
                ready::sort_servers(&mut self.servers);
                true
            }
            "GUILD_MEMBER_UPDATE" => {
                let Some(guild) = data["guild_id"].as_str() else {
                    return false;
                };
                self.learn_member(guild, data);
                true
            }
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" | "GUILD_ROLE_DELETE" => {
                let Some(server) = data["guild_id"]
                    .as_str()
                    .and_then(|guild| self.servers.iter_mut().find(|server| server.id == guild))
                else {
                    return false;
                };
                if name == "GUILD_ROLE_DELETE" {
                    if let Some(role) = data["role_id"].as_str() {
                        server.roles.remove(role);
                    }
                } else if let Some((id, bits)) = ready::role(&data["role"]) {
                    server.roles.insert(id, bits);
                }
                true
            }
            "GUILD_DELETE" => {
                let Some(id) = data["id"].as_str() else {
                    return false;
                };
                self.servers.retain(|server| server.id != id);
                self.voice.retain(|(guild, _), _| guild != id);
                true
            }
            "CHANNEL_CREATE" | "CHANNEL_UPDATE" | "CHANNEL_DELETE" => {
                let (Some(guild), Some(id)) = (data["guild_id"].as_str(), data["id"].as_str())
                else {
                    return false;
                };
                let Some(server) = self.servers.iter_mut().find(|server| server.id == guild) else {
                    return false;
                };
                server.voice_channels.retain(|channel| channel.id != id);
                if name != "CHANNEL_DELETE"
                    && let Some(channel) = ready::voice_channel(data)
                {
                    server.voice_channels.push(channel);
                    ready::sort_channels(&mut server.voice_channels);
                }
                true
            }
            _ => false,
        }
    }

    fn voice_state(&mut self, guild: &str, state: &Value) {
        let Some(user) = state["user_id"].as_str() else {
            return;
        };
        let key = (guild.to_owned(), user.to_owned());
        match state["channel_id"].as_str() {
            Some(channel) => {
                let flag = |name: &str| state[name].as_bool().unwrap_or(false);
                self.voice.insert(
                    key,
                    VoiceEntry {
                        channel: channel.to_owned(),
                        muted: flag("self_mute") || flag("mute"),
                        deafened: flag("self_deaf") || flag("deaf"),
                    },
                );
            }
            None => {
                self.voice.remove(&key);
            }
        }
    }

    fn learn_user(&mut self, user: &Value) {
        if let (Some(id), Some(name)) = (user["id"].as_str(), ready::display_name(user)) {
            self.names.insert(id.to_owned(), name.to_owned());
        }
    }

    fn learn_member(&mut self, guild: &str, member: &Value) {
        self.learn_user(&member["user"]);
        let user = member["user_id"]
            .as_str()
            .or_else(|| member["user"]["id"].as_str());
        if let (Some(user), Some(nick)) = (user, member["nick"].as_str()) {
            self.nicknames
                .insert((guild.to_owned(), user.to_owned()), nick.to_owned());
        }
        if user == Some(self.me.as_str())
            && let Some(roles) = member["roles"].as_array()
        {
            let roles = roles
                .iter()
                .filter_map(|role| role.as_str().map(str::to_owned))
                .collect();
            self.my_roles.insert(guild.to_owned(), roles);
        }
    }

    /// `merged_members` lists members per guild, in the same order as `guilds`.
    fn learn_members(&mut self, guilds: &Value, merged: &Value) {
        let guilds = guilds.as_array().into_iter().flatten();
        let merged = merged.as_array().into_iter().flatten();
        for (guild, members) in guilds.zip(merged) {
            let Some(guild) = guild["id"].as_str() else {
                continue;
            };
            for member in members.as_array().into_iter().flatten() {
                self.learn_member(guild, member);
            }
        }
    }

    fn name(&self, guild: &str, user: &str) -> String {
        self.nicknames
            .get(&(guild.to_owned(), user.to_owned()))
            .or_else(|| self.names.get(user))
            .cloned()
            .unwrap_or_else(|| "Discord user".into())
    }

    fn snapshot(&self) -> Snapshot {
        let mut voice: HashMap<String, Vec<VoiceMember>> = HashMap::new();
        for ((guild, user), entry) in &self.voice {
            voice
                .entry(entry.channel.clone())
                .or_default()
                .push(VoiceMember {
                    user_id: user.clone(),
                    name: self.name(guild, user),
                    muted: entry.muted,
                    deafened: entry.deafened,
                });
        }
        for members in voice.values_mut() {
            members.sort_by(|a, b| {
                a.name
                    .to_lowercase()
                    .cmp(&b.name.to_lowercase())
                    .then_with(|| a.user_id.cmp(&b.user_id))
            });
        }
        // Mark what this account may do in each voice channel.
        let servers = self
            .servers
            .iter()
            .map(|server| {
                let roles = self.my_roles.get(&server.id).map_or(&[][..], Vec::as_slice);
                let mut server = server.clone();
                let mut channels = std::mem::take(&mut server.voice_channels);
                for channel in &mut channels {
                    channel.access = server.access(channel, &self.me, roles);
                }
                server.voice_channels = channels;
                server
            })
            .collect();
        Snapshot {
            status: Status::Online,
            user_id: Some(self.me.clone()),
            servers,
            voice,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tracks_voice_channels_and_names() {
        let ready = json!({
            "user": {"id": "1", "username": "me"},
            "users": [{"id": "2", "username": "ann", "global_name": "Ann"}],
            "guilds": [{"id": "10", "properties": {"name": "Test"}, "channels": [
                {"id": "11", "name": "Lounge", "type": 2, "position": 0},
            ]}],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready);
        tracker.apply(
            "READY_SUPPLEMENTAL",
            &json!({
                "guilds": [{"id": "10", "voice_states": [
                    {"user_id": "2", "channel_id": "11", "self_mute": true},
                ]}],
                "merged_members": [[{"user_id": "2", "nick": "Annie"}]],
            }),
        );
        let snapshot = tracker.snapshot();
        let lounge = &snapshot.voice["11"];
        assert_eq!(lounge.len(), 1);
        assert_eq!(lounge[0].name, "Annie");
        assert!(lounge[0].muted);

        tracker.apply(
            "VOICE_STATE_UPDATE",
            &json!({"guild_id": "10", "user_id": "2", "channel_id": null}),
        );
        assert!(tracker.snapshot().voice.is_empty());

        tracker.apply(
            "CHANNEL_CREATE",
            &json!({"guild_id": "10", "id": "12", "name": "Games", "type": 2, "position": 1}),
        );
        let names: Vec<_> = tracker.servers[0]
            .voice_channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, ["Lounge", "Games"]);
    }

    #[test]
    fn marks_voice_channels_this_account_can_join() {
        let hidden = r#"[{"id": "10", "type": 0, "allow": "0", "deny": "1024"},
                         {"id": "20", "type": 0, "allow": "1024", "deny": "0"}]"#;
        let ready = json!({
            "user": {"id": "1"},
            "guilds": [{"id": "10", "properties": {"name": "Test", "owner_id": "9"},
                "roles": [{"id": "10", "permissions": "1049600"}, {"id": "20", "permissions": "0"}],
                "channels": [
                    {"id": "11", "name": "Lounge", "type": 2, "position": 0},
                    {"id": "12", "name": "Staff", "type": 2, "position": 1,
                     "permission_overwrites": serde_json::from_str::<Value>(hidden).unwrap()},
                ]}],
            "merged_members": [[{"user_id": "1", "roles": []}]],
        });
        let mut tracker = Tracker::from_ready("1".into(), &ready);
        let channels = |tracker: &Tracker| -> Vec<String> {
            tracker.snapshot().servers[0]
                .voice_channels
                .iter()
                .filter(|channel| channel.access == ready::Access::Join)
                .map(|channel| channel.name.clone())
                .collect()
        };
        assert_eq!(channels(&tracker), ["Lounge"]);
        tracker.apply(
            "GUILD_MEMBER_UPDATE",
            &json!({"guild_id": "10", "user": {"id": "1"}, "roles": ["20"]}),
        );
        assert_eq!(channels(&tracker), ["Lounge", "Staff"]);
    }
}
