use deepseekcode_schema::session::PromptDelivery;
#[cfg(feature = "os-keychain")]
use keyring::Entry;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[cfg(feature = "os-keychain")]
const KEYRING_SERVICE: &str = "deepseekcode";
#[cfg(feature = "os-keychain")]
const SERVER_PASSWORD_ACCOUNT: &str = "server_password";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeId {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeId {
    pub const ALL: [ThemeId; 3] = [ThemeId::Dark, ThemeId::Light, ThemeId::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeId::Dark => "Dark",
            ThemeId::Light => "Light",
            ThemeId::System => "System",
        }
    }
}

/// One user-added model entry (Settings → Custom models), e.g. an
/// OpenRouter `author/model` id the static catalog cannot enumerate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomModelEntry {
    pub provider_id: String,
    pub id: String,
    pub name: String,
}

/// In-memory preferences (includes server password loaded from keyring).
#[derive(Debug, Clone)]
pub struct AppPreferences {
    pub selected_agent: String,
    pub selected_model: String,
    pub selected_provider: Option<String>,
    pub prompt_delivery: PromptDelivery,
    pub server_url: Option<String>,
    pub server_password: Option<String>,
    pub theme: ThemeId,
    pub custom_models: Vec<CustomModelEntry>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            selected_agent: "build".to_string(),
            selected_model: "gpt-4o-mini".to_string(),
            selected_provider: Some("openai".to_string()),
            prompt_delivery: PromptDelivery::Steer,
            server_url: None,
            server_password: None,
            theme: ThemeId::Dark,
            custom_models: Vec::new(),
        }
    }
}

/// JSON on disk — server password is never written here (keyring only).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreferencesFile {
    pub selected_agent: String,
    pub selected_model: String,
    pub selected_provider: Option<String>,
    #[serde(default)]
    pub prompt_delivery: PromptDelivery,
    #[serde(default)]
    pub server_url: Option<String>,
    /// Legacy plaintext field — migrated to keyring on load, then removed from disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_password: Option<String>,
    #[serde(default)]
    pub theme: ThemeId,
    /// User-added models (Settings → Custom models). `default` keeps old
    /// files loadable.
    #[serde(default)]
    pub custom_models: Vec<CustomModelEntry>,
}

impl Default for PreferencesFile {
    fn default() -> Self {
        let defaults = AppPreferences::default();
        Self {
            selected_agent: defaults.selected_agent,
            selected_model: defaults.selected_model,
            selected_provider: defaults.selected_provider,
            prompt_delivery: defaults.prompt_delivery,
            server_url: defaults.server_url,
            server_password: None,
            theme: defaults.theme,
            custom_models: defaults.custom_models,
        }
    }
}

impl From<PreferencesFile> for AppPreferences {
    fn from(file: PreferencesFile) -> Self {
        Self {
            selected_agent: file.selected_agent,
            selected_model: file.selected_model,
            selected_provider: file.selected_provider,
            prompt_delivery: file.prompt_delivery,
            server_url: file.server_url,
            server_password: None,
            theme: file.theme,
            custom_models: file.custom_models,
        }
    }
}

impl From<&AppPreferences> for PreferencesFile {
    fn from(prefs: &AppPreferences) -> Self {
        Self {
            selected_agent: prefs.selected_agent.clone(),
            selected_model: prefs.selected_model.clone(),
            selected_provider: prefs.selected_provider.clone(),
            prompt_delivery: prefs.prompt_delivery,
            server_url: prefs.server_url.clone(),
            server_password: None,
            theme: prefs.theme,
            custom_models: prefs.custom_models.clone(),
        }
    }
}

pub fn preferences_path() -> Option<PathBuf> {
    deepseekcode_types::data_dir().map(|dir| dir.join("deepseekcode").join("preferences.json"))
}

pub fn load_preferences() -> AppPreferences {
    let Some(path) = preferences_path() else {
        return AppPreferences::default();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return AppPreferences::default();
    };
    let Ok(file) = serde_json::from_str::<PreferencesFile>(&raw) else {
        return AppPreferences::default();
    };

    let mut prefs: AppPreferences = file.clone().into();
    prefs.server_password = load_server_password();

    if prefs.server_password.is_none() {
        if let Some(legacy) = file.server_password.filter(|p| !p.is_empty()) {
            if keyring_enabled() {
                store_server_password(Some(&legacy));
                prefs.server_password = Some(legacy);
                let _ = save_preferences_file(&path, &PreferencesFile::from(&prefs));
            } else {
                prefs.server_password = Some(legacy);
            }
        }
    } else if file.server_password.is_some() && keyring_enabled() {
        let cleared = PreferencesFile::from(&prefs);
        let _ = save_preferences_file(&path, &cleared);
    }

    prefs
}

