use async_trait::async_trait;
use code_plus_plus_config::CodePlusPlusConfig;
use code_plus_plus_schema::tool::{ToolContext, ToolFailure};
use glob::Pattern;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

pub struct SkillCatalog;

impl SkillCatalog {
    pub fn discover(
        paths: &[String],
        project_dir: &str,
        index_url: Option<&str>,
    ) -> Vec<SkillEntry> {
        tracing::debug!(
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            project_dir = %project_dir,
            paths = paths.len(),
            has_index = index_url.is_some(),
            "skill discover started"
        );
        let mut found = Vec::new();
        let mut roots: Vec<PathBuf> = paths
            .iter()
            .map(|source| {
                if Path::new(source).is_absolute() {
                    PathBuf::from(source)
                } else {
                    PathBuf::from(project_dir).join(source)
                }
            })
            .collect();
        roots.push(
            PathBuf::from(project_dir)
                .join(".code-plus-plus")
                .join("skills"),
        );

        for base in roots {
            if !base.is_dir() {
                continue;
            }
            if let Ok(entries) = std::fs::read_dir(&base) {
                for entry in entries.flatten() {
                    let skill_md = entry.path().join("SKILL.md");
                    if skill_md.exists() {
                        if let Some(entry) = parse_skill_md(&skill_md) {
                            found.push(entry);
                        }
                    }
                }
            }
        }

        if let Some(url) = index_url {
            found.extend(fetch_remote_index(url));
        }

        tracing::info!(
            target: code_plus_plus_telemetry::APP,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            project_dir = %project_dir,
            skills = found.len(),
            "skill discover completed"
        );
        found
    }

    pub fn filter_for_agent(
        skills: &[SkillEntry],
        agent: &str,
        config: &CodePlusPlusConfig,
    ) -> Vec<SkillEntry> {
        let allowed_tools = agent_allowed_tools(agent, config);
        if !allowed_tools.iter().any(|t| t == "skill") {
            return Vec::new();
        }
        skills
            .iter()
            .filter(|skill| {
                let pattern = format!("skill:{}", skill.name);
                !config
                    .permissions
                    .deny
                    .iter()
                    .any(|p| skill_pattern_matches(p, &pattern))
            })
            .cloned()
            .collect()
    }

    pub fn guidance_text(skills: &[SkillEntry]) -> String {
        if skills.is_empty() {
            return String::new();
        }
        // Cache-stable catalog: bound the prefix, spill the rest behind the
        // `skill` tool. Full SKILL.md bodies never enter `system`.
        const MAX_CATALOG_ENTRIES: usize = 24;
        const MAX_CATALOG_CHARS: usize = 2_000;
        let mut lines =
            vec!["Available skills (use the skill tool to load full content):".to_string()];
        let mut chars = lines[0].len();
        let mut shown = 0usize;
        let mut sorted: Vec<&SkillEntry> = skills.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        for s in sorted.iter().take(MAX_CATALOG_ENTRIES) {
            let line = format!("- {}: {}", s.name, s.description);
            if chars + line.len() + 1 > MAX_CATALOG_CHARS {
                break;
            }
            chars += line.len() + 1;
            lines.push(line);
            shown += 1;
        }
        if shown < skills.len() {
            lines.push(format!(
                "…({} more; discover with skill tool)",
                skills.len() - shown
            ));
        }
        lines.join("\n")
    }

    pub fn load_content(entry: &SkillEntry) -> Option<String> {
        std::fs::read_to_string(&entry.path).ok()
    }
}

/// Builtin `skill` tool — loads full SKILL.md content by name.
pub struct SkillTool {
    skills: Vec<SkillEntry>,
}

impl SkillTool {
    pub fn new(skills: Vec<SkillEntry>) -> Self {
        Self { skills }
    }
}

#[async_trait]
impl crate::tools::registry::Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Load the full content of an available skill by name"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Skill name from available_skills in system context"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> std::result::Result<Value, ToolFailure> {
        let name = input["name"].as_str().ok_or_else(|| ToolFailure {
            message: "missing skill name".into(),
            retryable: false,
        })?;
        let skill = self
            .skills
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| ToolFailure {
                message: format!("skill not found: {name}"),
                retryable: false,
            })?;
        let content = SkillCatalog::load_content(skill).ok_or_else(|| ToolFailure {
            message: format!("failed to read skill file: {}", skill.path.display()),
            retryable: false,
        })?;
        let base_dir = skill
            .path
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        Ok(json!({
            "name": skill.name,
            "description": skill.description,
            "base_dir": base_dir,
            "content": content,
        }))
    }
}

