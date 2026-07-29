use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use vodozemac::olm::{Account, OlmMessage};
use vodozemac::megolm::{GroupSession, InboundGroupSession, MegolmMessage, SessionKey};
use vodozemac::Curve25519PublicKey;

use crate::persistence;
use crate::{ClientEvent, PeerId, PeerInfo, ServerEvent};

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Runtime::new().expect("failed to start Tokio runtime")
    })
}

/// Connection lifecycle, mirrored to the UI layer.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Failed { reason: String },
}

/// A single chat message or system notice, ready for display. Always
/// plaintext by the time it reaches the UI layer -- decryption happens
/// entirely below this boundary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChatMessage {
    pub from: String,
    pub text: String,
    pub is_system: bool,
}

/// Implemented by the host app (Swift/Kotlin) to receive events from the client.
/// Methods may be called from a background thread; the implementation is
/// responsible for hopping to the main thread before touching UI state.
#[uniffi::export(with_foreign)]
pub trait ConnectClientListener: Send + Sync {
    fn on_state_changed(&self, state: ConnectionState);
    fn on_message(&self, message: ChatMessage);
}

/// End-to-end encryption state for one connection: our own identity/outbound
/// ratchet, plus everything we've learned about peers currently in the room.
///
/// Uses vodozemac's Olm (pairwise, X3DH-style) to privately hand each peer
/// our Megolm outbound session key, then Megolm (group ratchet) to encrypt
/// the actual chat messages once per send rather than once per recipient.
/// This is the same architecture Matrix uses.
///
/// The identity (`account`) is persisted to `data_dir` and reloaded across
/// restarts (see persistence.rs), and `known_peer_keys` is a
/// trust-on-first-use store keyed by display name -- if a name we've seen
/// before shows up with a different identity key, that's surfaced to the
/// listener as a system-message warning rather than silently accepted.
/// Known, deliberate limitations still remaining for this v1: each client
/// only ever publishes a single Olm one-time key, which -- unlike textbook
/// X3DH -- gets reused if more than one peer establishes a session with us
/// before we reconnect; TOFU is anchored to display name, so a peer
/// impersonating an existing name from a fresh identity looks identical to
/// that peer just changing devices (no stronger identity than "the name
/// someone typed in"). Message history isn't available to late joiners
/// (true even before E2EE existed: the server never stored anything).
struct CryptoState {
    account: Account,
    display_name: String,
    data_dir: String,
    outbound_group_session: GroupSession,
    peer_identity_keys: HashMap<PeerId, Curve25519PublicKey>,
    peer_display_names: HashMap<PeerId, String>,
    /// display_name -> base64 identity key, persisted to disk.
    known_peer_keys: HashMap<String, String>,
    // Deliberately two separate maps, not one keyed by peer_id: the Olm
    // session we create to *send* a peer our Megolm key is a different
    // object from the one we create to *decrypt* the Megolm key they send
    // us, even though both are "with" the same peer. Collapsing them into
    // one map means decrypt ends up using the wrong session and silently
    // fails.
    outbound_olm_sessions: HashMap<PeerId, vodozemac::olm::Session>,
    inbound_olm_sessions: HashMap<PeerId, vodozemac::olm::Session>,
    inbound_group_sessions: HashMap<PeerId, InboundGroupSession>,
}

type SharedCrypto = std::sync::Arc<Mutex<CryptoState>>;

/// A LAN relay client: connects over WebSocket, establishes end-to-end
/// encrypted sessions with every other peer in the room, and reports
/// decrypted events back through a listener. The server only ever sees
/// ciphertext for `Message`/`KeyExchange` traffic.
#[derive(uniffi::Object)]
pub struct ConnectClient {
    /// Directory this client's identity and known-peer-keys are persisted
    /// to. Must be a writable, platform-sandboxed directory (app support
    /// dir on Apple platforms, `Context.filesDir` on Android) -- the Rust
    /// core has no notion of "the right place" on any given OS, so the
    /// platform layer is responsible for supplying it.
    data_dir: String,
    outgoing: Mutex<Option<mpsc::UnboundedSender<ClientEvent>>>,
    crypto: Mutex<Option<SharedCrypto>>,
    listener: Mutex<Option<std::sync::Arc<dyn ConnectClientListener>>>,
}

