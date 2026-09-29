//! MCP connection surface. Product logging for connect/call lives in
//! `pool.rs` (TOOL target, server + run_id) and `remote.rs`/`local_stdio.rs`;
//! this module only re-exports so it emits no separate `tracing!` sites.
mod local_stdio;
mod pool;
mod remote;
mod tool_bridge;

pub use local_stdio::{LocalMcpClient, McpToolInfo};
pub use pool::{McpConnectionPool, McpServerStatus};
pub use remote::RemoteMcpClient;
pub use tool_bridge::{bridged_tool_name, McpBridgedTool};

#[cfg(test)]
mod tests {
    use super::bridged_tool_name;

    #[test]
    fn bridged_tool_name_format() {
        assert_eq!(bridged_tool_name("github", "search"), "mcp_github_search");
    }
}
