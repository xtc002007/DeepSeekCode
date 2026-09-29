use camino::{Utf8Path, Utf8PathBuf};
use deepseekcode_config::{load_effective_config, DeepSeekCodeConfig};
use deepseekcode_storage::{normalize_directory_path, Database};
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
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            db_path = %db_path,
            "engine open started"
        );
        let project_dir = normalize_directory_path(&project_dir.into());
        let config = Utf8Path::new(&project_dir)
            .canonicalize_utf8()
            .ok()
            .or_else(|| Some(Utf8PathBuf::from(project_dir.clone())))
            .map(|p| load_effective_config(&p))
            .transpose()
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::APP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    project_dir = %project_dir,
                    error = %e,
                    "engine open failed: config load"
                );
                e
            })?
            .unwrap_or_default();
        let db = Arc::new(Database::open(db_path).await.map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::APP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                project_dir = %project_dir,
                error = %e,
                "engine open failed: database open"
            );
            e
        })?);
        let locations = LocationMap::new(db.clone(), config.clone());
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "engine open completed"
        );
        Ok(Self {
            db,
            config,
            locations,
            project_dir,
        })
    }

    pub async fn location(&self, project_dir: &str) -> Arc<crate::location::LocationServices> {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "engine location requested"
        );
        self.locations.get_or_create(project_dir).await
    }

    /// Reload config from disk and re-materialize tools/skills/MCP for a location.
    pub async fn reload_catalog(&mut self, project_dir: &str) -> Result<()> {
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "engine reload_catalog started"
        );
        // Never silently keep dirty config: canonicalize failure is an error,
        // not a skip (previously dirty config + materialization ran anyway).
        let path = Utf8Path::new(project_dir)
            .canonicalize_utf8()
            .map_err(|e| {
                tracing::error!(
                    target: deepseekcode_telemetry::APP,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    project_dir = %project_dir,
                    error = %e,
                    "engine reload_catalog failed: canonicalize"
                );
                deepseekcode_types::DeepSeekCodeError::Config(format!(
                    "canonicalize {project_dir}: {e}"
                ))
            })?;
        let cfg = load_effective_config(&path).map_err(|e| {
            tracing::error!(
                target: deepseekcode_telemetry::APP,
                run_id = %deepseekcode_telemetry::current_run_id(),
                project_dir = %project_dir,
                error = %e,
                "engine reload_catalog failed: config load"
            );
            e
        })?;
        self.config = cfg.clone();
        self.locations.update_config(cfg).await;
        self.reload_materialization(project_dir).await;
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "engine reload_catalog completed"
        );
        Ok(())
    }

    pub async fn reload_materialization(&self, project_dir: &str) {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "engine reload_materialization started"
        );
        self.locations.reload_materialization(project_dir).await;
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %project_dir,
            "engine reload_materialization completed"
        );
    }

    pub async fn interrupt_all(&self) {
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %self.project_dir,
            "engine interrupt_all started"
        );
        self.locations.interrupt_all().await;
        tracing::info!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %self.project_dir,
            "engine interrupt_all completed"
        );
    }

    pub fn skills(&self) -> Vec<SkillEntry> {
        tracing::debug!(
            target: deepseekcode_telemetry::APP,
            run_id = %deepseekcode_telemetry::current_run_id(),
            project_dir = %self.project_dir,
            "engine skills listed"
        );
        SkillCatalog::discover(
            &self.config.skills,
            &self.project_dir,
            self.config.skills_index_url.as_deref(),
        )
    }
}