fn parse_skill_md(path: &Path) -> Option<SkillEntry> {
    let content = std::fs::read_to_string(path).ok()?;
    let name = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "skill".into());
    let description = extract_frontmatter_field(&content, "description")
        .unwrap_or_else(|| "No description".into());
    let name = extract_frontmatter_field(&content, "name").unwrap_or(name);
    Some(SkillEntry {
        name,
        description,
        path: path.to_path_buf(),
    })
}

fn extract_frontmatter_field(content: &str, field: &str) -> Option<String> {
    if !content.starts_with("---") {
        return None;
    }
    let rest = content.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    let front = &rest[..end];
    for line in front.lines() {
        let line = line.trim();
        if let Some((key, value)) = line.split_once(':') {
            if key.trim() == field {
                return Some(value.trim().trim_matches('"').to_string());
            }
        }
    }
    None
}

fn fetch_remote_index(url: &str) -> Vec<SkillEntry> {
    // Issue-9: 5s timeout + entry bound, mirroring `load_instruction_url`.
    // A hanging skills index must never block `drain` nor flap the catalog
    // prefix every turn. P1-9: async client (no `blocking` feature).
    let content = match crate::system_context::source::blocking_http_get(url) {
        Ok(text) => text,
        Err(e) => {
            tracing::warn!(
                target: code_plus_plus_telemetry::APP,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                url = %url,
                error = %e,
                "skills index fetch failed"
            );
            return Vec::new();
        }
    };
    let value: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: code_plus_plus_telemetry::APP,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                url = %url,
                error = %e,
                "skills index parse failed"
            );
            return Vec::new();
        }
    };
    let items = value
        .get("skills")
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array())
        .cloned()
        .unwrap_or_default();
    // Bound remote catalog so one huge index cannot blow the prefix; the
    // `guidance_text` budget (24 entries / 2000 chars) truncates the rest
    // behind the `skill` tool.
    const MAX_REMOTE_SKILLS: usize = 64;
    items
        .into_iter()
        .take(MAX_REMOTE_SKILLS)
        .filter_map(|item| {
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .map(String::from)?;
            let description: String = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("No description")
                .chars()
                .take(200)
                .collect();
            let path = item
                .get("path")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(name.replace(':', "/")));
            Some(SkillEntry {
                name,
                description,
                path,
            })
        })
        .collect()
}

/// Delegates to the canonical allow-list in `tools::registry` (single source
/// of truth). A forked copy lived here and drifted out of sync; delegating
/// keeps skill visibility consistent with what the runner actually sends.
fn agent_allowed_tools(agent: &str, config: &CodePlusPlusConfig) -> Vec<String> {
    crate::tools::registry::agent_allowed_tools(agent, config)
}

fn skill_pattern_matches(pattern: &str, skill_name: &str) -> bool {
    if pattern == "*" || pattern == skill_name {
        return true;
    }
    Pattern::new(pattern)
        .map(|p| p.matches(skill_name))
        .unwrap_or(false)
}

pub type SkillDiscovery = SkillCatalog;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn discovers_skill_with_frontmatter() {
        let dir = TempDir::new().unwrap();
        let skill_dir = dir
            .path()
            .join(".code-plus-plus")
            .join("skills")
            .join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: test-skill\ndescription: A test skill\n---\n# Body",
        )
        .unwrap();
        let skills = SkillCatalog::discover(&[], dir.path().to_str().unwrap(), None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
    }

    #[test]
    fn guidance_text_is_bounded_and_sorted() {
        let skills: Vec<SkillEntry> = (0..30)
            .map(|i| SkillEntry {
                name: format!("skill-{i:02}"),
                description: "d".into(),
                path: PathBuf::from(format!("/tmp/{i}")),
            })
            .collect();
        let text = SkillCatalog::guidance_text(&skills);
        // Bounded for prefix cache, remainder behind the skill tool.
        assert!(text.len() <= 2200);
        assert!(text.contains("more; discover with skill tool"));
        // Sorted so the prefix is byte-stable.
        let first = text.find("skill-00").unwrap();
        let second = text.find("skill-01").unwrap();
        assert!(first < second);
    }
}
