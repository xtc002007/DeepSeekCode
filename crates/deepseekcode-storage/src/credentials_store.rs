use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use deepseekcode_types::{DeepSeekCodeError, Result};
use keyring::Entry;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

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
        let _ = try_keyring_delete(provider_id);
        sqlx::query(r#"DELETE FROM credentials WHERE provider_id = ?"#)
            .bind(provider_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(())
    }

    pub async fn list_connected(&self) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar::<_, String>(
            r#"SELECT provider_id FROM credentials ORDER BY provider_id"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
        Ok(rows)
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
    !std::env::var("DEEPSEEKCODE_DISABLE_KEYRING")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn try_keyring_store(provider_id: &str, api_key: &str) -> Result<()> {
    let entry = Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    entry
        .set_password(api_key)
        .map_err(|e| DeepSeekCodeError::Storage(format!("keyring store: {e}")))
}

fn try_keyring_get(provider_id: &str) -> Result<Option<String>> {
    let entry = Entry::new(KEYRING_SERVICE, provider_id)
        .map_err(|e| DeepSeekCodeError::Storage(e.to_string()))?;
    match entry.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(DeepSeekCodeError::Storage(format!("keyring get: {e}"))),
    }
}

fn try_keyring_delete(provider_id: &str) -> Result<()> {
    if let Ok(entry) = Entry::new(KEYRING_SERVICE, provider_id) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

fn derive_local_key() -> [u8; 32] {
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
    let nonce_bytes = rand_bytes(12);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, secret.as_bytes())
        .map_err(|e| DeepSeekCodeError::Storage(format!("encrypt: {e}")))?;
    let mut payload = nonce_bytes;
    payload.extend(ciphertext);
    Ok(format!("{ENCRYPTED_PREFIX}{}", STANDARD.encode(payload)))
}

fn decrypt_secret(cipher: &Aes256Gcm, payload: &str) -> Result<String> {
    let bytes = STANDARD
        .decode(payload)
        .map_err(|e| DeepSeekCodeError::Storage(format!("decode: {e}")))?;
    if bytes.len() <= 12 {
        return Err(DeepSeekCodeError::Storage("invalid encrypted payload".into()));
    }
    let (nonce_bytes, ciphertext) = bytes.split_at(12);
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|e| DeepSeekCodeError::Storage(format!("decrypt: {e}")))?;
    String::from_utf8(plaintext).map_err(|e| DeepSeekCodeError::Storage(e.to_string()))
}

fn rand_bytes(len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    if getrandom::getrandom(&mut out).is_ok() {
        return out;
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let mut state = hasher.finish();
    for byte in &mut out {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (state >> 33) as u8;
    }
    out
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
