//! Which network a room is on, and what that network lets Talk do there.
//! A room's id names its network: Discord's ids are built in
//! [`crate::discord`], and every other id is a Matrix room's. Code that has
//! only an id reads it with [`Network::of`]; a [`crate::chat::ChatRoom`]
//! carries its network and capabilities, which the backend sets.

/// The network a room, a server or a call is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Network {
    #[default]
    Matrix,
    Discord,
}

impl Network {
    /// The network a room or server id names.
    pub fn of(id: &str) -> Self {
        if id.starts_with(crate::discord::PREFIX) {
            Self::Discord
        } else {
            Self::Matrix
        }
    }

    /// What Talk can do in this network's rooms.
    pub const fn capabilities(self) -> Capabilities {
        match self {
            Self::Matrix => Capabilities {
                forward: true,
                spoilers: false,
                categories: false,
                dm_calls: false,
            },
            Self::Discord => Capabilities {
                // Discord turns down forwarding from or to its channels.
                forward: false,
                spoilers: true,
                categories: true,
                dm_calls: true,
            },
        }
    }
}

/// What a room's network lets Talk do there, for the UI to follow rather
/// than asking which network the room is on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities {
    /// Messages can be forwarded into it, and from it.
    pub forward: bool,
    /// Its messages hide text between `||` until clicked.
    pub spoilers: bool,
    /// Its server lists channels under categories of its own.
    pub categories: bool,
    /// A DM or group DM has a call of its own.
    pub dm_calls: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_ids_name_their_network() {
        assert_eq!(Network::of("!room:example.org"), Network::Matrix);
        assert_eq!(Network::of("@ann:example.org"), Network::Matrix);
        assert_eq!(
            Network::of(&crate::discord::room_id("10", "11")),
            Network::Discord
        );
        assert_eq!(
            Network::of(&crate::discord::room_id(crate::discord::DM_GUILD, "20")),
            Network::Discord
        );
        assert_eq!(
            Network::of(&crate::discord::server_id("10")),
            Network::Discord
        );
        assert_eq!(Network::of(""), Network::Matrix);
    }
}
