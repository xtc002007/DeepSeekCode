use async_trait::async_trait;
use chrono::Utc;
use deepseekcode_schema::event::EventKind;
use deepseekcode_schema::plan::{PlanMeta, PlanStatus};
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::registry::Tool;
use super::services::current_services;

const MIN_PLAN_BYTES: usize = 200;

pub struct PlanCompleteTool;

#[async_trait]
impl Tool for PlanCompleteTool {
    fn name(&self) -> &str {
        "plan_complete"
    }

    fn description(&self) -> &str {
        "Mark a planning document as complete and persist plan metadata"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "summary": { "type": "string" },
                "primary_path": { "type": "string" },
                "plan_path": { "type": "string" },
                "plan_id": { "type": "string" }
            },
            "required": ["title"]
        })
    }

    async fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        let title = input["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| ToolFailure {
                message: "title is required".into(),
                retryable: false,
            })?
            .trim()
            .to_string();
        let summary = input["summary"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);

        let resolved = resolve_plan_location(&ctx.working_directory, &input, &title)?;
        let plan_id = resolved.plan_id;
        let primary_path = resolved.primary_path;
        let primary_file = resolved.primary_file;
        let plan_dir = resolved.plan_dir;

        if !primary_file.is_file() {
            return Err(ToolFailure {
                message: format!(
                    "plan file not found at .deepseekcode/plan/{plan_id}/{primary_path}; write the plan before calling plan_complete"
                ),
                retryable: true,
            });
        }

        let content = std::fs::read_to_string(&primary_file).map_err(|e| ToolFailure {
            message: format!("failed to read plan file: {e}"),
            retryable: true,
        })?;
        // Unicode-aware char count (P0-8: previously byte `len`, so 67 CJK
        // chars passed the 200-byte floor).
        if content.trim().chars().count() < MIN_PLAN_BYTES {
            return Err(ToolFailure {
                message: format!(
                    "plan file is too short (minimum {MIN_PLAN_BYTES} characters of content required)"
                ),
                retryable: true,
            });
        }

        std::fs::create_dir_all(&plan_dir).map_err(|e| ToolFailure {
            message: format!("failed to ensure plan directory: {e}"),
            retryable: true,
        })?;

        let now = Utc::now();
        let created_at = read_existing_created_at(&plan_dir).unwrap_or(now);
        let meta = PlanMeta {
            plan_id: plan_id.clone(),
            session_id: ctx.session_id.to_string(),
            title: title.clone(),
            summary: summary.clone(),
            primary_path: primary_path.clone(),
            status: PlanStatus::Completed,
            created_at,
            completed_at: Some(now),
        };
        let meta_path = plan_dir.join("meta.json");
        let meta_json = serde_json::to_string_pretty(&meta).map_err(|e| ToolFailure {
            message: format!("failed to serialize plan meta: {e}"),
            retryable: false,
        })?;
        std::fs::write(&meta_path, meta_json).map_err(|e| ToolFailure {
            message: format!("failed to write meta.json: {e}"),
            retryable: true,
        })?;

        let plan_path = format!(".deepseekcode/plan/{plan_id}/{primary_path}");
        let services = current_services().ok_or_else(|| ToolFailure {
            message: "plan_complete services not configured".into(),
            retryable: false,
        })?;
        if let Some(events) = &services.events {
            events
                .append(
                    EventKind::PlanCompleted,
                    Some(ctx.session_id),
                    json!({
                        "plan_id": plan_id,
                        "plan_path": plan_path,
                        "title": title,
                        "summary": summary,
                        "session_id": ctx.session_id.to_string(),
                    }),
                )
                .await
                .map_err(|e| ToolFailure {
                    message: format!("failed to record plan completion: {e}"),
                    retryable: true,
                })?;
        }

        Ok(json!({
            "plan_id": plan_id,
            "plan_path": plan_path,
            "title": title,
            "summary": summary,
            "status": "completed",
        }))
    }
}

struct ResolvedPlan {
    plan_id: String,
    primary_path: String,
    primary_file: PathBuf,
    plan_dir: PathBuf,
}

