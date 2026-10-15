use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub connected: bool,
    pub models: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub context_window: Option<u32>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ConnectProviderRequest {
    pub api_key: Option<String>,
    pub base_url: Option<String>,
}
