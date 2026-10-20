//! Connection passwords in the operating system's secure storage: the macOS Keychain, the
//! Windows Credential Manager, or the Secret Service (GNOME Keyring, KWallet) on Linux.
//! All passwords live in one entry (service "rusty-beaver", account "passwords", a JSON map
//! from connection id to password), read once per run: macOS asks for access once per item,
//! so one item means at most one prompt. Older versions kept one entry per connection id;
//! those move into the map when first read.
//! `RUSTY_BEAVER_KEYCHAIN=off` keeps passwords in connections.json instead (tests, automation).

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "rusty-beaver";
const ACCOUNT: &str = "passwords";

pub fn enabled() -> bool {
    std::env::var_os("RUSTY_BEAVER_KEYCHAIN").is_none_or(|v| v != "off")
}

/// The map, once read from the keychain.
static CACHE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn with_map<T>(
    f: impl FnOnce(&mut HashMap<String, String>) -> Result<T, String>,
) -> Result<T, String> {
    let mut cache = CACHE.lock().unwrap();
    if cache.is_none() {
        let map = match raw::get(ACCOUNT)? {
            Some(json) => {
                serde_json::from_str(&json).map_err(|e| format!("keychain entry: {e}"))?
            }
            None => HashMap::new(),
        };
        *cache = Some(map);
    }
    f(cache.as_mut().unwrap())
}

fn write(map: &HashMap<String, String>) -> Result<(), String> {
    if map.is_empty() {
        raw::delete(ACCOUNT);
        Ok(())
    } else {
        raw::set(ACCOUNT, &serde_json::to_string(map).unwrap())
    }
}

pub fn store(id: &str, password: &str) -> Result<(), String> {
    with_map(|m| {
        if m.get(id).map(String::as_str) == Some(password) {
            return Ok(());
        }
        m.insert(id.into(), password.into());
        write(m)
    })
}

/// The password stored for a connection; None when there is none.
pub fn load(id: &str) -> Result<Option<String>, String> {
    with_map(|m| {
        if let Some(p) = m.get(id) {
            return Ok(Some(p.clone()));
        }
        // An entry of its own, from before the single map: move it in.
        let Ok(Some(p)) = raw::get(id) else {
            return Ok(None);
        };
        m.insert(id.into(), p.clone());
        write(m)?;
        raw::delete(id);
        Ok(Some(p))
    })
}

pub fn delete(id: &str) {
    let _ = with_map(|m| {
        if m.remove(id).is_some() {
            write(m)
        } else {
            Ok(())
        }
    });
    raw::delete(id);
}

#[cfg(not(test))]
mod raw {
    use super::SERVICE;

    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, account).map_err(|e| e.to_string())
    }

    pub fn get(account: &str) -> Result<Option<String>, String> {
        match entry(account)?.get_password() {
            Ok(p) => Ok(Some(p)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(account: &str, secret: &str) -> Result<(), String> {
        entry(account)?
            .set_password(secret)
            .map_err(|e| e.to_string())
    }

    pub fn delete(account: &str) {
        if let Ok(e) = entry(account) {
            let _ = e.delete_credential();
        }
    }
}

// Tests never touch the real keychain: an in-memory store with the same behaviour.
#[cfg(test)]
pub(crate) mod raw {
    use std::collections::HashMap;
    use std::sync::Mutex;

    pub static STORE: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

    pub fn get(account: &str) -> Result<Option<String>, String> {
        let _ = super::SERVICE;
        Ok(STORE
            .lock()
            .unwrap()
            .get_or_insert_default()
            .get(account)
            .cloned())
    }

    pub fn set(account: &str, secret: &str) -> Result<(), String> {
        STORE
            .lock()
            .unwrap()
            .get_or_insert_default()
            .insert(account.into(), secret.into());
        Ok(())
    }

    pub fn delete(account: &str) {
        STORE
            .lock()
            .unwrap()
            .get_or_insert_default()
            .remove(account);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_keychain_entry_for_all_passwords() {
        raw::set("old-id", "legacy").unwrap(); // an entry of its own, from an older version
        store("x1", "one").unwrap();
        store("x2", "two").unwrap();
        assert_eq!(load("old-id").unwrap().as_deref(), Some("legacy"));
        assert_eq!(raw::get("old-id").unwrap(), None, "moved into the map");
        let map: HashMap<String, String> =
            serde_json::from_str(&raw::get(ACCOUNT).unwrap().unwrap()).unwrap();
        assert_eq!(
            (
                map["x1"].as_str(),
                map["x2"].as_str(),
                map["old-id"].as_str()
            ),
            ("one", "two", "legacy")
        );
        delete("x1");
        assert_eq!(load("x1").unwrap(), None);
        assert_eq!(load("x2").unwrap().as_deref(), Some("two"));
    }
}

/// A random id for a new connection (its keychain entry name).
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from the OS's random source: two of them give 128 random bits.
    let part = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}
