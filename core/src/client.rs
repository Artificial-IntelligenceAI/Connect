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
    /// The connection dropped unexpectedly (not via `disconnect()`) and a
    /// retry is scheduled with backoff. `attempt` is 1 on the first retry,
    /// incrementing until a connection succeeds again.
    Reconnecting { attempt: u32 },
    Failed { reason: String },
}

/// Delay before retry number `attempt` (1-indexed): 1s, 2s, 4s, 8s, 16s,
/// capped at 30s so a long outage doesn't back off forever.
fn backoff_delay(attempt: u32) -> std::time::Duration {
    let secs = 1u64.checked_shl(attempt.saturating_sub(1)).unwrap_or(u64::MAX);
    std::time::Duration::from_secs(secs.min(30))
}

/// Sleep for `dur`, waking early if `cancel` reports a disconnect. Returns
/// `true` if it was woken by cancellation rather than the timer.
async fn sleep_or_cancel(cancel: &mut tokio::sync::watch::Receiver<bool>, dur: std::time::Duration) -> bool {
    if *cancel.borrow() {
        return true;
    }
    tokio::select! {
        _ = tokio::time::sleep(dur) => false,
        _ = cancel.changed() => true,
    }
}

/// Forget everything learned about peers in the previous connection. Called
/// at the start of every (re)connect attempt after the first: the server
/// assigns a fresh peer_id per connection, so old sessions are keyed by
/// peer_ids nobody will send again, and the fresh `Roster` the server sends
/// right after we (re)join will repopulate this from scratch anyway. Our own
/// identity (`account`) and outbound Megolm session are untouched, so peers
/// who already have our key don't need a redundant key exchange.
fn reset_peer_state(state: &mut CryptoState) {
    state.peer_identity_keys.clear();
    state.peer_display_names.clear();
    state.outbound_olm_sessions.clear();
    state.inbound_olm_sessions.clear();
    state.inbound_group_sessions.clear();
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
    /// Set by `disconnect()` to tell the background connection/retry loop to
    /// stop -- an unexpected drop keeps retrying, but the user asking to
    /// disconnect should not.
    cancel: Mutex<Option<tokio::sync::watch::Sender<bool>>>,
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
            cancel: Mutex::new(None),
        })
    }

    pub fn connect(&self, host: String, port: u16, display_name: String, listener: std::sync::Arc<dyn ConnectClientListener>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *self.outgoing.lock().unwrap() = Some(tx.clone());

        let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
        *self.cancel.lock().unwrap() = Some(cancel_tx);

        let account = persistence::load_or_create_account(&self.data_dir);
        let identity_key = account.curve25519_key().to_base64();
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
            let mut attempt: u32 = 0;
            let mut first_attempt = true;

            loop {
                if *cancel_rx.borrow() {
                    return;
                }

                let ws_stream = match connect_async(&url).await {
                    Ok((stream, _)) => stream,
                    Err(err) => {
                        attempt += 1;
                        listener.on_state_changed(if first_attempt {
                            ConnectionState::Failed { reason: err.to_string() }
                        } else {
                            ConnectionState::Reconnecting { attempt }
                        });
                        if first_attempt || sleep_or_cancel(&mut cancel_rx, backoff_delay(attempt)).await {
                            return;
                        }
                        continue;
                    }
                };
                let (mut write, mut read) = ws_stream.split();

                let (identity_key, one_time_key) = {
                    let mut state = crypto.lock().unwrap();
                    if !first_attempt {
                        reset_peer_state(&mut state);
                    }
                    let otk_result = state.account.generate_one_time_keys(1);
                    let one_time_key = otk_result
                        .created
                        .first()
                        .copied()
                        .expect("just asked for one one-time key");
                    state.account.mark_keys_as_published();
                    persistence::save_account(&state.data_dir, &state.account);
                    (state.account.curve25519_key().to_base64(), one_time_key.to_base64())
                };

                let join = ClientEvent::Join {
                    display_name: display_name.clone(),
                    identity_key,
                    one_time_key,
                };
                let Ok(join_json) = serde_json::to_string(&join) else {
                    return;
                };
                if write.send(WsMessage::Text(join_json.into())).await.is_err() {
                    attempt += 1;
                    listener.on_state_changed(if first_attempt {
                        ConnectionState::Failed { reason: "failed to send join message".into() }
                    } else {
                        ConnectionState::Reconnecting { attempt }
                    });
                    if first_attempt || sleep_or_cancel(&mut cancel_rx, backoff_delay(attempt)).await {
                        return;
                    }
                    continue;
                }

                attempt = 0;
                first_attempt = false;
                listener.on_state_changed(ConnectionState::Connected);

                let recv_listener = listener.clone();
                let recv_crypto = crypto.clone();
                let recv_tx = tx.clone();
                let mut recv_task = tokio::spawn(async move {
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

                loop {
                    tokio::select! {
                        event = rx.recv() => {
                            match event {
                                Some(event) => {
                                    let Ok(json) = serde_json::to_string(&event) else {
                                        continue;
                                    };
                                    if write.send(WsMessage::Text(json.into())).await.is_err() {
                                        break;
                                    }
                                }
                                None => break,
                            }
                        }
                        // The read side ending (cleanly or with an error) is
                        // just as much a disconnect as a failed write -- if
                        // nothing was being sent when the peer went away,
                        // `write.send` above would never fire and this would
                        // be the only signal we get.
                        _ = &mut recv_task => break,
                        _ = cancel_rx.changed() => {
                            if *cancel_rx.borrow() {
                                recv_task.abort();
                                let _ = write.close().await;
                                return;
                            }
                        }
                    }
                }
                recv_task.abort();

                if *cancel_rx.borrow() {
                    return;
                }

                attempt += 1;
                listener.on_state_changed(ConnectionState::Reconnecting { attempt });
                if sleep_or_cancel(&mut cancel_rx, backoff_delay(attempt)).await {
                    return;
                }
            }
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
        if let Some(cancel_tx) = self.cancel.lock().unwrap().take() {
            let _ = cancel_tx.send(true);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct TestListener {
        messages: Mutex<Vec<ChatMessage>>,
        states: Mutex<Vec<ConnectionState>>,
    }

    impl ConnectClientListener for TestListener {
        fn on_state_changed(&self, state: ConnectionState) {
            self.states.lock().unwrap().push(state);
        }
        fn on_message(&self, message: ChatMessage) {
            self.messages.lock().unwrap().push(message);
        }
    }

    /// A throwaway data_dir under the OS temp directory, unique per call --
    /// `handle_new_peer` persists known-peer keys as a side effect, and an
    /// empty/relative data_dir would resolve against the test binary's CWD
    /// (the crate root under `cargo test`), leaving stray JSON files in the
    /// repo instead of a real sandboxed directory.
    fn temp_data_dir() -> String {
        std::env::temp_dir()
            .join(format!("connect-client-test-{}", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    fn new_crypto(display_name: &str) -> SharedCrypto {
        std::sync::Arc::new(Mutex::new(CryptoState {
            account: Account::new(),
            display_name: display_name.to_string(),
            data_dir: temp_data_dir(),
            outbound_group_session: GroupSession::new(Default::default()),
            peer_identity_keys: HashMap::new(),
            peer_display_names: HashMap::new(),
            known_peer_keys: HashMap::new(),
            outbound_olm_sessions: HashMap::new(),
            inbound_olm_sessions: HashMap::new(),
            inbound_group_sessions: HashMap::new(),
        }))
    }

    /// Publishes a one-time key the way `connect()` would and returns the
    /// `PeerInfo` this crypto state would announce to the room under
    /// `peer_id`.
    fn announce(crypto: &SharedCrypto, peer_id: &str) -> PeerInfo {
        let mut state = crypto.lock().unwrap();
        let identity_key = state.account.curve25519_key().to_base64();
        let display_name = state.display_name.clone();
        let otk_result = state.account.generate_one_time_keys(1);
        let one_time_key = otk_result.created.first().copied().unwrap().to_base64();
        state.account.mark_keys_as_published();
        PeerInfo {
            peer_id: peer_id.to_string(),
            display_name,
            identity_key,
            one_time_key,
        }
    }

    #[test]
    fn backoff_delay_grows_exponentially_and_caps_at_30s() {
        assert_eq!(backoff_delay(1), std::time::Duration::from_secs(1));
        assert_eq!(backoff_delay(2), std::time::Duration::from_secs(2));
        assert_eq!(backoff_delay(3), std::time::Duration::from_secs(4));
        assert_eq!(backoff_delay(4), std::time::Duration::from_secs(8));
        assert_eq!(backoff_delay(5), std::time::Duration::from_secs(16));
        assert_eq!(backoff_delay(6), std::time::Duration::from_secs(30)); // would be 32 uncapped
        assert_eq!(backoff_delay(20), std::time::Duration::from_secs(30));
    }

    #[tokio::test]
    async fn sleep_or_cancel_returns_false_when_the_timer_elapses() {
        let (_tx, mut rx) = tokio::sync::watch::channel(false);
        let cancelled = sleep_or_cancel(&mut rx, std::time::Duration::from_millis(5)).await;
        assert!(!cancelled);
    }

    #[tokio::test]
    async fn sleep_or_cancel_wakes_early_and_returns_true_on_cancel() {
        let (tx, mut rx) = tokio::sync::watch::channel(false);
        let waiter = tokio::spawn(async move {
            sleep_or_cancel(&mut rx, std::time::Duration::from_secs(30)).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        tx.send(true).unwrap();
        assert!(waiter.await.unwrap());
    }

    #[test]
    fn reset_peer_state_clears_per_peer_maps_but_keeps_identity() {
        let crypto = new_crypto("Alice");
        let identity_before = {
            let mut state = crypto.lock().unwrap();
            let key = state.account.curve25519_key();
            state.peer_identity_keys.insert("p1".into(), key);
            state.peer_display_names.insert("p1".into(), "Someone".into());
            state.account.curve25519_key().to_base64()
        };

        reset_peer_state(&mut crypto.lock().unwrap());

        let state = crypto.lock().unwrap();
        assert!(state.peer_identity_keys.is_empty());
        assert!(state.peer_display_names.is_empty());
        assert_eq!(state.account.curve25519_key().to_base64(), identity_before);
    }

    /// The core value proposition, exercised end to end without a server:
    /// two independent crypto states learn about each other, exchange Olm
    /// key material, and can then decrypt each other's Megolm chat
    /// messages -- in both directions.
    #[test]
    fn full_handshake_and_message_roundtrip_both_directions() {
        let alice = new_crypto("Alice");
        let bob = new_crypto("Bob");
        let alice_info = announce(&alice, "alice-peer");
        let bob_info = announce(&bob, "bob-peer");

        let (alice_tx, mut alice_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let (bob_tx, mut bob_rx) = mpsc::unbounded_channel::<ClientEvent>();
        let alice_listener: std::sync::Arc<dyn ConnectClientListener> =
            std::sync::Arc::new(TestListener::default());
        let bob_listener: std::sync::Arc<dyn ConnectClientListener> =
            std::sync::Arc::new(TestListener::default());

        // Each side learns about the other (as if from a Roster/PeerJoined
        // event) and fires off an Olm-encrypted Megolm key exchange.
        handle_new_peer(&alice, &alice_tx, &alice_listener, &bob_info);
        handle_new_peer(&bob, &bob_tx, &bob_listener, &alice_info);

        let alice_to_bob = match alice_rx.try_recv().unwrap() {
            ClientEvent::KeyExchange { to, ciphertext } => {
                assert_eq!(to, "bob-peer");
                ciphertext
            }
            other => panic!("expected a KeyExchange, got {other:?}"),
        };
        let bob_to_alice = match bob_rx.try_recv().unwrap() {
            ClientEvent::KeyExchange { to, ciphertext } => {
                assert_eq!(to, "alice-peer");
                ciphertext
            }
            other => panic!("expected a KeyExchange, got {other:?}"),
        };

        // Deliver each side's key exchange to the other, establishing
        // inbound Olm sessions and, from them, inbound Megolm sessions.
        handle_key_exchange(&bob, &"alice-peer".to_string(), &alice_to_bob);
        handle_key_exchange(&alice, &"bob-peer".to_string(), &bob_to_alice);

        let ciphertext = {
            let mut state = alice.lock().unwrap();
            state.outbound_group_session.encrypt("hello bob").to_base64()
        };
        let (sender, text) = decrypt_message(&bob, &"alice-peer".to_string(), &ciphertext)
            .expect("bob should be able to decrypt alice's message");
        assert_eq!(sender, "Alice");
        assert_eq!(text, "hello bob");

        let ciphertext = {
            let mut state = bob.lock().unwrap();
            state.outbound_group_session.encrypt("hi alice").to_base64()
        };
        let (sender, text) = decrypt_message(&alice, &"bob-peer".to_string(), &ciphertext)
            .expect("alice should be able to decrypt bob's message");
        assert_eq!(sender, "Bob");
        assert_eq!(text, "hi alice");
    }

    #[test]
    fn decrypt_message_from_a_peer_with_no_session_returns_none() {
        let alice = new_crypto("Alice");
        assert!(decrypt_message(&alice, &"nobody".to_string(), "not-real-ciphertext").is_none());
    }

    #[test]
    fn tofu_first_contact_is_remembered_with_a_new_contact_notice() {
        let alice = new_crypto("Alice");
        let bob = new_crypto("Bob");
        let bob_info = announce(&bob, "bob-peer");

        let (tx, _rx) = mpsc::unbounded_channel::<ClientEvent>();
        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();
        handle_new_peer(&alice, &tx, &dyn_listener, &bob_info);

        let messages = listener.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert!(messages[0].text.contains("New contact"));
        assert_eq!(
            alice.lock().unwrap().known_peer_keys.get("Bob"),
            Some(&bob_info.identity_key)
        );
    }

    #[test]
    fn tofu_unchanged_key_on_reconnect_is_silent() {
        let alice = new_crypto("Alice");
        let bob = new_crypto("Bob");
        let bob_info = announce(&bob, "bob-peer");

        let (tx, _rx) = mpsc::unbounded_channel::<ClientEvent>();
        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();

        handle_new_peer(&alice, &tx, &dyn_listener, &bob_info);
        // Bob reconnects: same identity key, but the server hands out a
        // fresh peer_id for the new connection.
        let mut bob_info_again = bob_info.clone();
        bob_info_again.peer_id = "bob-peer-2".into();
        handle_new_peer(&alice, &tx, &dyn_listener, &bob_info_again);

        assert_eq!(
            listener.messages.lock().unwrap().len(),
            1,
            "an unchanged identity key on reconnect should not raise a second notice"
        );
    }

    #[test]
    fn tofu_changed_key_for_a_known_name_raises_a_warning() {
        let alice = new_crypto("Alice");
        let bob = new_crypto("Bob");
        let bob_info = announce(&bob, "bob-peer");

        let (tx, _rx) = mpsc::unbounded_channel::<ClientEvent>();
        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();
        handle_new_peer(&alice, &tx, &dyn_listener, &bob_info);

        // Someone else (or a fresh install) shows up using Bob's name with
        // a different identity key.
        let impostor = new_crypto("Bob");
        let impostor_info = announce(&impostor, "bob-peer-2");
        handle_new_peer(&alice, &tx, &dyn_listener, &impostor_info);

        let messages = listener.messages.lock().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[1].text.contains("identity key has changed"));
    }

    // -- WebSocket state machine (connect()'s retry/backoff loop) --------
    //
    // These drive `ConnectClient::connect` against a real local TCP
    // listener instead of a mock, so they exercise the actual
    // `connect_async`/`tokio::select!` state machine, not just the logic
    // it calls out to. They're slower and timing-sensitive compared to
    // the crypto tests above (real sockets, real backoff delays), so each
    // wait is bounded by an explicit timeout that panics with a clear
    // message instead of hanging a stuck run indefinitely.

    async fn wait_until(
        listener: &TestListener,
        timeout: std::time::Duration,
        predicate: impl Fn(&Vec<ConnectionState>) -> bool,
    ) -> Vec<ConnectionState> {
        tokio::time::timeout(timeout, async {
            loop {
                {
                    let states = listener.states.lock().unwrap();
                    if predicate(&states) {
                        return states.clone();
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("timed out waiting for the expected connection states")
    }

    #[tokio::test]
    async fn first_attempt_failure_reports_failed_and_does_not_retry() {
        // Bind to get a free ephemeral port, then drop the listener so
        // nothing is actually listening there -- a deterministic way to
        // get connection-refused without racing a real server's shutdown.
        let doomed_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = doomed_listener.local_addr().unwrap().port();
        drop(doomed_listener);

        let client = ConnectClient::new(temp_data_dir());
        let test_listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = test_listener.clone();
        client.connect("127.0.0.1".into(), port, "Nobody".into(), dyn_listener);

        let states = wait_until(&test_listener, std::time::Duration::from_secs(5), |s| {
            s.iter().any(|state| matches!(state, ConnectionState::Failed { .. }))
        })
        .await;
        assert!(
            !states.iter().any(|state| matches!(state, ConnectionState::Reconnecting { .. })),
            "a first-attempt failure should report Failed, not retry: {states:?}"
        );

        // Confirm it really stopped, rather than being about to retry.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        assert_eq!(
            test_listener.states.lock().unwrap().len(),
            states.len(),
            "no further state changes should follow a first-attempt Failed"
        );
    }

    #[tokio::test]
    async fn drop_after_connecting_triggers_reconnect_to_the_same_address() {
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();

        let client = ConnectClient::new(temp_data_dir());
        let test_listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = test_listener.clone();
        client.connect("127.0.0.1".into(), port, "Reconnector".into(), dyn_listener);

        let (stream, _) = server.accept().await.unwrap();
        let first_ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        // Don't drop the socket until Connected has actually been
        // reported, so this exercises "drop after a real connection", not
        // a race with the initial join handshake.
        wait_until(&test_listener, std::time::Duration::from_secs(5), |s| {
            matches!(s.last(), Some(ConnectionState::Connected))
        })
        .await;

        drop(first_ws); // the server vanishes with nothing in flight

        wait_until(&test_listener, std::time::Duration::from_secs(5), |s| {
            s.iter().any(|state| matches!(state, ConnectionState::Reconnecting { attempt: 1 }))
        })
        .await;

        // The retry loop should come back to the same host:port.
        let (stream, _) = server.accept().await.unwrap();
        let _second_ws = tokio_tungstenite::accept_async(stream).await.unwrap();

        let states = wait_until(&test_listener, std::time::Duration::from_secs(10), |s| {
            s.iter().filter(|state| matches!(state, ConnectionState::Connected)).count() >= 2
        })
        .await;
        assert!(states.iter().any(|s| matches!(s, ConnectionState::Reconnecting { attempt: 1 })));
    }

    #[tokio::test]
    async fn disconnect_actually_stops_the_retry_loop() {
        // Regression test: `disconnect()` used to only clear the client's
        // own field references, leaving the spawned retry loop running
        // forever in the background. If that regresses, this test hangs
        // waiting for a server that never comes back, rather than passing.
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();

        let client = ConnectClient::new(temp_data_dir());
        let test_listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = test_listener.clone();
        client.connect("127.0.0.1".into(), port, "Canceller".into(), dyn_listener);

        let (stream, _) = server.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        wait_until(&test_listener, std::time::Duration::from_secs(5), |s| {
            matches!(s.last(), Some(ConnectionState::Connected))
        })
        .await;

        drop(ws);
        wait_until(&test_listener, std::time::Duration::from_secs(5), |s| {
            s.iter().any(|state| matches!(state, ConnectionState::Reconnecting { .. }))
        })
        .await;

        client.disconnect();
        let count_at_disconnect = test_listener.states.lock().unwrap().len();

        // The fake server deliberately never accepts again -- if
        // cancellation didn't actually take effect, the retry loop would
        // keep trying (and failing) and this count would grow.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
        assert_eq!(
            count_at_disconnect,
            test_listener.states.lock().unwrap().len(),
            "disconnect() should stop the retry loop, not just clear local references"
        );
    }
}
