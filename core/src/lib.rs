use serde::{Deserialize, Serialize};

/// Messages sent from a client to the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    Join { display_name: String },
    Message { text: String },
}

/// Messages sent from the server to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Message {
        from: String,
        text: String,
        timestamp_ms: u64,
    },
    SystemNotice {
        text: String,
    },
}
