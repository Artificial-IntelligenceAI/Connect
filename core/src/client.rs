use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use vodozemac::olm::{Account, OlmMessage};
use vodozemac::Curve25519PublicKey;

use crate::persistence;
use crate::{ClientEvent, GroupId, PeerId, PeerInfo, ServerEvent};

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

/// Forget everything about the previous connection's *ephemeral* peer_ids.
/// Called at the start of every (re)connect attempt after the first: the
/// server assigns a fresh peer_id per connection, so old peer_id-keyed
/// mappings point nowhere useful, and the fresh `Roster` the server sends
/// right after we (re)join will repopulate them from scratch anyway. Our
/// own identity (`account`), persisted group metadata, and -- notably --
/// the Olm sessions themselves are untouched: sessions are keyed by stable
/// identity key (see `CryptoState`'s doc comment), not by peer_id, so they
/// survive a reconnect intact instead of forcing every conversation to
/// silently re-handshake from scratch.
fn reset_peer_state(state: &mut CryptoState) {
    state.peer_identity_keys.clear();
    state.peer_display_names.clear();
    state.peer_id_by_identity.clear();
}

/// Which conversation a `ChatMessage` belongs to. `System` covers
/// connection-lifecycle notices (fingerprint, join/leave, TOFU warnings) --
/// nothing a user sent, nothing to reply to.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum Conversation {
    System,
    /// A 1:1 conversation, identified by the other party's stable identity
    /// key rather than their peer_id, which churns on every reconnect.
    Direct { peer_identity_key: String },
    /// A group chat. `group_name` rides along on every message since
    /// there's no separate lookup API yet for "what's this group called" --
    /// that's part of the future chat-list GUI, not this pass.
    Group { group_id: GroupId, group_name: String },
}

/// A single chat message or system notice, ready for display. Always
/// plaintext by the time it reaches the UI layer -- decryption happens
/// entirely below this boundary.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ChatMessage {
    pub from: String,
    pub text: String,
    pub conversation: Conversation,
}

/// A snapshot of one entry in the trust-on-first-use contacts store
/// (`known_peer_keys`), for populating a 1:1 chat list. `peer_id` is
/// `Some` if they're reachable on the current connection right now,
/// `None` if they're only known from a past session.
#[derive(Debug, Clone, uniffi::Record)]
pub struct KnownPeer {
    pub identity_key: String,
    pub display_name: String,
    pub peer_id: Option<PeerId>,
}

/// A snapshot of one persisted group chat, for populating a group chat
/// list. No membership detail beyond a count -- see `create_group`'s doc
/// comment for why full membership resolution only happens at send time.
#[derive(Debug, Clone, uniffi::Record)]
pub struct GroupSummary {
    pub group_id: GroupId,
    pub name: String,
    pub member_count: u32,
}

/// Implemented by the host app (Swift/Kotlin) to receive events from the client.
/// Methods may be called from a background thread; the implementation is
/// responsible for hopping to the main thread before touching UI state.
#[uniffi::export(with_foreign)]
pub trait ConnectClientListener: Send + Sync {
    fn on_state_changed(&self, state: ConnectionState);
    fn on_message(&self, message: ChatMessage);
}

/// End-to-end encryption state for one connection: our own identity, plus
/// everything we've learned about peers currently reachable through the
/// same relay server.
///
/// Everything -- 1:1 direct messages, group invites, and group chat
/// messages -- rides on vodozemac's Olm (pairwise, X3DH-style double
/// ratchet): a group message is just the same Olm-encrypted payload sent
/// individually to each current member, not a shared group ratchet like
/// Matrix's Megolm. That trades O(1) encryption per group send for
/// O(members), which doesn't matter at this app's scale and avoids a
/// whole class of key-distribution/mesh-convergence complexity a shared
/// group key would need.
///
/// The identity (`account`) is persisted to `data_dir` and reloaded across
/// restarts (see persistence.rs), and `known_peer_keys` is a
/// trust-on-first-use store keyed by display name -- if a name we've seen
/// before shows up with a different identity key, that's surfaced to the
/// listener as a system-message warning rather than silently accepted.
///
/// Known, deliberate v1 limitations: each client only ever publishes a
/// single Olm one-time key, reused across every peer who discovers us this
/// connection, not consumed once each per textbook X3DH; TOFU is anchored
/// to display name, so a peer impersonating an existing name from a fresh
/// identity looks identical to that peer just changing devices; Olm
/// sessions live only in memory (keyed by identity, so they survive a
/// reconnect, but not a full app restart -- there's no session
/// persistence to disk, only the long-term `Account`); a DM or group chat
/// only works between peers who have discovered each other through the
/// same relay server at some point (not necessarily *right now* -- see
/// `invite_to_group` for the one place that's no longer a hard
/// requirement). Message history isn't available to late joiners either
/// (true even before E2EE existed: the server never stored anything).
struct CryptoState {
    account: Account,
    display_name: String,
    data_dir: String,
    peer_identity_keys: HashMap<PeerId, Curve25519PublicKey>,
    peer_display_names: HashMap<PeerId, String>,
    /// display_name -> base64 identity key, persisted to disk.
    known_peer_keys: HashMap<String, String>,
    /// base64 identity key -> current live peer_id, so a DM/group send can
    /// resolve a stable member identity to whichever connection they're
    /// using right now. Rebuilt from scratch on every (re)connect, same as
    /// the peer-id-keyed maps above.
    peer_id_by_identity: HashMap<String, PeerId>,
    // Deliberately two separate maps, not one keyed by identity: the Olm
    // session we create to *send* a peer something is a different object
    // from the one we create to *decrypt* what they send us, even though
    // both are "with" the same peer. Collapsing them into one map means
    // decrypt ends up using the wrong session and silently fails. Used for
    // everything Olm-encrypted -- 1:1 messages, group invites, and group
    // messages all reuse these same per-identity sessions.
    //
    // Keyed by base64 identity key, not `PeerId`: a `PeerId` is only valid
    // for the lifetime of one connection (the server hands out a fresh one
    // every time), but an Olm session is fundamentally tied to an
    // *identity*, not a connection. Keying by identity means a session
    // survives our own reconnects (see `reset_peer_state`) *and* the
    // other party's -- load-bearing for `invite_to_group`, which needs to
    // be able to encrypt something for a peer who isn't even online right
    // now.
    outbound_olm_sessions: HashMap<String, vodozemac::olm::Session>,
    inbound_olm_sessions: HashMap<String, vodozemac::olm::Session>,
    /// group_id -> {name, members}, loaded from groups.json at connect()
    /// and updated whenever we create a group, invite someone to one, or
    /// receive an invite to one. No crypto session state lives here --
    /// group messages are just pairwise Olm messages fanned out to each
    /// member, see `send_group_message`.
    groups: HashMap<GroupId, persistence::GroupMetadata>,
}

