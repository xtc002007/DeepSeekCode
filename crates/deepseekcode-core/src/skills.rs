use async_trait::async_trait;
use deepseekcode_config::DeepSeekCodeConfig;
use deepseekcode_schema::tool::{ToolContext, ToolFailure};
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
        roots.push(PathBuf::from(project_dir).join(".deepseekcode").join("skills"));

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

        found
    }

    pub fn filter_for_agent(
        skills: &[SkillEntry],
        agent: &str,
        config: &DeepSeekCodeConfig,
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
        let mut lines =
            vec!["Available skills (use the skill tool to load full content):".to_string()];
        for s in skills {
            lines.push(format!("- {}: {}", s.name, s.description));
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
        let name = input["name"]
            .as_str()
            .ok_or_else(|| ToolFailure {
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
    let response = match reqwest::blocking::get(url) {
        Ok(resp) if resp.status().is_success() => resp,
        Ok(resp) => {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                url = %url,
                status = %resp.status(),
                "skills index fetch failed"
            );
            return Vec::new();
        }
        Err(e) => {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
                url = %url,
                error = %e,
                "skills index fetch failed"
            );
            return Vec::new();
        }
    };
    let value: Value = match response.json() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: deepseekcode_telemetry::APP,
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
    items
        .into_iter()
        .filter_map(|item| {
            let name = item.get("name").and_then(|v| v.as_str()).map(String::from)?;
            let description = item
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("No description")
                .to_string();
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

fn agent_allowed_tools(agent: &str, config: &DeepSeekCodeConfig) -> Vec<String> {
    if let Some(agent_cfg) = config.agents.get(agent) {
        if let Some(tools) = &agent_cfg.tools {
            return tools.clone();
        }
    }
    match agent {
        "plan" => vec![
            "read".into(),
            "grep".into(),
            "glob".into(),
            "list".into(),
            "write".into(),
            "edit".into(),
            "skill".into(),
            "question".into(),
            "plan_complete".into(),
        ],
        _ => vec![
            "read".into(),
            "write".into(),
            "edit".into(),
            "grep".into(),
            "glob".into(),
            "bash".into(),
            "apply_patch".into(),
            "todowrite".into(),
            "todo_write".into(),
            "skill".into(),
            "webfetch".into(),
            "lsp".into(),
        ],
    }
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
        let skill_dir = dir.path().join(".deepseekcode").join("skills").join("my-skill");
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
}
