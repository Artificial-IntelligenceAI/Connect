use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use messaging_core::{ClientEvent, GroupId, PeerId, PeerInfo, ServerEvent};
use tokio::sync::mpsc;
use uuid::Uuid;

struct PeerHandle {
    info: PeerInfo,
    sender: mpsc::UnboundedSender<ServerEvent>,
}

/// A `GroupInvite` addressed to an identity that wasn't reachable when it
/// was sent -- held until that identity's owner next joins. Lives only in
/// memory for this server process's lifetime: like everything else here,
/// restarting the relay drops anything still pending, the same as it drops
/// every other bit of state.
struct PendingInvite {
    from_identity_key: String,
    group_id: GroupId,
    ciphertext: String,
}

#[derive(Clone, Default)]
struct AppState {
    peers: Arc<Mutex<HashMap<PeerId, PeerHandle>>>,
    /// to_identity_key -> queued invites, delivered the moment that
    /// identity's owner sends `Join`.
    pending_invites: Arc<Mutex<HashMap<String, Vec<PendingInvite>>>>,
}

impl AppState {
    /// Send to every connected peer except `except` -- used for peer
    /// discovery (`PeerJoined`/`PeerLeft`), not chat content.
    fn broadcast_except(&self, except: &PeerId, event: ServerEvent) {
        let peers = self.peers.lock().unwrap();
        for (id, peer) in peers.iter() {
            if id != except {
                let _ = peer.sender.send(event.clone());
            }
        }
    }

    /// Relay to exactly one named peer, if they're still connected. Used
    /// for `DirectMessage`/`GroupInvite`/`GroupMessage` -- the server
    /// never has the keys to read any of it, only who it's addressed to.
    fn send_to(&self, to: &PeerId, event: ServerEvent) {
        if let Some(peer) = self.peers.lock().unwrap().get(to) {
            let _ = peer.sender.send(event);
        }
    }

    /// Find the live peer_id currently registered for `identity_key`, if
    /// any -- a linear scan, which is fine at this app's expected
    /// (LAN-party-sized) peer counts.
    fn peer_id_for_identity(&self, identity_key: &str) -> Option<PeerId> {
        self.peers
            .lock()
            .unwrap()
            .values()
            .find(|p| p.info.identity_key == identity_key)
            .map(|p| p.info.peer_id.clone())
    }
}