#[uniffi::export]
impl ConnectClient {
    #[uniffi::constructor]
    pub fn new(data_dir: String) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            data_dir,
            outgoing: Mutex::new(None),
            crypto: Mutex::new(None),
            listener: Mutex::new(None),
        })
    }

    pub fn connect(&self, host: String, port: u16, display_name: String, listener: std::sync::Arc<dyn ConnectClientListener>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *self.outgoing.lock().unwrap() = Some(tx.clone());

        let mut account = persistence::load_or_create_account(&self.data_dir);
        let otk_result = account.generate_one_time_keys(1);
        let one_time_key = otk_result
            .created
            .first()
            .copied()
            .expect("just asked for one one-time key");
        account.mark_keys_as_published();
        persistence::save_account(&self.data_dir, &account);

        let identity_key = account.curve25519_key().to_base64();
        let one_time_key = one_time_key.to_base64();
        let outbound_group_session = GroupSession::new(Default::default());
        let known_peer_keys = persistence::load_known_peers(&self.data_dir);

        let crypto: SharedCrypto = std::sync::Arc::new(Mutex::new(CryptoState {
            account,
            display_name: display_name.clone(),
            data_dir: self.data_dir.clone(),
            outbound_group_session,
            peer_identity_keys: HashMap::new(),
            peer_display_names: HashMap::new(),
            known_peer_keys,
            outbound_olm_sessions: HashMap::new(),
            inbound_olm_sessions: HashMap::new(),
            inbound_group_sessions: HashMap::new(),
        }));
        *self.crypto.lock().unwrap() = Some(crypto.clone());
        *self.listener.lock().unwrap() = Some(listener.clone());

        listener.on_state_changed(ConnectionState::Connecting);
        listener.on_message(ChatMessage {
            from: String::new(),
            text: format!(
                "Your fingerprint: {}",
                persistence::format_fingerprint(&identity_key)
            ),
            is_system: true,
        });

        runtime().spawn(async move {
            let url = format!("ws://{host}:{port}/ws");
            let ws_stream = match connect_async(&url).await {
                Ok((stream, _)) => stream,
                Err(err) => {
                    listener.on_state_changed(ConnectionState::Failed {
                        reason: err.to_string(),
                    });
                    return;
                }
            };
            let (mut write, mut read) = ws_stream.split();

            let join = ClientEvent::Join {
                display_name,
                identity_key,
                one_time_key,
            };
            let Ok(join_json) = serde_json::to_string(&join) else {
                return;
            };
            if write.send(WsMessage::Text(join_json.into())).await.is_err() {
                listener.on_state_changed(ConnectionState::Failed {
                    reason: "failed to send join message".into(),
                });
                return;
            }
            listener.on_state_changed(ConnectionState::Connected);

            let recv_listener = listener.clone();
            let recv_crypto = crypto.clone();
            let recv_tx = tx.clone();
            let recv_task = tokio::spawn(async move {
                while let Some(Ok(msg)) = read.next().await {
                    let WsMessage::Text(text) = msg else { continue };
                    let Ok(event) = serde_json::from_str::<ServerEvent>(&text) else {
                        continue;
                    };

                    match event {
                        ServerEvent::Roster { peers } => {
                            for peer in peers {
                                handle_new_peer(&recv_crypto, &recv_tx, &recv_listener, &peer);
                            }
                        }
                        ServerEvent::PeerJoined { peer } => {
                            let display_name = peer.display_name.clone();
                            handle_new_peer(&recv_crypto, &recv_tx, &recv_listener, &peer);
                            recv_listener.on_message(ChatMessage {
                                from: String::new(),
                                text: format!("{display_name} joined"),
                                is_system: true,
                            });
                        }
                        ServerEvent::PeerLeft { peer_id } => {
                            let display_name = {
                                let mut state = recv_crypto.lock().unwrap();
                                state.outbound_olm_sessions.remove(&peer_id);
                                state.inbound_olm_sessions.remove(&peer_id);
                                state.inbound_group_sessions.remove(&peer_id);
                                state.peer_identity_keys.remove(&peer_id);
                                state.peer_display_names.remove(&peer_id)
                            };
                            if let Some(display_name) = display_name {
                                recv_listener.on_message(ChatMessage {
                                    from: String::new(),
                                    text: format!("{display_name} left"),
                                    is_system: true,
                                });
                            }
                        }
                        ServerEvent::KeyExchange { from, ciphertext } => {
                            handle_key_exchange(&recv_crypto, &from, &ciphertext);
                        }
                        ServerEvent::Message { from, ciphertext } => {
                            if let Some((sender_name, text)) =
                                decrypt_message(&recv_crypto, &from, &ciphertext)
                            {
                                recv_listener.on_message(ChatMessage {
                                    from: sender_name,
                                    text,
                                    is_system: false,
                                });
                            }
                        }
                    }
                }
            });

            while let Some(event) = rx.recv().await {
                let Ok(json) = serde_json::to_string(&event) else {
                    continue;
                };
                if write.send(WsMessage::Text(json.into())).await.is_err() {
                    break;
                }
            }
            recv_task.abort();
        });
    }

    pub fn send(&self, text: String) {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return;
        };
        let (ciphertext, from) = {
            let mut state = crypto.lock().unwrap();
            let ciphertext = state.outbound_group_session.encrypt(&text).to_base64();
            (ciphertext, state.display_name.clone())
        };
        if let Some(tx) = self.outgoing.lock().unwrap().as_ref() {
            let _ = tx.send(ClientEvent::Message { ciphertext });
        }

        // The server doesn't echo our own messages back to us (it only
        // broadcasts to *other* peers), so echo locally instead of round-
        // tripping through a self-decrypt.
        if let Some(listener) = self.listener.lock().unwrap().as_ref() {
            listener.on_message(ChatMessage {
                from,
                text,
                is_system: false,
            });
        }
    }

    pub fn disconnect(&self) {
        *self.outgoing.lock().unwrap() = None;
        *self.crypto.lock().unwrap() = None;
        *self.listener.lock().unwrap() = None;
    }
}