type SharedCrypto = std::sync::Arc<Mutex<CryptoState>>;

/// A LAN relay client: connects over WebSocket and exchanges end-to-end
/// encrypted 1:1 messages and group chat messages with other peers
/// reachable through the same relay server. The server only ever sees
/// ciphertext, routed by peer_id -- never anything about who's messaging
/// whom or what a group is called.
#[derive(uniffi::Object)]
pub struct ConnectClient {
    /// Directory this client's identity, known-peer-keys, and group
    /// metadata are persisted to. Must be a writable, platform-sandboxed
    /// directory (app support dir on Apple platforms, `Context.filesDir`
    /// on Android) -- the Rust core has no notion of "the right place" on
    /// any given OS, so the platform layer is responsible for supplying it.
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
        let known_peer_keys = persistence::load_known_peers(&self.data_dir);
        let groups = persistence::load_groups(&self.data_dir);

        let crypto: SharedCrypto = std::sync::Arc::new(Mutex::new(CryptoState {
            account,
            display_name: display_name.clone(),
            data_dir: self.data_dir.clone(),
            peer_identity_keys: HashMap::new(),
            peer_display_names: HashMap::new(),
            known_peer_keys,
            peer_id_by_identity: HashMap::new(),
            outbound_olm_sessions: HashMap::new(),
            inbound_olm_sessions: HashMap::new(),
            groups,
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
            conversation: Conversation::System,
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
                let mut recv_task = tokio::spawn(async move {
                    while let Some(Ok(msg)) = read.next().await {
                        let WsMessage::Text(text) = msg else { continue };
                        let Ok(event) = serde_json::from_str::<ServerEvent>(&text) else {
                            continue;
                        };

                        match event {
                            ServerEvent::Roster { peers } => {
                                for peer in peers {
                                    handle_new_peer(&recv_crypto, &recv_listener, &peer);
                                }
                            }
                            ServerEvent::PeerJoined { peer } => {
                                let display_name = peer.display_name.clone();
                                handle_new_peer(&recv_crypto, &recv_listener, &peer);
                                recv_listener.on_message(ChatMessage {
                                    from: String::new(),
                                    text: format!("{display_name} joined"),
                                    conversation: Conversation::System,
                                });
                            }
                            ServerEvent::PeerLeft { peer_id } => {
                                let display_name = {
                                    let mut state = recv_crypto.lock().unwrap();
                                    state.outbound_olm_sessions.remove(&peer_id);
                                    state.inbound_olm_sessions.remove(&peer_id);
                                    if let Some(identity_key) = state.peer_identity_keys.remove(&peer_id) {
                                        state.peer_id_by_identity.remove(&identity_key.to_base64());
                                    }
                                    state.peer_display_names.remove(&peer_id)
                                };
                                if let Some(display_name) = display_name {
                                    recv_listener.on_message(ChatMessage {
                                        from: String::new(),
                                        text: format!("{display_name} left"),
                                        conversation: Conversation::System,
                                    });
                                }
                            }
                            ServerEvent::DirectMessage { from, ciphertext } => {
                                if let Some((sender_name, text)) =
                                    handle_direct_message(&recv_crypto, &from, &ciphertext)
                                {
                                    let peer_identity_key = recv_crypto
                                        .lock()
                                        .unwrap()
                                        .peer_identity_keys
                                        .get(&from)
                                        .map(|k| k.to_base64());
                                    if let Some(peer_identity_key) = peer_identity_key {
                                        recv_listener.on_message(ChatMessage {
                                            from: sender_name,
                                            text,
                                            conversation: Conversation::Direct { peer_identity_key },
                                        });
                                    }
                                }
                            }
                            ServerEvent::GroupInvite { from, group_id, ciphertext } => {
                                let from_identity_key = identity_key_for_peer(&recv_crypto, &from);
                                if let Some(group_name) = from_identity_key
                                    .and_then(|k| handle_group_invite(&recv_crypto, &k, &group_id, &ciphertext))
                                {
                                    recv_listener.on_message(ChatMessage {
                                        from: String::new(),
                                        text: format!("Added to group \"{group_name}\""),
                                        conversation: Conversation::System,
                                    });
                                }
                            }
                            ServerEvent::InviteToGroup { from_identity_key, group_id, ciphertext } => {
                                if let Some(group_name) =
                                    handle_group_invite(&recv_crypto, &from_identity_key, &group_id, &ciphertext)
                                {
                                    recv_listener.on_message(ChatMessage {
                                        from: String::new(),
                                        text: format!("Added to group \"{group_name}\""),
                                        conversation: Conversation::System,
                                    });
                                }
                            }
                            ServerEvent::GroupMessage { from, group_id, ciphertext } => {
                                if let Some((sender_name, group_name, text)) =
                                    handle_group_message(&recv_crypto, &from, &group_id, &ciphertext)
                                {
                                    recv_listener.on_message(ChatMessage {
                                        from: sender_name,
                                        text,
                                        conversation: Conversation::Group { group_id, group_name },
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

    /// Send a 1:1 message to `peer_identity_key`, resolving their current
    /// live `peer_id` at send time (not whenever the caller first learned
    /// about them) -- same pattern `send_group_message` already uses, so a
    /// UI holding a chat open across one of their reconnects doesn't need
    /// to track a separately-refreshed peer_id itself. If they're not
    /// currently reachable, the network send is skipped, but the message
    /// still echoes locally -- matches this file's existing fire-and-forget
    /// philosophy (delivery was never confirmed even before this).
    pub fn send_direct_message(&self, peer_identity_key: String, text: String) {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return;
        };

        let peer_id = crypto.lock().unwrap().peer_id_by_identity.get(&peer_identity_key).cloned();
        if let Some(peer_id) = &peer_id {
            let ciphertext = {
                let mut state = crypto.lock().unwrap();
                state.outbound_olm_sessions.get_mut(&peer_identity_key).and_then(|session| {
                    let olm_message = session.encrypt(text.as_bytes()).ok()?;
                    serde_json::to_string(&olm_message).ok()
                })
            };
            if let Some(ciphertext) = ciphertext {
                if let Some(tx) = self.outgoing.lock().unwrap().as_ref() {
                    let _ = tx.send(ClientEvent::DirectMessage { to: peer_id.clone(), ciphertext });
                }
            }
        }

        // The server never echoes anything back to its sender, so echo
        // locally instead of round-tripping through a self-decrypt.
        let from = crypto.lock().unwrap().display_name.clone();
        if let Some(listener) = self.listener.lock().unwrap().as_ref() {
            listener.on_message(ChatMessage {
                from,
                text,
                conversation: Conversation::Direct { peer_identity_key },
            });
        }
    }

    /// Create a group chat named `name` with the given currently-online
    /// peers as its (fixed, v1) membership, persist it, and invite each
    /// member. Returns the new group's id, or `None` if not connected or
    /// none of `member_peer_ids` resolved to a known peer.
    pub fn create_group(&self, name: String, member_peer_ids: Vec<String>) -> Option<String> {
        let crypto = self.crypto.lock().unwrap().clone()?;

        let members: Vec<persistence::GroupMember> = {
            let state = crypto.lock().unwrap();
            member_peer_ids
                .iter()
                .filter_map(|peer_id| {
                    let identity_key = state.peer_identity_keys.get(peer_id)?.to_base64();
                    let display_name = state.peer_display_names.get(peer_id)?.clone();
                    Some(persistence::GroupMember { identity_key, display_name })
                })
                .collect()
        };
        if members.is_empty() {
            return None;
        }

        let group_id = uuid::Uuid::new_v4().to_string();
        let data_dir = {
            let mut state = crypto.lock().unwrap();
            state.groups.insert(
                group_id.clone(),
                persistence::GroupMetadata { name: name.clone(), members: members.clone() },
            );
            state.data_dir.clone()
        };
        persistence::save_groups(&data_dir, &crypto.lock().unwrap().groups.clone());

        let payload = GroupInvitePayload { name, members };
        if let Ok(payload_json) = serde_json::to_string(&payload) {
            if let Some(tx) = self.outgoing.lock().unwrap().as_ref() {
                let mut state = crypto.lock().unwrap();
                for peer_id in &member_peer_ids {
                    let Some(identity_key) = state.peer_identity_keys.get(peer_id).map(|k| k.to_base64()) else {
                        continue;
                    };
                    let Some(session) = state.outbound_olm_sessions.get_mut(&identity_key) else { continue };
                    let Ok(olm_message) = session.encrypt(payload_json.as_bytes()) else { continue };
                    let Ok(ciphertext) = serde_json::to_string(&olm_message) else { continue };
                    let _ = tx.send(ClientEvent::GroupInvite {
                        to: peer_id.clone(),
                        group_id: group_id.clone(),
                        ciphertext,
                    });
                }
            }
        }
        Some(group_id)
    }

    /// Send a message to every currently-resolvable (online) member of
    /// `group_id` -- one pairwise Olm-encrypted `GroupMessage` per member,
    /// see `CryptoState`'s doc comment for why there's no shared group key.
    pub fn send_group_message(&self, group_id: String, text: String) {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return;
        };

        let (targets, group_name, from) = {
            let state = crypto.lock().unwrap();
            let Some(group) = state.groups.get(&group_id) else {
                return;
            };
            // (peer_id to address the wire message to, identity_key to look
            // up the right Olm session) for every member currently online.
            let targets: Vec<(PeerId, String)> = group
                .members
                .iter()
                .filter_map(|m| {
                    let peer_id = state.peer_id_by_identity.get(&m.identity_key)?.clone();
                    Some((peer_id, m.identity_key.clone()))
                })
                .collect();
            (targets, group.name.clone(), state.display_name.clone())
        };

        if let Some(tx) = self.outgoing.lock().unwrap().as_ref() {
            let mut state = crypto.lock().unwrap();
            for (peer_id, identity_key) in &targets {
                let Some(session) = state.outbound_olm_sessions.get_mut(identity_key) else { continue };
                let Ok(olm_message) = session.encrypt(text.as_bytes()) else { continue };
                let Ok(ciphertext) = serde_json::to_string(&olm_message) else { continue };
                let _ = tx.send(ClientEvent::GroupMessage {
                    to: peer_id.clone(),
                    group_id: group_id.clone(),
                    ciphertext,
                });
            }
        }

        if let Some(listener) = self.listener.lock().unwrap().as_ref() {
            listener.on_message(ChatMessage {
                from,
                text,
                conversation: Conversation::Group { group_id, group_name },
            });
        }
    }

    /// Invite `peer_identity_key` -- any known peer, online or offline --
    /// to an existing group, updating its persisted membership and sending
    /// the invite via `ClientEvent::InviteToGroup`. Unlike `create_group`'s
    /// invite step, this doesn't need a live peer_id: the server delivers
    /// it immediately if they're online, or holds it until they next join
    /// otherwise (see `server/src/main.rs`'s pending-invite mailbox).
    ///
    /// Requires an existing outbound Olm session with that identity --
    /// i.e. they must already be a "known peer" (`list_known_peers()`),
    /// someone this client has discovered through the relay at some point.
    /// Returns `false` if the group doesn't exist, they're already a
    /// member, we don't recognize the identity, or we're not connected.
    pub fn invite_to_group(&self, group_id: String, peer_identity_key: String) -> bool {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return false;
        };

        let Some(display_name) = crypto
            .lock()
            .unwrap()
            .known_peer_keys
            .iter()
            .find(|(_, identity)| **identity == peer_identity_key)
            .map(|(name, _)| name.clone())
        else {
            return false;
        };

        // Build the *updated* member list -- including the new invitee --
        // up front, so the payload they receive actually tells them
        // they're in the group, not just who was there before them.
        let (payload_json, group_name, updated_members) = {
            let state = crypto.lock().unwrap();
            let Some(group) = state.groups.get(&group_id) else {
                return false;
            };
            if group.members.iter().any(|m| m.identity_key == peer_identity_key) {
                return false;
            }
            let mut updated_members = group.members.clone();
            updated_members.push(persistence::GroupMember {
                identity_key: peer_identity_key.clone(),
                display_name: display_name.clone(),
            });
            let payload = GroupInvitePayload { name: group.name.clone(), members: updated_members.clone() };
            let Ok(payload_json) = serde_json::to_string(&payload) else {
                return false;
            };
            (payload_json, group.name.clone(), updated_members)
        };

        let ciphertext = {
            let mut state = crypto.lock().unwrap();
            state.outbound_olm_sessions.get_mut(&peer_identity_key).and_then(|session| {
                let olm_message = session.encrypt(payload_json.as_bytes()).ok()?;
                serde_json::to_string(&olm_message).ok()
            })
        };
        let Some(ciphertext) = ciphertext else {
            return false;
        };

        let data_dir = {
            let mut state = crypto.lock().unwrap();
            if let Some(group) = state.groups.get_mut(&group_id) {
                group.members = updated_members;
            }
            state.data_dir.clone()
        };
        persistence::save_groups(&data_dir, &crypto.lock().unwrap().groups.clone());

        if let Some(tx) = self.outgoing.lock().unwrap().as_ref() {
            let _ = tx.send(ClientEvent::InviteToGroup {
                to_identity_key: peer_identity_key,
                group_id,
                ciphertext,
            });
        }

        if let Some(listener) = self.listener.lock().unwrap().as_ref() {
            listener.on_message(ChatMessage {
                from: String::new(),
                text: format!("Invited {display_name} to \"{group_name}\""),
                conversation: Conversation::System,
            });
        }
        true
    }

    /// Identity keys of `group_id`'s current members, for filtering an
    /// "invite someone" list down to people who aren't already in it.
    /// Empty if the group is unknown or we're not connected. Note this
    /// never includes the local user's own identity key -- membership
    /// lists only ever record *other* members, the same way
    /// `create_group`'s `member_peer_ids` never includes the creator.
    pub fn list_group_members(&self, group_id: String) -> Vec<String> {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return Vec::new();
        };
        let state = crypto.lock().unwrap();
        state
            .groups
            .get(&group_id)
            .map(|group| group.members.iter().map(|m| m.identity_key.clone()).collect())
            .unwrap_or_default()
    }

    /// Every peer this client has ever discovered (persisted TOFU contacts,
    /// `known_peers.json`), for populating a 1:1 chat list. Empty if not
    /// currently connected -- the contacts themselves are only loaded into
    /// memory once `connect()` has run.
    pub fn list_known_peers(&self) -> Vec<KnownPeer> {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return Vec::new();
        };
        let state = crypto.lock().unwrap();
        state
            .known_peer_keys
            .iter()
            .map(|(display_name, identity_key)| KnownPeer {
                identity_key: identity_key.clone(),
                display_name: display_name.clone(),
                peer_id: state.peer_id_by_identity.get(identity_key).cloned(),
            })
            .collect()
    }

    /// Every group chat this client is a member of (persisted `groups.json`,
    /// created locally or learned via a `GroupInvite`), for populating a
    /// group chat list. Empty if not currently connected.
    pub fn list_groups(&self) -> Vec<GroupSummary> {
        let Some(crypto) = self.crypto.lock().unwrap().clone() else {
            return Vec::new();
        };
        let state = crypto.lock().unwrap();
        state
            .groups
            .iter()
            .map(|(group_id, group)| GroupSummary {
                group_id: group_id.clone(),
                name: group.name.clone(),
                member_count: group.members.len() as u32,
            })
            .collect()
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

/// The plaintext payload carried inside a `GroupInvite`'s Olm ciphertext:
/// enough for the invitee to know the group's name and every member's
/// stable identity (so they can resolve live peer_ids for future sends
/// via `peer_id_by_identity`, and re-persist the same metadata locally).
#[derive(serde::Serialize, serde::Deserialize)]
struct GroupInvitePayload {
    name: String,
    members: Vec<persistence::GroupMember>,
}

/// Learn a peer's identity key -- checking it against what we've seen for
/// that display name before (trust-on-first-use) -- and prepare a local
/// outbound Olm session to them (using their published identity_key and
/// one_time_key). Nothing is sent over the network here: session prep is
/// pure local computation, ready for whenever we or they actually start a
/// 1:1 conversation, a group invite, or a group message.
fn handle_new_peer(
    crypto: &SharedCrypto,
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
    state
        .peer_id_by_identity
        .insert(peer.identity_key.clone(), peer.peer_id.clone());

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
            conversation: Conversation::System,
        });
    }

    let mut state = crypto.lock().unwrap();
    // If we already have an outbound session with this identity -- from
    // earlier in this same connection, or preserved across a reconnect --
    // keep using it rather than starting a fresh one. A new session would
    // still work, but it'd needlessly reset the ratchet and, since the
    // identity's one-time key is reused per-connection rather than
    // consumed once (see the v1-limitations doc comment above), there's no
    // freshness to gain from redoing it.
    if state.outbound_olm_sessions.contains_key(&peer.identity_key) {
        return;
    }
    let Ok(session) =
        state
            .account
            .create_outbound_session(Default::default(), identity_key, one_time_key)
    else {
        return;
    };
    state.outbound_olm_sessions.insert(peer.identity_key.clone(), session);
}

/// Decrypt an Olm-encrypted payload from `from_identity_key`, creating an
/// inbound session first if this is the first message we've ever received
/// from that identity (a PreKeyMessage). Shared by direct messages, group
/// invites, and group messages -- any of the three can legitimately be the
/// first thing a fresh Olm session ever carries, now that there's no
/// dedicated handshake message type.
fn olm_decrypt(crypto: &SharedCrypto, from_identity_key: &str, ciphertext: &str) -> Option<Vec<u8>> {
    let olm_message = serde_json::from_str::<OlmMessage>(ciphertext).ok()?;
    let mut state = crypto.lock().unwrap();
    if let Some(session) = state.inbound_olm_sessions.get_mut(from_identity_key) {
        return session.decrypt(&olm_message).ok();
    }
    let OlmMessage::PreKey(pre_key_message) = &olm_message else {
        return None;
    };
    let identity_key = Curve25519PublicKey::from_base64(from_identity_key).ok()?;
    let result = state
        .account
        .create_inbound_session(Default::default(), identity_key, pre_key_message)
        .ok()?;
    state
        .inbound_olm_sessions
        .insert(from_identity_key.to_string(), result.session);
    Some(result.plaintext)
}

/// Resolve a live peer_id's stable identity key, for handing off to
/// `olm_decrypt`. Only meaningful for the three events still addressed by
/// peer_id (`DirectMessage`/`GroupInvite`/`GroupMessage`) -- `InviteToGroup`
/// already carries the sender's identity key directly.
fn identity_key_for_peer(crypto: &SharedCrypto, peer_id: &PeerId) -> Option<String> {
    crypto.lock().unwrap().peer_identity_keys.get(peer_id).map(|k| k.to_base64())
}

/// Decrypt a 1:1 chat message from `from`, returning (sender display name, text).
fn handle_direct_message(crypto: &SharedCrypto, from: &PeerId, ciphertext: &str) -> Option<(String, String)> {
    let from_identity_key = identity_key_for_peer(crypto, from)?;
    let plaintext = olm_decrypt(crypto, &from_identity_key, ciphertext)?;
    let text = String::from_utf8(plaintext).ok()?;
    let sender_name = crypto.lock().unwrap().peer_display_names.get(from).cloned()?;
    Some((sender_name, text))
}

/// Learn about a group we've been invited to (or re-invited to, whether
/// that's a reconnect or a fresh `invite_to_group` from someone), persisting
/// its metadata. Returns the group's name, for a "you were added" notice,
/// on success.
fn handle_group_invite(crypto: &SharedCrypto, from_identity_key: &str, group_id: &GroupId, ciphertext: &str) -> Option<String> {
    let plaintext = olm_decrypt(crypto, from_identity_key, ciphertext)?;
    let payload: GroupInvitePayload = serde_json::from_slice(&plaintext).ok()?;

    let data_dir = {
        let mut state = crypto.lock().unwrap();
        state.groups.insert(
            group_id.clone(),
            persistence::GroupMetadata { name: payload.name.clone(), members: payload.members },
        );
        state.data_dir.clone()
    };
    let snapshot = crypto.lock().unwrap().groups.clone();
    persistence::save_groups(&data_dir, &snapshot);
    Some(payload.name)
}

/// Decrypt a group chat message from `from`, returning (sender display
/// name, group name, text).
fn handle_group_message(
    crypto: &SharedCrypto,
    from: &PeerId,
    group_id: &GroupId,
    ciphertext: &str,
) -> Option<(String, String, String)> {
    let from_identity_key = identity_key_for_peer(crypto, from)?;
    let plaintext = olm_decrypt(crypto, &from_identity_key, ciphertext)?;
    let text = String::from_utf8(plaintext).ok()?;
    let state = crypto.lock().unwrap();
    let sender_name = state.peer_display_names.get(from).cloned()?;
    let group_name = state.groups.get(group_id)?.name.clone();
    Some((sender_name, group_name, text))
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
            peer_identity_keys: HashMap::new(),
            peer_display_names: HashMap::new(),
            known_peer_keys: HashMap::new(),
            peer_id_by_identity: HashMap::new(),
            outbound_olm_sessions: HashMap::new(),
            inbound_olm_sessions: HashMap::new(),
            groups: HashMap::new(),
        }))
    }

    /// Publishes a one-time key the way `connect()` would and returns the
    /// `PeerInfo` this crypto state would announce under `peer_id`.
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

    /// Runs `handle_new_peer` for `a` learning about `b` and vice versa, so
    /// both sides have a prepped outbound Olm session to the other --
    /// mirrors what a real Roster/PeerJoined exchange does before any
    /// message can be sent.
    fn discover_each_other(a: &SharedCrypto, a_info: &PeerInfo, b: &SharedCrypto, b_info: &PeerInfo) {
        let listener: std::sync::Arc<dyn ConnectClientListener> = std::sync::Arc::new(TestListener::default());
        handle_new_peer(a, &listener, b_info);
        handle_new_peer(b, &listener, a_info);
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
            state.peer_id_by_identity.insert(key.to_base64(), "p1".into());
            state.account.curve25519_key().to_base64()
        };

        reset_peer_state(&mut crypto.lock().unwrap());

        let state = crypto.lock().unwrap();
        assert!(state.peer_identity_keys.is_empty());
        assert!(state.peer_display_names.is_empty());
        assert!(state.peer_id_by_identity.is_empty());
        assert_eq!(state.account.curve25519_key().to_base64(), identity_before);
    }

