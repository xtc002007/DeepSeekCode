use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, ToSchema, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Build,
    Plan,
    General,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AgentDefinition {
    pub kind: AgentKind,
    pub name: String,
    pub description: String,
    pub system_prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub tools: Vec<String>,
}

impl AgentDefinition {
    pub fn build() -> Self {
        Self {
            kind: AgentKind::Build,
            name: "build".into(),
            description: "Full-access agent for development work".into(),
            system_prompt: None,
            max_turns: None,
            tools: vec![
                "read".into(),
                "write".into(),
                "edit".into(),
                "apply_patch".into(),
                "grep".into(),
                "glob".into(),
                "list".into(),
                "bash".into(),
                "todowrite".into(),
                "todoread".into(),
                "webfetch".into(),
                "websearch".into(),
                "skill".into(),
                "question".into(),
                "task".into(),
                "lsp".into(),
            ],
        }
    }

    pub fn plan() -> Self {
        Self {
            kind: AgentKind::Plan,
            name: "plan".into(),
            description:
                "Planning agent that researches the codebase and writes implementation plans".into(),
            system_prompt: Some(Self::plan_system_prompt().into()),
            max_turns: Some(50),
            tools: vec![
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
        }
    }

    pub fn plan_system_prompt() -> &'static str {
        "You are in plan mode. Your job is to produce a systematic, thorough, and complete implementation plan based on the user's request.\n\
\n\
Workflow:\n\
1. Research the codebase first using read, grep, glob, and list.\n\
2. When you encounter uncertainty, contradictions, ambiguous requirements, or multiple valid approaches, you MUST call the question tool with 2-5 concrete options. Do not guess or assume. List only specific options in the options array — do not add a generic \"other\" option; the UI provides custom input automatically.\n\
3. Write the plan document to `.code-plus-plus/plan/<plan_id>/PLAN.md` (or another markdown file under that directory). The plan format is flexible: use whatever structure best fits the task (sections, lists, tables, diagrams, appendices). Aim for completeness and actionability.\n\
4. Do not modify project source files outside `.code-plus-plus/plan/`.\n\
5. When the plan is ready, call plan_complete with title and optional summary. Do not claim the plan is finished until plan_complete succeeds."
    }
}