/// Learn a peer's identity key -- checking it against what we've seen for
/// that display name before (trust-on-first-use) -- and, if we haven't
/// already, establish an outbound Olm session to them and hand them our
/// Megolm session key through it, so they can decrypt messages we send
/// from now on.
fn handle_new_peer(
    crypto: &SharedCrypto,
    tx: &mpsc::UnboundedSender<ClientEvent>,
    listener: &std::sync::Arc<dyn ConnectClientListener>,
    peer: &PeerInfo,
) {
    let Ok(identity_key) = Curve25519PublicKey::from_base64(&peer.identity_key) else {
        return;
    };
    let Ok(one_time_key) = Curve25519PublicKey::from_base64(&peer.one_time_key) else {
        return;
    };

    let mut state = crypto.lock().unwrap();
    state.peer_identity_keys.insert(peer.peer_id.clone(), identity_key);
    state
        .peer_display_names
        .insert(peer.peer_id.clone(), peer.display_name.clone());

    let verification_notice = match state.known_peer_keys.get(&peer.display_name) {
        Some(known_key) if known_key == &peer.identity_key => None,
        Some(_different_key) => Some(format!(
            "\u{26A0}\u{FE0F} {}'s identity key has changed since last time -- \
             could be a new device, could be someone else using that name. \
             New fingerprint: {}",
            peer.display_name,
            persistence::format_fingerprint(&peer.identity_key)
        )),
        None => Some(format!(
            "\u{1F511} New contact: {}. Fingerprint: {}",
            peer.display_name,
            persistence::format_fingerprint(&peer.identity_key)
        )),
    };
    state
        .known_peer_keys
        .insert(peer.display_name.clone(), peer.identity_key.clone());
    let data_dir = state.data_dir.clone();
    let known_peers_snapshot = state.known_peer_keys.clone();
    drop(state);

    persistence::save_known_peers(&data_dir, &known_peers_snapshot);
    if let Some(text) = verification_notice {
        listener.on_message(ChatMessage {
            from: String::new(),
            text,
            is_system: true,
        });
    }

    let mut state = crypto.lock().unwrap();
    if state.outbound_olm_sessions.contains_key(&peer.peer_id) {
        return;
    }

    let Ok(mut session) =
        state
            .account
            .create_outbound_session(Default::default(), identity_key, one_time_key)
    else {
        return;
    };

    let session_key_b64 = state.outbound_group_session.session_key().to_base64();
    let Ok(olm_message) = session.encrypt(session_key_b64.as_bytes()) else {
        return;
    };
    state.outbound_olm_sessions.insert(peer.peer_id.clone(), session);
    drop(state);

    let Ok(ciphertext) = serde_json::to_string(&olm_message) else {
        return;
    };
    let _ = tx.send(ClientEvent::KeyExchange {
        to: peer.peer_id.clone(),
        ciphertext,
    });
}

/// Decrypt an incoming Olm message (our Megolm session key from `from`),
/// creating an inbound Olm session first if this is their first message to us.
fn handle_key_exchange(crypto: &SharedCrypto, from: &PeerId, ciphertext: &str) {
    let Ok(olm_message) = serde_json::from_str::<OlmMessage>(ciphertext) else {
        return;
    };

    let mut state = crypto.lock().unwrap();
    let plaintext = if let Some(session) = state.inbound_olm_sessions.get_mut(from) {
        session.decrypt(&olm_message).ok()
    } else {
        let OlmMessage::PreKey(pre_key_message) = &olm_message else {
            return;
        };
        let Some(&identity_key) = state.peer_identity_keys.get(from) else {
            return;
        };
        let Ok(result) =
            state
                .account
                .create_inbound_session(Default::default(), identity_key, pre_key_message)
        else {
            return;
        };
        state.inbound_olm_sessions.insert(from.clone(), result.session);
        Some(result.plaintext)
    };

    let Some(plaintext) = plaintext else { return };
    let Ok(session_key_b64) = String::from_utf8(plaintext) else {
        return;
    };
    let Ok(session_key) = SessionKey::from_base64(&session_key_b64) else {
        return;
    };
    let inbound = InboundGroupSession::new(&session_key, Default::default());
    state.inbound_group_sessions.insert(from.clone(), inbound);
}

/// Decrypt a broadcast chat message from `from`, returning (display_name, text).
fn decrypt_message(crypto: &SharedCrypto, from: &PeerId, ciphertext: &str) -> Option<(String, String)> {
    let megolm_message = MegolmMessage::from_base64(ciphertext).ok()?;
    let mut state = crypto.lock().unwrap();
    let session = state.inbound_group_sessions.get_mut(from)?;
    let decrypted = session.decrypt(&megolm_message).ok()?;
    let text = String::from_utf8(decrypted.plaintext).ok()?;
    let sender_name = state
        .peer_display_names
        .get(from)
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    Some((sender_name, text))
}
