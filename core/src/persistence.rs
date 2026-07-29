use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use vodozemac::olm::Account;

use crate::GroupId;

/// Deliberately unencrypted-at-rest: relies entirely on the OS-level
/// sandboxing of the directory the platform layer hands us (app support dir
/// on Apple platforms, app-private files dir on Android) rather than
/// encrypting the identity file itself. A real hardening pass would add
/// at-rest encryption (e.g. tied to platform Keychain/Keystore), not just
/// trust the directory.
const IDENTITY_FILE: &str = "identity.json";
const KNOWN_PEERS_FILE: &str = "known_peers.json";
const GROUPS_FILE: &str = "groups.json";

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

/// A group chat's persisted metadata -- who's in it and what it's called.
/// Deliberately doesn't include any Olm/Megolm session state: group
/// messages ride the same per-peer Olm sessions DMs use, and those get
/// re-established lazily (see `handle_new_peer` in client.rs) as members
/// come back online, so there's nothing crypto-related worth pickling
/// across restarts here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMember {
    pub identity_key: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMetadata {
    pub name: String,
    pub members: Vec<GroupMember>,
}

pub fn load_groups(data_dir: &str) -> HashMap<GroupId, GroupMetadata> {
    fs::read_to_string(Path::new(data_dir).join(GROUPS_FILE))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_groups(data_dir: &str, groups: &HashMap<GroupId, GroupMetadata>) {
    let _ = fs::create_dir_all(data_dir);
    if let Ok(json) = serde_json::to_string(groups) {
        let _ = fs::write(Path::new(data_dir).join(GROUPS_FILE), json);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, collision-proof scratch directory for one test -- persisted
    /// files are on real disk, so tests can't share a directory without
    /// racing each other.
    fn temp_dir(label: &str) -> String {
        std::env::temp_dir()
            .join(format!("connect-persistence-test-{label}-{}", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn format_fingerprint_groups_into_four_character_chunks() {
        assert_eq!(format_fingerprint("abcdefgh"), "abcd efgh");
        assert_eq!(format_fingerprint("abcdefg"), "abcd efg");
        assert_eq!(format_fingerprint("abc"), "abc");
        assert_eq!(format_fingerprint(""), "");
    }

    #[test]
    fn load_or_create_account_persists_identity_across_calls() {
        let dir = temp_dir("identity");
        let first = load_or_create_account(&dir);
        let second = load_or_create_account(&dir);
        assert_eq!(
            first.curve25519_key().to_base64(),
            second.curve25519_key().to_base64(),
            "reloading from the same data_dir should return the same identity"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_or_create_account_generates_distinct_identities_per_dir() {
        let dir_a = temp_dir("identity-a");
        let dir_b = temp_dir("identity-b");
        let a = load_or_create_account(&dir_a);
        let b = load_or_create_account(&dir_b);
        assert_ne!(a.curve25519_key().to_base64(), b.curve25519_key().to_base64());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn known_peers_round_trip_through_disk() {
        let dir = temp_dir("known-peers");
        let mut peers = HashMap::new();
        peers.insert("Alice".to_string(), "some-base64-identity-key".to_string());
        peers.insert("Bob".to_string(), "another-base64-identity-key".to_string());

        save_known_peers(&dir, &peers);
        assert_eq!(load_known_peers(&dir), peers);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_known_peers_defaults_to_empty_when_nothing_saved_yet() {
        let dir = temp_dir("no-peers-yet");
        assert!(load_known_peers(&dir).is_empty());
    }

    #[test]
    fn groups_round_trip_through_disk() {
        let dir = temp_dir("groups");
        let mut groups = HashMap::new();
        groups.insert(
            "group-1".to_string(),
            GroupMetadata {
                name: "Family".to_string(),
                members: vec![
                    GroupMember { identity_key: "alice-key".into(), display_name: "Alice".into() },
                    GroupMember { identity_key: "bob-key".into(), display_name: "Bob".into() },
                ],
            },
        );

        save_groups(&dir, &groups);
        let loaded = load_groups(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["group-1"].name, "Family");
        assert_eq!(loaded["group-1"].members.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_groups_defaults_to_empty_when_nothing_saved_yet() {
        let dir = temp_dir("no-groups-yet");
        assert!(load_groups(&dir).is_empty());
    }
}
