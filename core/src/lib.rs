use serde::{Deserialize, Serialize};

uniffi::setup_scaffolding!();

mod client;
mod persistence;
pub use client::{ChatMessage, ConnectClient, ConnectClientListener, ConnectionState};

/// Server-assigned per-connection identifier.
pub type PeerId = String;

/// Client-generated identifier for a group chat -- the server has no
/// notion of groups at all, this only means anything to the clients that
/// are members of it.
pub type GroupId = String;

/// A peer's public identity, as announced to everyone connected to the
/// same relay server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub display_name: String,
    /// Base64 Curve25519 identity key (`vodozemac::Curve25519PublicKey::to_base64`).
    pub identity_key: String,
    /// Base64 Curve25519 one-time key, consumed by the first peer to establish
    /// a session with us. Reused across every peer who discovers us this
    /// connection -- a known, documented limitation of this LAN/dev-focused
    /// implementation (see core/src/client.rs), not full X3DH one-time-key
    /// hygiene.
    pub one_time_key: String,
}

/// Messages sent from a client to the server. The server only ever relays
/// ciphertext to the named `to` peer (or, for `Join`, broadcasts peer
/// discovery) -- it never has the keys to decrypt any of it, and has no
/// notion of direct-message conversations or group chats, only "peer X
/// wants this ciphertext delivered to peer Y."
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Join {
        display_name: String,
        identity_key: String,
        one_time_key: String,
    },
    /// An Olm-encrypted 1:1 chat message.
    DirectMessage { to: PeerId, ciphertext: String },
    /// An Olm-encrypted group invite: name + member list, addressed to one
    /// invitee at a time (each invitee gets their own ciphertext, since
    /// it's pairwise-encrypted, not a shared group key).
    GroupInvite { to: PeerId, group_id: GroupId, ciphertext: String },
    /// An Olm-encrypted group chat message, addressed to one member at a
    /// time -- sending to a group of N means N of these, one per member.
    GroupMessage { to: PeerId, group_id: GroupId, ciphertext: String },
}

/// Messages sent from the server to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Sent once, right after a client joins: every currently-connected peer.
    Roster { peers: Vec<PeerInfo> },
    PeerJoined { peer: PeerInfo },
    PeerLeft { peer_id: PeerId },
    DirectMessage { from: PeerId, ciphertext: String },
    GroupInvite { from: PeerId, group_id: GroupId, ciphertext: String },
    GroupMessage { from: PeerId, group_id: GroupId, ciphertext: String },
}
