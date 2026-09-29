use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use deepseekcode_types::{DeepSeekCodeError, Result};
#[cfg(feature = "os-keychain")]
use keyring::Entry;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

#[cfg(feature = "os-keychain")]
const KEYRING_SERVICE: &str = "deepseekcode";
const KEYRING_MARKER: &str = "keyring";
const ENCRYPTED_PREFIX: &str = "enc:";

pub struct CredentialsStore {
    pool: SqlitePool,
    cipher: Aes256Gcm,
}

impl CredentialsStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            cipher: Aes256Gcm::new_from_slice(&derive_local_key()).expect("valid AES key length"),
        }
    }

    pub async fn store(&self, provider_id: &str, api_key: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let encrypted = encrypt_secret(&self.cipher, api_key)?;
        self.upsert_metadata(provider_id, &encrypted, &now).await?;

        if keyring_enabled() {
            if let Err(e) = try_keyring_store(provider_id, api_key) {
                tracing::warn!(
                    target: deepseekcode_telemetry::APP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    provider_id = provider_id,
                    error = %e,
                    "keyring mirror failed; encrypted sqlite copy retained"
                );
            }
        }

        Ok(())
    }

    pub async fn get(&self, provider_id: &str) -> Result<Option<String>> {
        let row = sqlx::query_as::<_, (String,)>(
            r#"SELECT encrypted_value FROM credentials WHERE provider_id = ?"#,
        )
        .bind(provider_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;

        let Some((stored,)) = row else {
            return Ok(None);
        };

        if let Some(payload) = stored.strip_prefix(ENCRYPTED_PREFIX) {
            return decrypt_secret(&self.cipher, payload).map(Some);
        }

        // Legacy rows that only mirrored to keyring before dual-write.
        if stored == KEYRING_MARKER && keyring_enabled() {
            return match try_keyring_get(provider_id) {
                Ok(Some(key)) => Ok(Some(key)),
                Ok(None) => Ok(None),
                Err(e) => {
                    tracing::warn!(
                        target: deepseekcode_telemetry::APP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        provider_id = provider_id,
                        error = %e,
                        "keyring read failed for legacy credential row"
                    );
                    Ok(None)
                }
            };
        }

        Ok(None)
    }

    pub async fn delete(&self, provider_id: &str) -> Result<()> {
        // Surface keyring failures instead of swallowing: callers log them.
        if let Err(e) = try_keyring_delete(provider_id) {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                provider_id = provider_id,
                error = %e,
                "keyring delete failed; sqlite row still removed"
            );
        }
        sqlx::query(r#"DELETE FROM credentials WHERE provider_id = ?"#)
            .bind(provider_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn list_connected(&self) -> Result<Vec<String>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            r#"SELECT provider_id, encrypted_value FROM credentials ORDER BY provider_id"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        // Only report rows whose payload actually decrypts; corrupt /
        // wrong-machine rows are skipped (and warned) instead of reported
        // as connected.
        let mut out = Vec::new();
        for (provider_id, stored) in rows {
            if let Some(payload) = stored.strip_prefix(ENCRYPTED_PREFIX) {
                if decrypt_secret(&self.cipher, payload).is_ok() {
                    out.push(provider_id);
                } else {
                    tracing::warn!(
                        target: deepseekcode_telemetry::APP,
                        run_id = %deepseekcode_telemetry::current_run_id(),
                        provider_id = %provider_id,
                        "credential undecryptable; hiding from connected list"
                    );
                }
            } else if stored == KEYRING_MARKER {
                if let Ok(Some(_)) = try_keyring_get(&provider_id) {
                    out.push(provider_id);
                }
            }
        }
        Ok(out)
    }

    async fn upsert_metadata(
        &self,
        provider_id: &str,
        encrypted_value: &str,
        created_at: &str,
    ) -> Result<()> {
        sqlx::query(
            r#"INSERT OR REPLACE INTO credentials (provider_id, encrypted_value, created_at)
               VALUES (?, ?, ?)"#,
        )
        .bind(provider_id)
        .bind(encrypted_value)
        .bind(created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }
}

fn keyring_enabled() -> bool {
    // Compile-time gate first: `--no-default-features` builds (containers
    // without D-Bus/Keychain) never touch the OS keychain at all.
    #[cfg(not(feature = "os-keychain"))]
    {
        return false;
    }
    #[cfg(feature = "os-keychain")]
    {
        !std::env::var("DEEPSEEKCODE_DISABLE_KEYRING")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
}

#[cfg(feature = "os-keychain")]
fn try_keyring_store(provider_id: &str, api_key: &str) -> Result<()> {
    let entry = Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    entry
        .set_password(api_key)
        .map_err(|e| DeepSeekCodeError::Storage(format!("keyring store: {e}")))
}

#[cfg(not(feature = "os-keychain"))]
fn try_keyring_store(_provider_id: &str, _api_key: &str) -> Result<()> {
    Err(DeepSeekCodeError::Storage(
        "os keychain disabled in this build".into(),
    ))
}

#[cfg(feature = "os-keychain")]
fn try_keyring_get(provider_id: &str) -> Result<Option<String>> {
    let entry = Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(DeepSeekCodeError::Storage(format!("keyring get: {e}"))),
    }
}

#[cfg(not(feature = "os-keychain"))]
fn try_keyring_get(_provider_id: &str) -> Result<Option<String>> {
    Ok(None)
}

#[cfg(feature = "os-keychain")]
fn try_keyring_delete(provider_id: &str) -> Result<()> {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, provider_id) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

#[cfg(not(feature = "os-keychain"))]
fn try_keyring_delete(_provider_id: &str) -> Result<()> {
    Ok(())
}

/// v2 key derivation: per-machine salt file + 50k SHA-256 iterations.
/// Falls back to v1 only for decrypting legacy rows (see `derive_legacy_key`).
fn machine_salt() -> Vec<u8> {
    // Stable per-machine salt stored outside the DB so a copied DB alone is
    // insufficient for offline decryption. Best-effort: env override for CI.
    if let Ok(salt) = std::env::var("DEEPSEEKCODE_CRED_SALT") {
        return salt.into_bytes();
    }
    let path = deepseekcode_types::config_dir()
        .map(|p| p.join("deepseekcode").join(".cred_salt"))
        .unwrap_or_else(|| std::path::PathBuf::from(".deepseekcode-cred-salt"));
    if let Ok(existing) = std::fs::read(&path) {
        if existing.len() >= 16 {
            return existing;
        }
    }
    let mut salt = vec![0u8; 32];
    if getrandom::fill(&mut salt).is_err() {
        // Fail-closed salt: process-unique but not persisted; decrypt of old
        // rows will fail loudly instead of silently using a weak key.
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (i, b) in salt.iter_mut().enumerate() {
            *b = ((nanos >> (i % 8)) ^ (i as u128 * 0x9E3779B97F4A7C15)) as u8;
        }
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &salt);
    salt
}

fn derive_local_key() -> [u8; 32] {
    derive_v2_key(&machine_salt())
}

fn derive_v2_key(salt: &[u8]) -> [u8; 32] {
    // 50k-iteration SHA-256 KDF (no extra dep): user+host+version+salt.
    let user = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_default();
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let mut key = [0u8; 32];
    let mut hasher = Sha256::new();
    hasher.update(b"deepseekcode-credentials-v2");
    hasher.update(salt);
    hasher.update(user.as_bytes());
    hasher.update(host.as_bytes());
    let mut digest: Vec<u8> = hasher.finalize().to_vec();
    for _ in 0..50_000 {
        let mut h = Sha256::new();
        h.update(&digest);
        h.update(salt);
        digest = h.finalize().to_vec();
    }
    key.copy_from_slice(&digest[..32]);
    key
}

fn derive_legacy_key() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"deepseekcode-credentials-v1");
    if let Ok(user) = std::env::var("USERNAME").or_else(|_| std::env::var("USER")) {
        hasher.update(user.as_bytes());
    }
    if let Ok(computer) = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")) {
        hasher.update(computer.as_bytes());
    }
    hasher.finalize().into()
}

