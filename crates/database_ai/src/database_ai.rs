pub mod autonomous_agent;
pub mod mention_provider;
pub mod plan_analyzer;
pub mod query_generator;
pub mod query_optimizer;
pub mod schema_context;
mod mcp_server_manager;
mod mcp_tools;
mod tools;
#[cfg(test)]
mod tests;

pub use tools::{
    DescribeObjectTool, ExecuteQueryTool, ExplainQueryTool, GetSchemaTool, ListObjectsTool,
    ModifyDataTool,
};

use agent::Thread;
use gpui::App;

pub fn init(cx: &mut App) {
    cx.observe_new(|thread: &mut Thread, _window, _cx| {
        thread.add_tool(ExecuteQueryTool);
        thread.add_tool(DescribeObjectTool);
        thread.add_tool(ListObjectsTool);
        thread.add_tool(ExplainQueryTool);
        thread.add_tool(ModifyDataTool);
        thread.add_tool(GetSchemaTool);
    })
    .detach();

    // The MCP server manager must live for the entire app lifetime but has no natural
    // owner to store it in. We intentionally leak it via mem::forget because there is
    // no global entity storage that fits without a larger refactor.
    let mcp_manager = mcp_server_manager::DatabaseMcpServerManager::start(cx);
    std::mem::forget(mcp_manager);
}