    #[test]
    fn decrypt_direct_message_from_a_peer_with_no_session_returns_none() {
        let alice = new_crypto("Alice");
        assert!(handle_direct_message(&alice, &"nobody".to_string(), "not-real-ciphertext").is_none());
    }

    #[test]
    fn tofu_first_contact_is_remembered_with_a_new_contact_notice() {
        let alice = new_crypto("Alice");
        let bob = new_crypto("Bob");
        let bob_info = announce(&bob, "bob-peer");

        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();
        handle_new_peer(&alice, &dyn_listener, &bob_info);

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

        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();

        handle_new_peer(&alice, &dyn_listener, &bob_info);
        // Bob reconnects: same identity key, but the server hands out a
        // fresh peer_id for the new connection.
        let mut bob_info_again = bob_info.clone();
        bob_info_again.peer_id = "bob-peer-2".into();
        handle_new_peer(&alice, &dyn_listener, &bob_info_again);

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

        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();
        handle_new_peer(&alice, &dyn_listener, &bob_info);

        // Someone else (or a fresh install) shows up using Bob's name with
        // a different identity key.
        let impostor = new_crypto("Bob");
        let impostor_info = announce(&impostor, "bob-peer-2");
        handle_new_peer(&alice, &dyn_listener, &impostor_info);

        let messages = listener.messages.lock().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(messages[1].text.contains("identity key has changed"));
    }

