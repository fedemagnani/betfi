//! `--account-snapshot`: boot an account from a saved Pascal account state (the mode-R demo
//! boots the mock from the real hedger's account, spec section 9 step 3).
//!
//! Accepted files, all as Pascal serves them:
//!
//! - a `GET /api/v1/accounts/{owner}` response (`{"status":"success","data":{…},…}`);
//! - an `account` WebSocket snapshot frame (`{"channel":"account","owner":…,"data":{…}}`);
//! - a bare `AccountState` object.
//!
//! The REST response does not name its owner, so the argument is `<owner>=<file>`; the owner
//! may be left out when the file is a WebSocket frame (it carries `owner`).

use std::path::{Path, PathBuf};

use pascal_rs::AccountState;
use serde_json::Value;

use crate::requests::is_pubkey;

/// An account state to load at boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSnapshot {
    /// Account owner (base58).
    pub owner: String,
    /// The state.
    pub state: AccountState,
}

/// Why a snapshot could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// File read failure.
    #[error("reading {0}: {1}")]
    Io(String, std::io::Error),
    /// Not JSON or not an account state.
    #[error("{0}: {1}")]
    Decode(String, String),
    /// No owner given and none in the file.
    #[error("{0}: the file does not name its owner; pass <owner>=<file>")]
    NoOwner(String),
    /// Bad owner.
    #[error("{0:?} is not a base58 public key")]
    Owner(String),
}

impl AccountSnapshot {
    /// Parses `[<owner>=]<file>` and loads the file.
    pub fn load_spec(spec: &str) -> Result<AccountSnapshot, SnapshotError> {
        match spec.split_once('=') {
            Some((owner, path)) if is_pubkey(owner) => {
                Self::load(Path::new(path), Some(owner.to_owned()))
            }
            _ => Self::load(&PathBuf::from(spec), None),
        }
    }

    /// Loads `path`; `owner` overrides the owner named in the file.
    pub fn load(path: &Path, owner: Option<String>) -> Result<AccountSnapshot, SnapshotError> {
        let name = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|e| SnapshotError::Io(name.clone(), e))?;
        let v: Value = serde_json::from_str(&text)
            .map_err(|e| SnapshotError::Decode(name.clone(), e.to_string()))?;
        Self::from_value(&name, v, owner)
    }

    /// Decodes an already parsed file (see the module docs for the accepted shapes).
    pub fn from_value(
        name: &str,
        v: Value,
        owner: Option<String>,
    ) -> Result<AccountSnapshot, SnapshotError> {
        let named = v.get("owner").and_then(Value::as_str).map(str::to_owned);
        if v.get("status").and_then(Value::as_str) == Some("error") {
            return Err(SnapshotError::Decode(
                name.to_owned(),
                "the saved response is an error envelope".into(),
            ));
        }
        let wrapped = v.get("status").is_some() || v.get("channel").is_some();
        let data = if wrapped {
            v.get("data").cloned().unwrap_or(Value::Null)
        } else {
            v
        };
        let state: AccountState = serde_json::from_value(data)
            .map_err(|e| SnapshotError::Decode(name.to_owned(), e.to_string()))?;
        let owner = owner
            .or(named)
            .ok_or_else(|| SnapshotError::NoOwner(name.to_owned()))?;
        if !is_pubkey(&owner) {
            return Err(SnapshotError::Owner(owner));
        }
        Ok(AccountSnapshot { owner, state })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/pascal")
            .join(name)
    }

    #[test]
    fn loads_rest_and_ws_shapes() {
        const MM: &str = "EWGv5fY5VRNjjGDV7EkthQTnGSh2g94vqRhRVhomgBmR";
        let s = AccountSnapshot::load_spec(&format!(
            "{MM}={}",
            fixture("rest_accounts_owner_mm.json").display()
        ))
        .unwrap();
        assert_eq!(s.owner, MM);
        assert_eq!(s.state.positions.len(), 19);

        let s =
            AccountSnapshot::load_spec(&fixture("ws_account_snapshot.json").display().to_string())
                .unwrap();
        assert_eq!(s.owner, "7ATdPM8uGMu3qiRSCpc6hUrSZ5VxFSUyYLfRipABG5bC");
        assert!(!s.state.positions.is_empty());

        assert!(matches!(
            AccountSnapshot::load_spec(
                &fixture("rest_accounts_owner_mm.json").display().to_string()
            ),
            Err(SnapshotError::NoOwner(_))
        ));
        assert!(matches!(
            AccountSnapshot::load_spec(
                &fixture("rest_error_invalid_request.json")
                    .display()
                    .to_string()
            ),
            Err(SnapshotError::Decode(..))
        ));
        assert!(matches!(
            AccountSnapshot::load_spec("/nonexistent/x.json"),
            Err(SnapshotError::Io(..))
        ));
    }
}
