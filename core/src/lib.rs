use serde::{Deserialize, Serialize};

uniffi::setup_scaffolding!();

mod client;
pub use client::{ChatMessage, ConnectClient, ConnectClientListener, ConnectionState};

/// Server-assigned per-connection identifier.
pub type PeerId = String;

/// A peer's public identity, as announced to the room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub display_name: String,
    /// Base64 Curve25519 identity key (`vodozemac::Curve25519PublicKey::to_base64`).
    pub identity_key: String,
    /// Base64 Curve25519 one-time key, consumed by the first peer to establish
    /// a session with us. Reused across multiple peers in the same room --
    /// a known, documented limitation of this LAN/dev-focused implementation
    /// (see core/src/client.rs), not full X3DH one-time-key hygiene.
    pub one_time_key: String,
}

/// Messages sent from a client to the server. The server relays ciphertext
/// only -- it never has the keys to decrypt `Message`/`KeyExchange` content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Join {
        display_name: String,
        identity_key: String,
        one_time_key: String,
    },
    /// A Megolm-encrypted chat message, broadcast to the whole room.
    Message { ciphertext: String },
    /// An Olm-encrypted message (in practice, our Megolm session key)
    /// addressed to one specific peer, not broadcast.
    KeyExchange { to: PeerId, ciphertext: String },
}

/// Messages sent from the server to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Sent once, right after a client joins: every currently-connected peer.
    Roster { peers: Vec<PeerInfo> },
    PeerJoined { peer: PeerInfo },
    PeerLeft { peer_id: PeerId },
    Message { from: PeerId, ciphertext: String },
    KeyExchange { from: PeerId, ciphertext: String },
}
