//! API keys live in Windows Credential Manager (never in files). If the OS store is
//! unavailable (development on Linux), keys are kept in memory for the session only.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::OnceLock;

const SERVICE: &str = "Wisper";

fn account(provider: &str) -> &'static str {
    match provider {
        "openai" => "openai-api-key",
        _ => "anthropic-api-key",
    }
}

fn memory() -> &'static Mutex<HashMap<String, String>> {
    static M: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn get(provider: &str) -> Option<String> {
    if let Some(v) = memory().lock().get(provider) {
        return Some(v.clone());
    }
    let entry = keyring::Entry::new(SERVICE, account(provider)).ok()?;
    let v = entry.get_password().ok()?;
    let v = v.trim().to_string();
    (!v.is_empty()).then_some(v)
}

pub fn set(provider: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    let entry = keyring::Entry::new(SERVICE, account(provider)).map_err(|e| e.to_string());
    let result = match entry {
        Ok(e) if key.is_empty() => match e.delete_credential() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        },
        Ok(e) => e.set_password(key).map_err(|e| e.to_string()),
        Err(e) => Err(e),
    };
    let mut m = memory().lock();
    if result.is_err() {
        tracing::warn!("credential store unavailable; keeping the key in memory only");
    }
    if key.is_empty() {
        m.remove(provider);
    } else if result.is_err() {
        m.insert(provider.to_string(), key.to_string());
    } else {
        m.remove(provider);
    }
    Ok(())
}
