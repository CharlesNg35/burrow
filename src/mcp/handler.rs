use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, PaginatedRequestParams,
    ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{Peer, RequestContext};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::json;

use super::mode::Mode;
use super::tools::{Notices, Tools};
use super::upstream::{HANDSHAKE_WAIT, State};

pub const NAME: &str = "burrow";

impl Tools {
    fn offered(&self, mode: Mode) -> Vec<Tool> {
        self.tools
            .list_all()
            .into_iter()
            .filter(|t| mode.allows(&t.name))
            .collect()
    }

    fn follow_tools(&self, peer: Peer<RoleServer>) {
        let mut changes = self.up.subscribe();
        let (up, listed) = (self.up.clone(), self.listed.clone());
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                if !matches!(&*changes.borrow_and_update(), State::Up(_)) {
                    continue;
                }
                let mode = up.mode();
                let shown = listed.lock().ok().and_then(|l| *l);
                if shown.is_some_and(|m| m != mode)
                    && peer.notify_tool_list_changed().await.is_err()
                {
                    break;
                }
            }
        });
    }
}

impl ServerHandler for Tools {
    fn get_info(&self) -> ServerConfig {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();
        ServerConfig::new(capabilities)
            .with_server_info(Implementation::new(NAME, env!("CARGO_PKG_VERSION")))
            .with_instructions(self.up.mode().instructions(self.up.is_up()))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        self.up.settled(HANDSHAKE_WAIT).await;
        if self.notices == Notices::ToolChanges {
            self.follow_tools(context.peer.clone());
        }
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.up.settled(HANDSHAKE_WAIT).await;
        let mode = self.up.mode();
        if let Ok(mut l) = self.listed.lock() {
            *l = Some(mode);
        }
        let listed = ListToolsResult::with_all_items(self.offered(mode));
        let hints = context
            .protocol_version()
            .is_some_and(|v| v >= ProtocolVersion::V_2026_07_28);
        Ok(if hints {
            listed.with_ttl_ms(0).with_cache_scope(CacheScope::Public)
        } else {
            listed
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.get(name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let mode = self.up.mode();
        if self.tools.has_route(&request.name) && !mode.allows(&request.name) {
            let v = json!({
                "error": "not-offered",
                "message": format!("{} is not offered in this session, which is {}", request.name, mode.word()),
                "hint": mode.instructions(true),
            });
            return Ok(CallToolResult::structured_error(v).into());
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tools.call(tcc).await
    }
}