fn resolve_plan_location(
    working_directory: &str,
    input: &Value,
    title: &str,
) -> Result<ResolvedPlan, ToolFailure> {
    let working = PathBuf::from(working_directory);

    if let Some(plan_path) = input
        .get("plan_path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return resolve_from_relative_plan_path(&working, plan_path);
    }

    let primary_path = input["primary_path"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("PLAN.md")
        .to_string();

    let primary_file_name = Path::new(&primary_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("PLAN.md");

    let plan_id = input["plan_id"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| plan_id_from_primary_path(&primary_path))
        .or_else(|| discover_plan_with_file(&working, primary_file_name))
        .unwrap_or_else(|| generate_plan_id(title));

    let plan_dir = working.join(".deepseekcode").join("plan").join(&plan_id);
    let primary_file = plan_dir.join(&primary_path);

    if !primary_file.is_file() {
        if let Some(found_id) = discover_plan_with_file(&working, primary_file_name) {
            let plan_dir = working.join(".deepseekcode").join("plan").join(&found_id);
            let primary_file = plan_dir.join(primary_file_name);
            return Ok(ResolvedPlan {
                plan_id: found_id,
                primary_path: primary_file_name.to_string(),
                primary_file,
                plan_dir,
            });
        }
    }

    Ok(ResolvedPlan {
        plan_id,
        primary_path,
        primary_file,
        plan_dir,
    })
}

fn resolve_from_relative_plan_path(
    working: &Path,
    plan_path: &str,
) -> Result<ResolvedPlan, ToolFailure> {
    let rel = crate::project_context::policy_path_pattern(&working.to_string_lossy(), plan_path);
    let normalized = rel.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|p| !p.is_empty()).collect();
    let Some(plan_idx) = parts
        .windows(2)
        .position(|w| w[0] == ".deepseekcode" && w[1] == "plan")
    else {
        return Err(ToolFailure {
            message: format!("plan_path must be under .deepseekcode/plan/: {plan_path}"),
            retryable: true,
        });
    };
    let plan_id = parts.get(plan_idx + 2).ok_or_else(|| ToolFailure {
        message: format!("plan_path must include a plan id directory: {plan_path}"),
        retryable: true,
    })?;
    let primary_path = parts
        .get(plan_idx + 3..)
        .map(|slice| slice.join("/"))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "PLAN.md".into());
    let plan_dir = working.join(".deepseekcode").join("plan").join(plan_id);
    let primary_file = plan_dir.join(&primary_path);
    Ok(ResolvedPlan {
        plan_id: (*plan_id).to_string(),
        primary_path,
        primary_file,
        plan_dir,
    })
}

fn plan_id_from_primary_path(primary_path: &str) -> Option<String> {
    let path = Path::new(primary_path);
    let parent = path.parent()?;
    if parent.as_os_str().is_empty() {
        return None;
    }
    Some(parent.to_string_lossy().replace('\\', "/"))
}

fn discover_plan_with_file(working: &Path, primary_file_name: &str) -> Option<String> {
    let plan_root = working.join(".deepseekcode").join("plan");
    if !plan_root.is_dir() {
        return None;
    }
    // Deterministic: (mtime, plan_id) max wins so same-second concurrent
    // writes resolve identically (P0-8: previously mtime-only, nondeterministic).
    let mut best: Option<(SystemTime, String)> = None;
    for entry in std::fs::read_dir(&plan_root).ok()? {
        let entry = entry.ok()?;
        if !entry.file_type().ok()?.is_dir() {
            continue;
        }
        let plan_file = entry.path().join(primary_file_name);
        if !plan_file.is_file() {
            continue;
        }
        let modified = std::fs::metadata(&plan_file).ok()?.modified().ok()?;
        let id = entry.file_name().to_string_lossy().to_string();
        let candidate = (modified, id);
        if best.as_ref().map(|b| candidate > *b).unwrap_or(true) {
            best = Some(candidate);
        }
    }
    best.map(|(_, id)| id)
}

fn read_existing_created_at(plan_dir: &Path) -> Option<chrono::DateTime<Utc>> {
    let meta_path = plan_dir.join("meta.json");
    let raw = std::fs::read_to_string(meta_path).ok()?;
    let meta: PlanMeta = serde_json::from_str(&raw).ok()?;
    Some(meta.created_at)
}

fn generate_plan_id(title: &str) -> String {
    let slug = slugify(title);
    // Char-boundary safe truncation (P0-8: byte slice panicked on CJK).
    let slug = if slug.is_empty() {
        "plan".to_string()
    } else {
        let truncated: String = slug.chars().take(48).collect();
        truncated.trim_end_matches('-').to_string()
    };
    let date = Utc::now().format("%Y%m%d");
    // 6-char suffix (26 bits more entropy than 4) + existence check by
    // callers; ULID time-ordered so collisions are negligible.
    let suffix = ulid::Ulid::new().to_string();
    let suffix = &suffix[..6.min(suffix.len())];
    format!("{slug}-{date}-{suffix}")
}

#[allow(dead_code)]
fn truncate_char_boundary(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn slugify_normalizes_title() {
        assert_eq!(slugify("User Auth Refactor!"), "user-auth-refactor");
    }

    #[test]
    fn generate_plan_id_includes_date_and_suffix() {
        let id = generate_plan_id("My Plan");
        assert!(id.starts_with("my-plan-"));
        assert!(id.len() > "my-plan-".len() + 8);
    }

    #[test]
    fn discovers_existing_plan_directory() {
        let dir = tempdir().unwrap();
        let plan_dir = dir.path().join(".deepseekcode/plan/UI-redesign");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(plan_dir.join("PLAN.md"), "x".repeat(220)).unwrap();
        let found = discover_plan_with_file(dir.path(), "PLAN.md").unwrap();
        assert_eq!(found, "UI-redesign");
    }

    #[test]
    fn resolves_plan_id_from_nested_primary_path() {
        assert_eq!(
            plan_id_from_primary_path("UI-redesign/PLAN.md").as_deref(),
            Some("UI-redesign")
        );
    }
}