    /// The core 1:1 value proposition: two independent crypto states
    /// discover each other and exchange several Olm-encrypted messages in
    /// both directions -- not just a single one-shot roundtrip, which
    /// wouldn't catch a ratchet-advance bug that only shows up once a
    /// session has processed more than one message.
    #[test]
    fn direct_message_roundtrip_survives_several_messages_before_any_reply() {
        let alice = new_crypto("Alice");
        let bob = new_crypto("Bob");
        let alice_info = announce(&alice, "alice-peer");
        let bob_info = announce(&bob, "bob-peer");
        discover_each_other(&alice, &alice_info, &bob, &bob_info);

        let encrypt_to_bob = |text: &str| {
            let mut state = alice.lock().unwrap();
            let session = state.outbound_olm_sessions.get_mut(&bob_info.identity_key).unwrap();
            let msg = session.encrypt(text.as_bytes()).unwrap();
            serde_json::to_string(&msg).unwrap()
        };

        let first = encrypt_to_bob("hi bob, 1");
        let second = encrypt_to_bob("hi bob, 2");
        let third = encrypt_to_bob("hi bob, 3");

        let (sender, text) = handle_direct_message(&bob, &"alice-peer".to_string(), &first).unwrap();
        assert_eq!((sender.as_str(), text.as_str()), ("Alice", "hi bob, 1"));
        let (_, text) = handle_direct_message(&bob, &"alice-peer".to_string(), &second).unwrap();
        assert_eq!(text, "hi bob, 2");
        let (_, text) = handle_direct_message(&bob, &"alice-peer".to_string(), &third).unwrap();
        assert_eq!(text, "hi bob, 3");

        // And the reply direction, on Bob's independently-created outbound
        // session (a fresh, separate ratchet from Alice's, per the
        // CryptoState doc comment on the two-map split).
        let reply = {
            let mut state = bob.lock().unwrap();
            let session = state.outbound_olm_sessions.get_mut(&alice_info.identity_key).unwrap();
            let msg = session.encrypt("hey alice".as_bytes()).unwrap();
            serde_json::to_string(&msg).unwrap()
        };
        let (sender, text) = handle_direct_message(&alice, &"bob-peer".to_string(), &reply).unwrap();
        assert_eq!((sender.as_str(), text.as_str()), ("Bob", "hey alice"));
    }

