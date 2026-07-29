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
use messaging_core::{ClientEvent, PeerId, PeerInfo, ServerEvent};
use tokio::sync::mpsc;
use uuid::Uuid;

struct PeerHandle {
    info: PeerInfo,
    sender: mpsc::UnboundedSender<ServerEvent>,
}

#[derive(Clone, Default)]
struct AppState {
    peers: Arc<Mutex<HashMap<PeerId, PeerHandle>>>,
}

impl AppState {
    /// Send to every connected peer except `except`. The server relays
    /// ciphertext only -- it never has the keys to read `Message`/
    /// `KeyExchange` content, only who's in the room.
    fn broadcast_except(&self, except: &PeerId, event: ServerEvent) {
        let peers = self.peers.lock().unwrap();
        for (id, peer) in peers.iter() {
            if id != except {
                let _ = peer.sender.send(event.clone());
            }
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let state = AppState::default();

    let app = Router::new()
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 7878));
    tracing::info!("messaging-server listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
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

                    state2.broadcast_except(&peer_id2, ServerEvent::PeerJoined { peer: info });
                }
                ClientEvent::Message { ciphertext } => {
                    state2.broadcast_except(
                        &peer_id2,
                        ServerEvent::Message {
                            from: peer_id2.clone(),
                            ciphertext,
                        },
                    );
                }
                ClientEvent::KeyExchange { to, ciphertext } => {
                    let target = state2.peers.lock().unwrap().get(&to).map(|p| p.sender.clone());
                    if let Some(target) = target {
                        let _ = target.send(ServerEvent::KeyExchange {
                            from: peer_id2.clone(),
                            ciphertext,
                        });
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
