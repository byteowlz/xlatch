//! MCP adapter over `CrossLatch`'s core capability and job protocol.

use anyhow::Result;
use clap::Parser;
use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    schemars::{self, JsonSchema},
    serde::{Deserialize, Serialize},
    tool, tool_handler, tool_router,
    transport::io::stdio,
};
use std::path::PathBuf;
use xlatch_core::{
    capability::Request,
    local::{self, Control},
    paths::default_data_dir,
};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Expose approved CrossLatch capabilities to a local MCP client"
)]
struct Cli {
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let data_dir = cli.data_dir.map_or_else(default_data_dir, Ok)?;
    let server = McpServer {
        dir: data_dir,
        tool_router: McpServer::tool_router(),
    };
    server.serve(stdio()).await?.waiting().await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct InvokeParams {
    /// Stable capability id returned by discover.
    capability_id: String,
    /// Exact reviewed revision returned by discover.
    revision: String,
    /// JSON matching the capability's input schema.
    input: serde_json::Value,
    /// Unique invocation key; preserve it when retrying delivery.
    idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct JobParams {
    /// Job identifier returned by invoke.
    id: String,
}

#[derive(Clone)]
struct McpServer {
    dir: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    async fn call(&self, request: Request) -> Result<CallToolResult, McpError> {
        let result = local::call(&self.dir, Control::Rpc { request })
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(
            result.to_string(),
        )]))
    }
}

#[tool_router]
impl McpServer {
    /// Discover contracts without bypassing their activation status.
    #[tool(
        description = "List CrossLatch capabilities, including typed input/output schemas and approval status. Only active revisions can be invoked. This local adapter has the daemon OS user's authority."
    )]
    async fn discover(&self) -> Result<CallToolResult, McpError> {
        self.call(Request::Discover).await
    }
    /// Submit an approved action to the durable queue.
    #[tool(
        description = "Invoke an approved CrossLatch capability. Returns a durable job; use job to retrieve its result. Use the input schema and revision from discover."
    )]
    async fn invoke(
        &self,
        Parameters(p): Parameters<InvokeParams>,
    ) -> Result<CallToolResult, McpError> {
        self.call(Request::Invoke {
            capability_id: p.capability_id,
            revision: p.revision,
            input: p.input,
            idempotency_key: p.idempotency_key,
        })
        .await
    }
    /// Retrieve a job and its result.
    #[tool(description = "Retrieve a CrossLatch job's status, error, and result.")]
    async fn job(&self, Parameters(p): Parameters<JobParams>) -> Result<CallToolResult, McpError> {
        self.call(Request::Job { id: p.id }).await
    }
    /// Cancel outstanding execution.
    #[tool(description = "Cancel a queued or running CrossLatch job.")]
    async fn cancel(
        &self,
        Parameters(p): Parameters<JobParams>,
    ) -> Result<CallToolResult, McpError> {
        self.call(Request::Cancel { id: p.id }).await
    }
}

#[tool_handler(router=self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions=Some("CrossLatch is the capability/job core. This stdio MCP process is a local operator adapter; keep it inside the operator's trust boundary.".into());
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}