    /// Group create -> invite -> message, among three parties: the creator
    /// and two members can all decrypt each other's group messages, and a
    /// non-member (who never got an invite) cannot.
    #[test]
    fn group_invite_and_message_roundtrip_among_three_parties() {
        let alice = new_crypto("Alice"); // creator
        let bob = new_crypto("Bob"); // member
        let carol = new_crypto("Carol"); // not invited
        let alice_info = announce(&alice, "alice-peer");
        let bob_info = announce(&bob, "bob-peer");
        let carol_info = announce(&carol, "carol-peer");
        discover_each_other(&alice, &alice_info, &bob, &bob_info);
        discover_each_other(&alice, &alice_info, &carol, &carol_info);
        discover_each_other(&bob, &bob_info, &carol, &carol_info);

        let group_id: GroupId = "group-1".to_string();
        let payload = GroupInvitePayload {
            name: "Family".to_string(),
            members: vec![
                persistence::GroupMember { identity_key: alice_info.identity_key.clone(), display_name: "Alice".into() },
                persistence::GroupMember { identity_key: bob_info.identity_key.clone(), display_name: "Bob".into() },
            ],
        };
        let payload_json = serde_json::to_string(&payload).unwrap();

        // Alice invites Bob (but not Carol).
        let invite_ciphertext = {
            let mut state = alice.lock().unwrap();
            let session = state.outbound_olm_sessions.get_mut(&bob_info.identity_key).unwrap();
            let msg = session.encrypt(payload_json.as_bytes()).unwrap();
            serde_json::to_string(&msg).unwrap()
        };
        let group_name = handle_group_invite(&bob, &alice_info.identity_key, &group_id, &invite_ciphertext)
            .expect("bob should learn about the group");
        assert_eq!(group_name, "Family");
        assert_eq!(bob.lock().unwrap().groups[&group_id].members.len(), 2);

        // Alice also records the group locally the way create_group() would.
        alice.lock().unwrap().groups.insert(
            group_id.clone(),
            persistence::GroupMetadata { name: "Family".to_string(), members: payload.members },
        );

        // Bob sends a group message; Alice (a member) can decrypt it.
        let bob_message_ciphertext = {
            let mut state = bob.lock().unwrap();
            let session = state.outbound_olm_sessions.get_mut(&alice_info.identity_key).unwrap();
            let msg = session.encrypt("hello family".as_bytes()).unwrap();
            serde_json::to_string(&msg).unwrap()
        };
        let (sender, name, text) =
            handle_group_message(&alice, &"bob-peer".to_string(), &group_id, &bob_message_ciphertext)
                .expect("alice should decrypt bob's group message");
        assert_eq!(sender, "Bob");
        assert_eq!(name, "Family");
        assert_eq!(text, "hello family");

        // Carol was never invited: she has no group state at all, so even
        // though she could technically decrypt Olm ciphertext addressed to
        // her, nothing group-related was ever sent to her -- there's simply
        // no ciphertext for her to receive. Confirm the group stayed
        // unknown to her.
        assert!(!carol.lock().unwrap().groups.contains_key(&group_id));
    }