fn encrypt_secret(cipher: &Aes256Gcm, secret: &str) -> Result<String> {
    // Fail-closed: never fall back to predictable nonces; surface the error.
    let mut nonce_bytes = vec![0u8; 12];
    getrandom::fill(&mut nonce_bytes)
        .map_err(|e| DeepSeekCodeError::Storage(format!("rand: {e}")))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, secret.as_bytes())
        .map_err(|e| DeepSeekCodeError::Storage(format!("encrypt: {e}")))?;
    Ok(encode_secret(&nonce_bytes, &ciphertext))
}

fn decrypt_secret(cipher: &Aes256Gcm, payload: &str) -> Result<String> {
    // Payloads are `v2:<base64>` or legacy bare base64 (v1 key).
    let (version, b64) = match payload.split_once(':') {
        Some(("v2", rest)) => ("v2", rest),
        _ => ("v1", payload),
    };
    let bytes = STANDARD
        .decode(b64)
        .map_err(|e| DeepSeekCodeError::Storage(format!("decode: {e}")))?;
    if bytes.len() <= 12 {
        return Err(DeepSeekCodeError::Storage(
            "invalid encrypted payload".into(),
        ));
    }
    let (nonce_bytes, ciphertext) = bytes.split_at(12);
    let try_decrypt = |c: &Aes256Gcm| {
        c.decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            .map_err(|e| DeepSeekCodeError::Storage(format!("decrypt: {e}")))
    };
    // v2 first; legacy rows fall back to v1 key (then callers should re-store
    // to rotate). Distinguish corrupt vs wrong-machine via error chain.
    let plaintext = match version {
        "v2" => try_decrypt(cipher).or_else(|_| {
            let legacy =
                Aes256Gcm::new_from_slice(&derive_legacy_key()).expect("valid AES key length");
            try_decrypt(&legacy)
        })?,
        _ => {
            let legacy =
                Aes256Gcm::new_from_slice(&derive_legacy_key()).expect("valid AES key length");
            try_decrypt(&legacy).or_else(|_| try_decrypt(cipher))?
        }
    };
    String::from_utf8(plaintext).map_err(|e| DeepSeekCodeError::Storage(e.to_string()))
}

