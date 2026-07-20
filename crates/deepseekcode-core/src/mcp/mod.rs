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
        assert_eq!(bridged_tool_name("github", "search"), "github_search");
    }
}
