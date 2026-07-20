use deepseekcode_schema::provider::{ModelInfo, ProviderInfo};
use deepseekcode_storage::CredentialsStore;
use deepseekcode_types::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ProviderCatalog {
    credentials: RwLock<HashMap<String, String>>,
    store: Option<Arc<CredentialsStore>>,
    base_urls: RwLock<HashMap<String, String>>,
}

impl ProviderCatalog {
    pub fn new() -> Self {
        Self {
            credentials: RwLock::new(HashMap::new()),
            store: None,
            base_urls: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_store(store: Arc<CredentialsStore>) -> Self {
        Self {
            credentials: RwLock::new(HashMap::new()),
            store: Some(store),
            base_urls: RwLock::new(HashMap::new()),
        }
    }

    pub async fn load_from_store(&self) -> Result<()> {
        if let Some(store) = &self.store {
            for provider_id in store.list_connected().await? {
                if let Some(key) = store.get(&provider_id).await? {
                    self.credentials
                        .write()
                        .await
                        .insert(provider_id, key);
                }
            }
        }
        Ok(())
    }

    pub async fn load_from_config(&self, providers: &HashMap<String, deepseekcode_config::ProviderConfig>) {
        for (id, cfg) in providers {
            if let Some(key) = &cfg.api_key {
                self.credentials
                    .write()
                    .await
                    .insert(id.clone(), key.clone());
            }
            if let Some(url) = &cfg.base_url {
                self.base_urls.write().await.insert(id.clone(), url.clone());
            } else if id == "deepseek" {
                self.base_urls
                    .write()
                    .await
                    .insert(id.clone(), deepseek_default_base_url());
            }
        }
    }

    pub fn context_window_for(&self, provider_id: &str, model_id: &str) -> Option<u32> {
        let creds = HashMap::new();
        let provider = provider_entry(provider_id, provider_id, &creds);
        provider
            .models
            .iter()
            .find(|m| m.id == model_id)
            .and_then(|m| m.context_window)
    }

    pub async fn list(&self) -> Vec<ProviderInfo> {
        let creds = self.credentials.read().await;
        vec![
            provider_entry("openai", "OpenAI", &creds),
            provider_entry("anthropic", "Anthropic", &creds),
            provider_entry("deepseek", "DeepSeek", &creds),
            provider_entry("openrouter", "OpenRouter", &creds),
            provider_entry("openai-compatible", "OpenAI Compatible", &creds),
        ]
    }

    pub async fn connect(
        &self,
        provider_id: &str,
        api_key: String,
        base_url: Option<String>,
    ) -> Result<()> {
        self.credentials
            .write()
            .await
            .insert(provider_id.to_string(), api_key.clone());
        if let Some(url) = base_url.filter(|u| !u.is_empty()) {
            self.base_urls
                .write()
                .await
                .insert(provider_id.to_string(), url);
        }
        if provider_id == "deepseek" {
            let mut urls = self.base_urls.write().await;
            if !urls.contains_key("deepseek") {
                urls.insert("deepseek".into(), deepseek_default_base_url());
            }
        }
        if let Some(store) = &self.store {
            store.store(provider_id, &api_key).await?;
        }
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            provider_id = provider_id,
            "provider connected"
        );
        Ok(())
    }

    pub async fn disconnect(&self, provider_id: &str) -> Result<()> {
        self.credentials.write().await.remove(provider_id);
        if let Some(store) = &self.store {
            store.delete(provider_id).await?;
        }
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            provider_id = provider_id,
            "provider disconnected"
        );
        Ok(())
    }

    pub async fn get_api_key(&self, provider_id: &str) -> Option<String> {
        if let Some(key) = self.credentials.read().await.get(provider_id).cloned() {
            return Some(key);
        }
        None
    }

    pub async fn resolve_api_key(&self, provider_id: &str) -> Option<String> {
        if let Some(key) = self.get_api_key(provider_id).await {
            return Some(key);
        }
        match provider_id {
            "openai" | "openai-compatible" => std::env::var("OPENAI_API_KEY").ok(),
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
            "deepseek" => std::env::var("DEEPSEEK_API_KEY").ok(),
            _ => None,
        }
    }

    pub async fn get_base_url(&self, provider_id: &str) -> Option<String> {
        if let Some(url) = self.base_urls.read().await.get(provider_id).cloned() {
            return Some(url);
        }
        match provider_id {
            "deepseek" => Some(deepseek_default_base_url()),
            _ => None,
        }
    }
}

fn deepseek_default_base_url() -> String {
    deepseekcode_llm::DEEPSEEK_DEFAULT_BASE_URL.to_string()
}

fn provider_entry(id: &str, name: &str, creds: &HashMap<String, String>) -> ProviderInfo {
    let models = match id {
        "openai" | "openai-compatible" => vec![
            model("gpt-4o", "GPT-4o", id, 128_000),
            model("gpt-4o-mini", "GPT-4o Mini", id, 128_000),
        ],
        "anthropic" => vec![
            model("claude-sonnet-4-20250514", "Claude Sonnet 4", id, 128_000),
            model("claude-3-5-haiku-20241022", "Claude 3.5 Haiku", id, 128_000),
        ],
        "deepseek" => vec![
            model("deepseek-v4-flash", "DeepSeek V4 Flash", id, 1_000_000),
            model("deepseek-v4-pro", "DeepSeek V4 Pro", id, 1_000_000),
            model("deepseek-chat", "DeepSeek Chat (legacy)", id, 1_000_000),
        ],
        "openrouter" => vec![model("openai/gpt-4o-mini", "GPT-4o Mini (OR)", id, 128_000)],
        _ => vec![],
    };
    ProviderInfo {
        id: id.into(),
        name: name.into(),
        connected: creds.contains_key(id),
        models,
    }
}

fn model(id: &str, name: &str, provider: &str, context_window: u32) -> ModelInfo {
    ModelInfo {
        id: id.into(),
        name: name.into(),
        provider_id: provider.into(),
        context_window: Some(context_window),
        enabled: true,
    }
}

impl Default for ProviderCatalog {
    fn default() -> Self {
        Self::new()
    }
}
