use deepseekcode_schema::agent::AgentDefinition;

pub fn default_agents() -> Vec<AgentDefinition> {
    vec![AgentDefinition::build(), AgentDefinition::plan()]
}

pub fn plan_system_prompt() -> &'static str {
    AgentDefinition::plan_system_prompt()
}