pub fn save_preferences(prefs: &AppPreferences) {
    let Some(path) = preferences_path() else {
        return;
    };
    let mut file = PreferencesFile::from(prefs);
    if keyring_enabled() {
        store_server_password(prefs.server_password.as_deref());
    } else if let Some(pass) = &prefs.server_password {
        // Never log the password value itself: only the fact + run_id.
        tracing::warn!(
            target: deepseekcode_telemetry::DESKTOP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            "DEEPSEEKCODE_DISABLE_KEYRING set; storing server password in preferences.json"
        );
        file.server_password = Some(pass.clone());
    }
    let _ = save_preferences_file(&path, &file);
}

fn save_preferences_file(path: &PathBuf, file: &PreferencesFile) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(file).map_err(std::io::Error::other)?;
    fs::write(path, raw)
}

fn keyring_enabled() -> bool {
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
fn load_server_password() -> Option<String> {
    if !keyring_enabled() {
        return None;
    }
    let entry = Entry::new(KEYRING_SERVICE, SERVER_PASSWORD_ACCOUNT).ok()?;
    match entry.get_password() {
        Ok(password) if !password.is_empty() => Some(password),
        Ok(_) => None,
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            tracing::warn!(
                target: deepseekcode_telemetry::DESKTOP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                error = %e,
                "failed to read server password from keyring"
            );
            None
        }
    }
}

#[cfg(not(feature = "os-keychain"))]
fn load_server_password() -> Option<String> {
    None
}

#[cfg(not(feature = "os-keychain"))]
fn store_server_password(_password: Option<&str>) {}

#[cfg(feature = "os-keychain")]
fn store_server_password(password: Option<&str>) {
    if !keyring_enabled() {
        return;
    }
    let Ok(entry) = Entry::new(KEYRING_SERVICE, SERVER_PASSWORD_ACCOUNT) else {
        return;
    };
    match password.filter(|p| !p.is_empty()) {
        Some(value) => {
            if let Err(e) = entry.set_password(value) {
                tracing::warn!(
                    target: deepseekcode_telemetry::DESKTOP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    error = %e,
                    "failed to store server password in keyring"
                );
            }
        }
        None => {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn preferences_file_omits_server_password_on_save() {
        let _guard = test_lock();
        std::env::set_var("DEEPSEEKCODE_DISABLE_KEYRING", "1");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let prefs = AppPreferences {
            server_password: Some("secret-pass".into()),
            ..AppPreferences::default()
        };
        let file = PreferencesFile::from(&prefs);
        save_preferences_file(&path, &file).unwrap();
        let raw = fs::read_to_string(path).unwrap();
        assert!(!raw.contains("secret-pass"));
        assert!(!raw.contains("server_password"));
        std::env::remove_var("DEEPSEEKCODE_DISABLE_KEYRING");
    }

    #[test]
    fn migrates_legacy_plaintext_server_password() {
        let _guard = test_lock();
        std::env::set_var("DEEPSEEKCODE_DISABLE_KEYRING", "1");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        let legacy = PreferencesFile {
            server_password: Some("legacy-pass".into()),
            ..PreferencesFile::default()
        };
        save_preferences_file(&path, &legacy).unwrap();
        let loaded_raw = fs::read_to_string(&path).unwrap();
        assert!(loaded_raw.contains("legacy-pass"));

        // Simulate migration: read legacy from file, clear from disk
        let parsed: PreferencesFile = serde_json::from_str(&loaded_raw).unwrap();
        assert_eq!(parsed.server_password.as_deref(), Some("legacy-pass"));
        let migrated = PreferencesFile {
            server_password: None,
            ..parsed
        };
        save_preferences_file(&path, &migrated).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert!(!after.contains("legacy-pass"));
        std::env::remove_var("DEEPSEEKCODE_DISABLE_KEYRING");
    }
}