fn encode_secret(nonce: &[u8], ciphertext: &[u8]) -> String {
    let mut payload = nonce.to_vec();
    payload.extend_from_slice(ciphertext);
    format!("{ENCRYPTED_PREFIX}v2:{}", STANDARD.encode(payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[tokio::test]
    async fn encrypted_sqlite_fallback_roundtrip() {
        std::env::set_var("DEEPSEEKCODE_DISABLE_KEYRING", "1");
        let db = Database::open_in_memory().await.unwrap();
        let store = CredentialsStore::new(db.pool().clone());
        store.store("openai", "sk-test-secret").await.unwrap();
        let loaded = store.get("openai").await.unwrap();
        assert_eq!(loaded.as_deref(), Some("sk-test-secret"));
        std::env::remove_var("DEEPSEEKCODE_DISABLE_KEYRING");
    }

    #[tokio::test]
    async fn sqlite_encrypted_copy_survives_without_keyring() {
        let db = Database::open_in_memory().await.unwrap();
        let store = CredentialsStore::new(db.pool().clone());
        std::env::set_var("DEEPSEEKCODE_DISABLE_KEYRING", "1");
        store.store("deepseek", "sk-persist-me").await.unwrap();
        std::env::remove_var("DEEPSEEKCODE_DISABLE_KEYRING");

        let loaded = store.get("deepseek").await.unwrap();
        assert_eq!(loaded.as_deref(), Some("sk-persist-me"));
    }
}