    // -- Chat-list query methods -------------------------------------------
    //
    // These construct a `ConnectClient` directly and reach into its private
    // `crypto`/`outgoing`/`listener` fields (legal: this test module is a
    // descendant of `client`) to seed state without needing a real
    // connection, the same shortcut `new_crypto`/`announce` above use.

    #[test]
    fn list_known_peers_and_groups_are_empty_when_not_connected() {
        let client = ConnectClient::new(temp_data_dir());
        assert!(client.list_known_peers().is_empty());
        assert!(client.list_groups().is_empty());
    }

    #[test]
    fn list_known_peers_reports_online_and_offline_status() {
        let client = ConnectClient::new(temp_data_dir());
        let crypto = new_crypto("Alice");
        {
            let mut state = crypto.lock().unwrap();
            state.known_peer_keys.insert("Bob".into(), "bob-identity-key".into());
            state.known_peer_keys.insert("Carol".into(), "carol-identity-key".into());
            // Bob is currently reachable; Carol is only known from the past
            // (no matching peer_id_by_identity entry).
            state.peer_id_by_identity.insert("bob-identity-key".into(), "bob-peer".into());
        }
        *client.crypto.lock().unwrap() = Some(crypto);

        let mut peers = client.list_known_peers();
        peers.sort_by(|a, b| a.display_name.cmp(&b.display_name));

        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].display_name, "Bob");
        assert_eq!(peers[0].peer_id.as_deref(), Some("bob-peer"));
        assert_eq!(peers[1].display_name, "Carol");
        assert_eq!(peers[1].peer_id, None);
    }

    #[test]
    fn list_groups_reports_names_and_member_counts() {
        let client = ConnectClient::new(temp_data_dir());
        let crypto = new_crypto("Alice");
        {
            let mut state = crypto.lock().unwrap();
            state.groups.insert(
                "group-1".to_string(),
                persistence::GroupMetadata {
                    name: "Family".to_string(),
                    members: vec![
                        persistence::GroupMember { identity_key: "a".into(), display_name: "Alice".into() },
                        persistence::GroupMember { identity_key: "b".into(), display_name: "Bob".into() },
                    ],
                },
            );
        }
        *client.crypto.lock().unwrap() = Some(crypto);

        let groups = client.list_groups();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Family");
        assert_eq!(groups[0].member_count, 2);
    }

    #[test]
    fn list_group_members_reports_identity_keys_and_is_empty_for_an_unknown_group() {
        let client = ConnectClient::new(temp_data_dir());
        let crypto = new_crypto("Alice");
        {
            let mut state = crypto.lock().unwrap();
            state.groups.insert(
                "group-1".to_string(),
                persistence::GroupMetadata {
                    name: "Family".to_string(),
                    members: vec![
                        persistence::GroupMember { identity_key: "a".into(), display_name: "Alice".into() },
                        persistence::GroupMember { identity_key: "b".into(), display_name: "Bob".into() },
                    ],
                },
            );
        }
        *client.crypto.lock().unwrap() = Some(crypto);

        let mut members = client.list_group_members("group-1".to_string());
        members.sort();
        assert_eq!(members, vec!["a".to_string(), "b".to_string()]);

        assert!(client.list_group_members("no-such-group".to_string()).is_empty());
    }

    /// Regression guard for the `send_direct_message` signature change:
    /// resolving the target's live peer_id now happens at send time, and
    /// an unresolvable (offline) target must not silently drop the local
    /// echo the way an early `return` before the echo block used to.
    #[test]
    fn send_direct_message_echoes_locally_even_when_the_target_is_offline() {
        let client = ConnectClient::new(temp_data_dir());
        *client.crypto.lock().unwrap() = Some(new_crypto("Alice"));

        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *client.outgoing.lock().unwrap() = Some(tx);
        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();
        *client.listener.lock().unwrap() = Some(dyn_listener);

        client.send_direct_message("offline-bob-identity".to_string(), "hi".to_string());

        assert!(
            rx.try_recv().is_err(),
            "no network send should happen for a peer with no resolvable peer_id"
        );
        let messages = listener.messages.lock().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "hi");
        assert!(matches!(
            &messages[0].conversation,
            Conversation::Direct { peer_identity_key } if peer_identity_key == "offline-bob-identity"
        ));
    }

    // -- invite_to_group ---------------------------------------------------
    //
    // The whole point of this method: it must work for a peer who isn't
    // currently reachable, as long as we've discovered them at some point
    // before (i.e. they show up in `list_known_peers()`). These tests
    // simulate "known but offline right now" explicitly, by discovering a
    // peer once via `handle_new_peer` and then removing only their
    // peer_id-keyed mappings -- exactly what's left after they disconnect,
    // now that Olm sessions are keyed by identity instead of peer_id.

    #[test]
    fn invite_to_group_sends_to_a_known_but_currently_offline_peer() {
        let alice = new_crypto("Alice");
        let carol = new_crypto("Carol");
        let carol_info = announce(&carol, "carol-peer");

        let listener = std::sync::Arc::new(TestListener::default());
        let dyn_listener: std::sync::Arc<dyn ConnectClientListener> = listener.clone();
        handle_new_peer(&alice, &dyn_listener, &carol_info); // met once, e.g. in an earlier session

        {
            let mut state = alice.lock().unwrap();
            // Carol has since gone offline: no live peer_id for her
            // anymore, but her known-peer record and outbound Olm session
            // both persist.
            state.peer_id_by_identity.remove(&carol_info.identity_key);
            state.peer_identity_keys.remove(&carol_info.peer_id);
            state.groups.insert(
                "group-1".to_string(),
                persistence::GroupMetadata {
                    name: "Family".to_string(),
                    members: vec![persistence::GroupMember {
                        identity_key: "alice-identity-key".into(),
                        display_name: "Alice".into(),
                    }],
                },
            );
        }

        let client = ConnectClient::new(temp_data_dir());
        *client.crypto.lock().unwrap() = Some(alice.clone());
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *client.outgoing.lock().unwrap() = Some(tx);
        *client.listener.lock().unwrap() = Some(dyn_listener);

        let ok = client.invite_to_group("group-1".to_string(), carol_info.identity_key.clone());
        assert!(ok, "inviting a known-but-offline peer should succeed");

        assert_eq!(
            alice.lock().unwrap().groups["group-1"].members.len(),
            2,
            "the new member should be persisted locally right away, not just sent over the wire"
        );

        match rx.try_recv() {
            Ok(ClientEvent::InviteToGroup { to_identity_key, group_id, .. }) => {
                assert_eq!(to_identity_key, carol_info.identity_key);
                assert_eq!(group_id, "group-1");
            }
            other => panic!("expected InviteToGroup, got {other:?}"),
        }
    }

    #[test]
    fn invite_to_group_ciphertext_decrypts_to_the_updated_member_list() {
        let alice = new_crypto("Alice");
        let carol = new_crypto("Carol");
        let alice_info = announce(&alice, "alice-peer");
        let carol_info = announce(&carol, "carol-peer");
        discover_each_other(&alice, &alice_info, &carol, &carol_info);

        alice.lock().unwrap().groups.insert(
            "group-1".to_string(),
            persistence::GroupMetadata {
                name: "Family".to_string(),
                members: vec![persistence::GroupMember {
                    identity_key: alice_info.identity_key.clone(),
                    display_name: "Alice".into(),
                }],
            },
        );

        let client = ConnectClient::new(temp_data_dir());
        *client.crypto.lock().unwrap() = Some(alice.clone());
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *client.outgoing.lock().unwrap() = Some(tx);
        *client.listener.lock().unwrap() =
            Some(std::sync::Arc::new(TestListener::default()) as std::sync::Arc<dyn ConnectClientListener>);

        assert!(client.invite_to_group("group-1".to_string(), carol_info.identity_key.clone()));

        let ciphertext = match rx.try_recv() {
            Ok(ClientEvent::InviteToGroup { ciphertext, .. }) => ciphertext,
            other => panic!("expected InviteToGroup, got {other:?}"),
        };

        let group_name = handle_group_invite(&carol, &alice_info.identity_key, &"group-1".to_string(), &ciphertext)
            .expect("carol should be able to decrypt the invite");
        assert_eq!(group_name, "Family");
        assert_eq!(carol.lock().unwrap().groups["group-1"].members.len(), 2);
    }

    #[test]
    fn invite_to_group_refuses_to_double_invite_an_existing_member() {
        let alice = new_crypto("Alice");
        let carol = new_crypto("Carol");
        let alice_info = announce(&alice, "alice-peer");
        let carol_info = announce(&carol, "carol-peer");
        discover_each_other(&alice, &alice_info, &carol, &carol_info);

        alice.lock().unwrap().groups.insert(
            "group-1".to_string(),
            persistence::GroupMetadata {
                name: "Family".to_string(),
                members: vec![
                    persistence::GroupMember { identity_key: alice_info.identity_key.clone(), display_name: "Alice".into() },
                    persistence::GroupMember { identity_key: carol_info.identity_key.clone(), display_name: "Carol".into() },
                ],
            },
        );

        let client = ConnectClient::new(temp_data_dir());
        *client.crypto.lock().unwrap() = Some(alice.clone());
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *client.outgoing.lock().unwrap() = Some(tx);
        *client.listener.lock().unwrap() =
            Some(std::sync::Arc::new(TestListener::default()) as std::sync::Arc<dyn ConnectClientListener>);

        let ok = client.invite_to_group("group-1".to_string(), carol_info.identity_key.clone());
        assert!(!ok, "inviting an existing member should be refused");
        assert!(rx.try_recv().is_err(), "nothing should be sent for a refused invite");
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
