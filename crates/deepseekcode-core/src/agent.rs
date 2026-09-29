use deepseekcode_schema::agent::AgentDefinition;

pub fn default_agents() -> Vec<AgentDefinition> {
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        "default_agents listed"
    );
    vec![AgentDefinition::build(), AgentDefinition::plan()]
}

pub fn plan_system_prompt() -> &'static str {
    tracing::debug!(
        target: deepseekcode_telemetry::APP,
        run_id = %deepseekcode_telemetry::current_run_id(),
        "plan_system_prompt loaded"
    );
    AgentDefinition::plan_system_prompt()
}