fn app() -> Router {
    Router::new()
        .route("/ws", get(ws_handler))
        .with_state(AppState::default())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let addr = SocketAddr::from(([0, 0, 0, 0], 7878));
    tracing::info!("messaging-server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app()).await.unwrap();
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let peer_id: PeerId = Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerEvent>();

    let mut send_task = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            let Ok(json) = serde_json::to_string(&event) else {
                continue;
            };
            if ws_sender.send(Message::Text(json)).await.is_err() {
                break;
            }
        }
    });

    let state2 = state.clone();
    let peer_id2 = peer_id.clone();
    let tx2 = tx.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_receiver.next().await {
            let Message::Text(text) = msg else { continue };
            let Ok(event) = serde_json::from_str::<ClientEvent>(&text) else {
                continue;
            };

            match event {
                ClientEvent::Join {
                    display_name,
                    identity_key,
                    one_time_key,
                } => {
                    tracing::info!("{display_name} joined");
                    let info = PeerInfo {
                        peer_id: peer_id2.clone(),
                        display_name,
                        identity_key,
                        one_time_key,
                    };

                    let roster: Vec<PeerInfo> = {
                        let peers = state2.peers.lock().unwrap();
                        peers.values().map(|p| p.info.clone()).collect()
                    };
                    let _ = tx2.send(ServerEvent::Roster { peers: roster });

                    state2.peers.lock().unwrap().insert(
                        peer_id2.clone(),
                        PeerHandle {
                            info: info.clone(),
                            sender: tx2.clone(),
                        },
                    );

                    let pending = state2.pending_invites.lock().unwrap().remove(&info.identity_key);

                    state2.broadcast_except(&peer_id2, ServerEvent::PeerJoined { peer: info });

                    if let Some(pending) = pending {
                        for invite in pending {
                            let _ = tx2.send(ServerEvent::InviteToGroup {
                                from_identity_key: invite.from_identity_key,
                                group_id: invite.group_id,
                                ciphertext: invite.ciphertext,
                            });
                        }
                    }
                }
                ClientEvent::DirectMessage { to, ciphertext } => {
                    state2.send_to(
                        &to,
                        ServerEvent::DirectMessage { from: peer_id2.clone(), ciphertext },
                    );
                }
                ClientEvent::GroupInvite { to, group_id, ciphertext } => {
                    state2.send_to(
                        &to,
                        ServerEvent::GroupInvite { from: peer_id2.clone(), group_id, ciphertext },
                    );
                }
                ClientEvent::GroupMessage { to, group_id, ciphertext } => {
                    state2.send_to(
                        &to,
                        ServerEvent::GroupMessage { from: peer_id2.clone(), group_id, ciphertext },
                    );
                }
                ClientEvent::InviteToGroup { to_identity_key, group_id, ciphertext } => {
                    let from_identity_key = state2
                        .peers
                        .lock()
                        .unwrap()
                        .get(&peer_id2)
                        .map(|p| p.info.identity_key.clone());
                    let Some(from_identity_key) = from_identity_key else { continue };

                    match state2.peer_id_for_identity(&to_identity_key) {
                        Some(to_peer_id) => state2.send_to(
                            &to_peer_id,
                            ServerEvent::InviteToGroup { from_identity_key, group_id, ciphertext },
                        ),
                        None => state2.pending_invites.lock().unwrap().entry(to_identity_key).or_default().push(
                            PendingInvite { from_identity_key, group_id, ciphertext },
                        ),
                    }
                }
            }
        }
    });

    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }

    if state.peers.lock().unwrap().remove(&peer_id).is_some() {
        state.broadcast_except(&peer_id, ServerEvent::PeerLeft { peer_id: peer_id.clone() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    fn dummy_peer_info(id: &str) -> PeerInfo {
        PeerInfo {
            peer_id: id.to_string(),
            display_name: id.to_string(),
            identity_key: format!("{id}-identity-key"),
            one_time_key: format!("{id}-otk"),
        }
    }

    #[test]
    fn broadcast_except_skips_the_excluded_peer_and_reaches_everyone_else() {
        let state = AppState::default();
        let (a_tx, mut a_rx) = mpsc::unbounded_channel();
        let (b_tx, mut b_rx) = mpsc::unbounded_channel();
        let (c_tx, mut c_rx) = mpsc::unbounded_channel();
        {
            let mut peers = state.peers.lock().unwrap();
            peers.insert("a".into(), PeerHandle { info: dummy_peer_info("a"), sender: a_tx });
            peers.insert("b".into(), PeerHandle { info: dummy_peer_info("b"), sender: b_tx });
            peers.insert("c".into(), PeerHandle { info: dummy_peer_info("c"), sender: c_tx });
        }

        state.broadcast_except(&"b".to_string(), ServerEvent::PeerLeft { peer_id: "someone".into() });

        assert!(a_rx.try_recv().is_ok(), "non-excluded peer a should receive the broadcast");
        assert!(b_rx.try_recv().is_err(), "the excluded peer b should not receive its own broadcast");
        assert!(c_rx.try_recv().is_ok(), "non-excluded peer c should receive the broadcast");
    }

    // -- Real end-to-end WebSocket integration tests ----------------------
    //
    // These run the actual axum app against a real TCP listener on an
    // ephemeral port and drive it with real tokio-tungstenite clients --
    // exercising the full join/roster/broadcast/key-exchange/disconnect
    // wire protocol, not just handler logic in isolation.

    type TestWs = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    async fn spawn_test_server() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, app()).await.unwrap();
        });
        port
    }

    async fn connect(port: u16) -> TestWs {
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/ws"))
            .await
            .expect("failed to connect to the test server");
        ws
    }

    fn join(display_name: &str) -> ClientEvent {
        ClientEvent::Join {
            display_name: display_name.to_string(),
            identity_key: format!("{display_name}-identity-key"),
            one_time_key: format!("{display_name}-otk"),
        }
    }

    async fn send(ws: &mut TestWs, event: ClientEvent) {
        let json = serde_json::to_string(&event).unwrap();
        ws.send(WsMessage::Text(json.into())).await.unwrap();
    }

    /// Reads the next text frame and decodes it as a `ServerEvent`, bounded
    /// by a timeout so a wrong-order expectation fails the test instead of
    /// hanging a stuck run forever.
    async fn recv(ws: &mut TestWs) -> ServerEvent {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let msg = ws
                    .next()
                    .await
                    .expect("stream ended unexpectedly")
                    .expect("websocket error");
                if let WsMessage::Text(text) = msg {
                    return serde_json::from_str(&text).unwrap();
                }
            }
        })
        .await
        .expect("timed out waiting for a server event")
    }

    async fn assert_silent(ws: &mut TestWs, within: Duration) {
        let result = tokio::time::timeout(within, ws.next()).await;
        assert!(result.is_err(), "expected no message to arrive, but one did");
    }

    #[tokio::test]
    async fn joining_alone_gets_an_empty_roster() {
        let port = spawn_test_server().await;
        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;

        match recv(&mut alice).await {
            ServerEvent::Roster { peers } => assert!(peers.is_empty()),
            other => panic!("expected Roster, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn second_joiner_sees_the_first_and_the_first_is_notified() {
        let port = spawn_test_server().await;
        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;
        recv(&mut alice).await; // her own (empty) roster

        let mut bob = connect(port).await;
        send(&mut bob, join("Bob")).await;
        match recv(&mut bob).await {
            ServerEvent::Roster { peers } => {
                assert_eq!(peers.len(), 1);
                assert_eq!(peers[0].display_name, "Alice");
            }
            other => panic!("expected Roster, got {other:?}"),
        }

        match recv(&mut alice).await {
            ServerEvent::PeerJoined { peer } => assert_eq!(peer.display_name, "Bob"),
            other => panic!("expected PeerJoined, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn direct_message_is_delivered_only_to_its_target() {
        let port = spawn_test_server().await;

        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;
        recv(&mut alice).await; // roster

        let mut bob = connect(port).await;
        send(&mut bob, join("Bob")).await;
        let alice_id = match recv(&mut bob).await {
            ServerEvent::Roster { peers } => peers[0].peer_id.clone(),
            other => panic!("expected Roster, got {other:?}"),
        };
        recv(&mut alice).await; // PeerJoined(bob)

        let mut carol = connect(port).await; // bystander, not addressed
        send(&mut carol, join("Carol")).await;
        recv(&mut carol).await; // roster with alice+bob
        recv(&mut alice).await; // PeerJoined(carol)
        recv(&mut bob).await; // PeerJoined(carol)

        send(&mut bob, ClientEvent::DirectMessage { to: alice_id, ciphertext: "dm-xyz".into() }).await;

        match recv(&mut alice).await {
            ServerEvent::DirectMessage { ciphertext, .. } => assert_eq!(ciphertext, "dm-xyz"),
            other => panic!("expected DirectMessage, got {other:?}"),
        }
        assert_silent(&mut carol, Duration::from_millis(300)).await;
        assert_silent(&mut bob, Duration::from_millis(300)).await; // no echo to the sender
    }

    #[tokio::test]
    async fn group_invite_and_messages_reach_only_the_intended_recipient() {
        let port = spawn_test_server().await;

        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;
        recv(&mut alice).await; // roster

        let mut bob = connect(port).await;
        send(&mut bob, join("Bob")).await;
        let alice_id = match recv(&mut bob).await {
            ServerEvent::Roster { peers } => peers[0].peer_id.clone(),
            other => panic!("expected Roster, got {other:?}"),
        };
        recv(&mut alice).await; // PeerJoined(bob)

        let mut carol = connect(port).await; // never invited to the group
        send(&mut carol, join("Carol")).await;
        recv(&mut carol).await; // roster with alice+bob
        recv(&mut alice).await; // PeerJoined(carol)
        recv(&mut bob).await; // PeerJoined(carol)

        let group_id = "group-1".to_string();
        send(
            &mut bob,
            ClientEvent::GroupInvite { to: alice_id.clone(), group_id: group_id.clone(), ciphertext: "invite-xyz".into() },
        )
        .await;
        match recv(&mut alice).await {
            ServerEvent::GroupInvite { group_id: received_id, ciphertext, .. } => {
                assert_eq!(received_id, group_id);
                assert_eq!(ciphertext, "invite-xyz");
            }
            other => panic!("expected GroupInvite, got {other:?}"),
        }
        assert_silent(&mut carol, Duration::from_millis(300)).await;

        send(
            &mut bob,
            ClientEvent::GroupMessage { to: alice_id, group_id: group_id.clone(), ciphertext: "group-msg-xyz".into() },
        )
        .await;
        match recv(&mut alice).await {
            ServerEvent::GroupMessage { group_id: received_id, ciphertext, .. } => {
                assert_eq!(received_id, group_id);
                assert_eq!(ciphertext, "group-msg-xyz");
            }
            other => panic!("expected GroupMessage, got {other:?}"),
        }
        assert_silent(&mut carol, Duration::from_millis(300)).await;
        assert_silent(&mut bob, Duration::from_millis(300)).await; // no echo to the sender
    }

    #[tokio::test]
    async fn invite_to_group_delivers_immediately_when_the_target_is_online() {
        let port = spawn_test_server().await;

        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;
        recv(&mut alice).await; // roster

        let mut bob = connect(port).await;
        send(&mut bob, join("Bob")).await;
        recv(&mut bob).await; // roster
        recv(&mut alice).await; // PeerJoined(bob)

        send(
            &mut alice,
            ClientEvent::InviteToGroup {
                to_identity_key: "Bob-identity-key".into(),
                group_id: "group-1".into(),
                ciphertext: "invite-xyz".into(),
            },
        )
        .await;

        match recv(&mut bob).await {
            ServerEvent::InviteToGroup { from_identity_key, group_id, ciphertext } => {
                assert_eq!(from_identity_key, "Alice-identity-key");
                assert_eq!(group_id, "group-1");
                assert_eq!(ciphertext, "invite-xyz");
            }
            other => panic!("expected InviteToGroup, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invite_to_group_is_held_and_delivered_once_the_offline_target_joins() {
        let port = spawn_test_server().await;

        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;
        recv(&mut alice).await; // roster

        // Bob is not connected at all yet -- Alice invites him anyway.
        send(
            &mut alice,
            ClientEvent::InviteToGroup {
                to_identity_key: "Bob-identity-key".into(),
                group_id: "group-1".into(),
                ciphertext: "invite-xyz".into(),
            },
        )
        .await;

        let mut bob = connect(port).await;
        send(&mut bob, join("Bob")).await;
        recv(&mut bob).await; // roster (empty for Bob, but arrives first)

        match recv(&mut bob).await {
            ServerEvent::InviteToGroup { from_identity_key, group_id, ciphertext } => {
                assert_eq!(from_identity_key, "Alice-identity-key");
                assert_eq!(group_id, "group-1");
                assert_eq!(ciphertext, "invite-xyz");
            }
            other => panic!("expected InviteToGroup, got {other:?}"),
        }

        // Delivered exactly once -- a second, unrelated joiner shouldn't
        // also receive Bob's already-flushed invite.
        let mut carol = connect(port).await;
        send(&mut carol, join("Carol")).await;
        recv(&mut carol).await; // roster
        recv(&mut alice).await; // PeerJoined(bob)
        recv(&mut alice).await; // PeerJoined(carol)
        recv(&mut bob).await; // PeerJoined(carol)
        assert_silent(&mut carol, Duration::from_millis(300)).await;
    }

    #[tokio::test]
    async fn disconnecting_broadcasts_peer_left_to_the_room() {
        let port = spawn_test_server().await;

        let mut alice = connect(port).await;
        send(&mut alice, join("Alice")).await;
        recv(&mut alice).await; // roster

        let mut bob = connect(port).await;
        send(&mut bob, join("Bob")).await;
        let alice_id = match recv(&mut bob).await {
            ServerEvent::Roster { peers } => peers[0].peer_id.clone(),
            other => panic!("expected Roster, got {other:?}"),
        };
        recv(&mut alice).await; // PeerJoined(bob)

        drop(alice); // simulate Alice's connection dropping

        match recv(&mut bob).await {
            ServerEvent::PeerLeft { peer_id } => assert_eq!(peer_id, alice_id),
            other => panic!("expected PeerLeft, got {other:?}"),
        }
    }
}
