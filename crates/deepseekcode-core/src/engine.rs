use camino::{Utf8Path, Utf8PathBuf};
use deepseekcode_config::{load_effective_config, DeepSeekCodeConfig};
use deepseekcode_storage::Database;
use deepseekcode_types::Result;
use std::sync::Arc;

use crate::location::LocationMap;
use crate::skills::{SkillCatalog, SkillEntry};

pub struct DeepSeekCodeEngine {
    pub db: Arc<Database>,
    pub config: DeepSeekCodeConfig,
    pub locations: LocationMap,
    pub project_dir: String,
}

impl DeepSeekCodeEngine {
    pub async fn open(project_dir: impl Into<String>, db_path: &str) -> Result<Self> {
        let project_dir = project_dir.into();
        let config = Utf8Path::new(&project_dir)
            .canonicalize_utf8()
            .ok()
            .or_else(|| Some(Utf8PathBuf::from(project_dir.clone())))
            .map(|p| load_effective_config(&p))
            .transpose()?
            .unwrap_or_default();
        let db = Arc::new(Database::open(db_path).await?);
        let locations = LocationMap::new(db.clone(), config.clone());
        Ok(Self {
            db,
            config,
            locations,
            project_dir,
        })
    }

    pub async fn location(&self, project_dir: &str) -> Arc<crate::location::LocationServices> {
        self.locations.get_or_create(project_dir).await
    }

    /// Reload config from disk and re-materialize tools/skills/MCP for a location.
    pub async fn reload_catalog(&mut self, project_dir: &str) -> Result<()> {
        if let Ok(path) = Utf8Path::new(project_dir).canonicalize_utf8() {
            let cfg = load_effective_config(&path)?;
            self.config = cfg.clone();
            self.locations.update_config(cfg).await;
        }
        self.reload_materialization(project_dir).await;
        Ok(())
    }

    pub async fn reload_materialization(&self, project_dir: &str) {
        self.locations.reload_materialization(project_dir).await;
    }

    pub fn skills(&self) -> Vec<SkillEntry> {
        SkillCatalog::discover(
            &self.config.skills,
            &self.project_dir,
            self.config.skills_index_url.as_deref(),
        )
    }
}
