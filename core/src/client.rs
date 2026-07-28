use std::sync::{Mutex, OnceLock};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::{ClientEvent, ServerEvent};

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

/// A single chat message or system notice, ready for display.
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

/// A LAN relay client: connects over WebSocket, speaks the join/message/
/// system_notice JSON protocol, and reports events back through a listener.
#[derive(uniffi::Object)]
pub struct ConnectClient {
    outgoing: Mutex<Option<mpsc::UnboundedSender<ClientEvent>>>,
}

#[uniffi::export]
impl ConnectClient {
    #[uniffi::constructor]
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            outgoing: Mutex::new(None),
        })
    }

    pub fn connect(&self, host: String, port: u16, display_name: String, listener: std::sync::Arc<dyn ConnectClientListener>) {
        let (tx, mut rx) = mpsc::unbounded_channel::<ClientEvent>();
        *self.outgoing.lock().unwrap() = Some(tx);

        listener.on_state_changed(ConnectionState::Connecting);

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

            let join = ClientEvent::Join { display_name };
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
            let recv_task = tokio::spawn(async move {
                while let Some(Ok(msg)) = read.next().await {
                    let WsMessage::Text(text) = msg else { continue };
                    let Ok(event) = serde_json::from_str::<ServerEvent>(&text) else {
                        continue;
                    };
                    let message = match event {
                        ServerEvent::Message { from, text, .. } => ChatMessage {
                            from,
                            text,
                            is_system: false,
                        },
                        ServerEvent::SystemNotice { text } => ChatMessage {
                            from: String::new(),
                            text,
                            is_system: true,
                        },
                    };
                    recv_listener.on_message(message);
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
        if let Some(tx) = self.outgoing.lock().unwrap().as_ref() {
            let _ = tx.send(ClientEvent::Message { text });
        }
    }

    pub fn disconnect(&self) {
        *self.outgoing.lock().unwrap() = None;
    }
}
