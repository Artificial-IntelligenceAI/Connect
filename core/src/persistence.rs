use std::collections::HashMap;
use std::fs;
use std::path::Path;

use vodozemac::olm::Account;

/// Deliberately unencrypted-at-rest: relies entirely on the OS-level
/// sandboxing of the directory the platform layer hands us (app support dir
/// on Apple platforms, app-private files dir on Android) rather than
/// encrypting the identity file itself. A real hardening pass would add
/// at-rest encryption (e.g. tied to platform Keychain/Keystore), not just
/// trust the directory.
const IDENTITY_FILE: &str = "identity.json";
const KNOWN_PEERS_FILE: &str = "known_peers.json";

pub fn load_or_create_account(data_dir: &str) -> Account {
    let path = Path::new(data_dir).join(IDENTITY_FILE);
    if let Ok(contents) = fs::read_to_string(&path) {
        if let Ok(pickle) = serde_json::from_str(&contents) {
            return Account::from_pickle(pickle);
        }
    }
    let account = Account::new();
    save_account(data_dir, &account);
    account
}

pub fn save_account(data_dir: &str, account: &Account) {
    let _ = fs::create_dir_all(data_dir);
    if let Ok(json) = serde_json::to_string(&account.pickle()) {
        let _ = fs::write(Path::new(data_dir).join(IDENTITY_FILE), json);
    }
}

/// Trust-on-first-use store: display_name -> base64 Curve25519 identity key,
/// last seen. Keyed by display name because that's the only stable-ish
/// identifier we have in a server with no real accounts -- a peer changing
/// their display name looks like a new contact, not a key change, since
/// there's nothing else to anchor identity to.
pub fn load_known_peers(data_dir: &str) -> HashMap<String, String> {
    fs::read_to_string(Path::new(data_dir).join(KNOWN_PEERS_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_known_peers(data_dir: &str, peers: &HashMap<String, String>) {
    let _ = fs::create_dir_all(data_dir);
    if let Ok(json) = serde_json::to_string(peers) {
        let _ = fs::write(Path::new(data_dir).join(KNOWN_PEERS_FILE), json);
    }
}

/// A short, human-comparable rendering of a base64 key -- not a hash, just
/// the key itself grouped for readability. Good enough for a v1 "does this
/// match what my contact shows on their screen" comparison; real safety
/// numbers (Signal/Matrix style) hash+combine both parties' keys instead.
pub fn format_fingerprint(base64_key: &str) -> String {
    base64_key
        .as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}
