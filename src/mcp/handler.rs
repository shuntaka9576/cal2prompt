use crate::core::cal2prompt::{Cal2Prompt, Cal2PromptError, JsonRpcErrorCode};
use crate::google::calendar::service::CalendarServiceError;
use crate::mcp::stdio::{Message, StdioTransport, Transport};
use futures::StreamExt;
use serde_json::{json, Value};

static TOOLS_JSON: &str = include_str!("./tools.json");

pub struct McpHandler<'a> {
    cal2prompt: &'a mut Cal2Prompt,
    initialized: bool,
}

impl<'a> McpHandler<'a> {
    pub fn new(cal2prompt: &'a mut Cal2Prompt) -> Self {
        Self {
            cal2prompt,
            initialized: false,
        }
    }

    pub async fn launch_mcp(&mut self, transport: &StdioTransport) -> anyhow::Result<()> {
        let mut stream = transport.receive();

        eprintln!("MCP stdio transport server started. Waiting for JSON messages on stdin...");

        while let Some(msg_result) = stream.next().await {
            match msg_result {
                Ok(Message::Request {
                    id, method, params, ..
                }) => {
                    self.handle_request_message(transport, id, method, params)
                        .await?;
                }
                Ok(Message::Notification { method, params, .. }) => {
                    eprintln!(
                        "[SERVER] Got Notification: method={}, params={:?}",
                        method, params
                    );
                }
                Ok(Message::Response {
                    id, result, error, ..
                }) => {
                    eprintln!(
                        "[SERVER] Got Response: id={}, result={:?}, error={:?}",
                        id, result, error
                    );
                }
                Err(e) => {
                    eprintln!("[SERVER] Error receiving message: {:?}", e);
                }
            }
        }

        Ok(())
    }

    async fn handle_request_message(
        &mut self,
        transport: &StdioTransport,
        id: u64,
        method: String,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        eprintln!(
            "[SERVER] Got Request: id={}, method={}, params={:?}",
            id, method, params
        );

        if method == "initialize" {
            return self.handle_initialize_request(transport, id).await;
        }

        if !self.initialized {
            return self
                .send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::InvalidRequest,
                    "Server not initialized. Send 'initialize' request first.".to_string(),
                )
                .await;
        }

        match method.as_str() {
            "tools/list" => self.handle_tools_list_request(transport, id).await,
            "tools/call" => self.handle_tools_call_request(transport, id, params).await,
            _ => {
                self.handle_generic_request(transport, id, method, params)
                    .await
            }
        }
    }

    async fn handle_initialize_request(
        &mut self,
        transport: &StdioTransport,
        id: u64,
    ) -> anyhow::Result<()> {
        if let Err(err) = self.handle_initialize(transport, id).await {
            eprintln!("[SERVER] Error handling initialize: {:?}", err);
            return self
                .send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::InternalError,
                    format!("Failed to initialize: {}", err),
                )
                .await;
        }
        self.initialized = true;
        Ok(())
    }

    async fn handle_tools_list_request(
        &self,
        transport: &StdioTransport,
        id: u64,
    ) -> anyhow::Result<()> {
        if let Err(err) = self.handle_tools_list(transport, id).await {
            eprintln!("[SERVER] Error handling tools/list: {:?}", err);
            return self
                .send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::InternalError,
                    format!("Failed to list tools: {}", err),
                )
                .await;
        }
        Ok(())
    }

    async fn handle_tools_call_request(
        &mut self,
        transport: &StdioTransport,
        id: u64,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        let account: Option<String> = params
            .as_ref()
            .and_then(|p| p.pointer("/arguments/profile"))
            .and_then(|v| v.as_str())
            .map(ToString::to_string);

        if let Err(err) = self.ensure_authentication(transport, id, account).await {
            return err;
        }

        if let Some(params_val) = params {
            self.handle_tools_call(transport, id, params_val).await?;
        }
        Ok(())
    }

    async fn ensure_authentication(
        &mut self,
        transport: &StdioTransport,
        id: u64,
        account: Option<String>,
    ) -> Result<(), anyhow::Result<()>> {
        let account_name = match &account {
            Some(p) => p.clone(),
            None => self.cal2prompt.accounts.keys().next().unwrap().clone(),
        };

        if self
            .cal2prompt
            .accounts
            .get(&account_name)
            .unwrap()
            .token
            .is_none()
        {
            if let Err(err) = self.cal2prompt.oauth(account.clone()).await {
                if let Some(Cal2PromptError::OAuth2PortInUse(_)) =
                    err.downcast_ref::<Cal2PromptError>()
                {
                    return Err(self.send_error_response(
                        transport,
                        id,
                        JsonRpcErrorCode::PortInUse,
                        "Port 9004 is already in use. Another instance of cal2prompt or Windsurf may be running.".to_string(),
                    )
                    .await);
                }

                return Err(self
                    .send_error_response(
                        transport,
                        id,
                        JsonRpcErrorCode::InternalError,
                        format!("Failed to authenticate: {}", err),
                    )
                    .await);
            }
        }

        if let Err(err) = self.cal2prompt.ensure_valid_token(account.clone()).await {
            if let Some(Cal2PromptError::OAuth2PortInUse(_)) = err.downcast_ref::<Cal2PromptError>()
            {
                return Err(self.send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::PortInUse,
                    "Port 9004 is already in use. Another instance of cal2prompt or Windsurf may be running.".to_string(),
                )
                .await);
            }

            return Err(self
                .send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::InternalError,
                    format!("Failed to refresh token: {}", err),
                )
                .await);
        }

        Ok(())
    }

    async fn handle_generic_request(
        &self,
        transport: &StdioTransport,
        id: u64,
        method: String,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        if let Err(err) = self.handle_request(transport, id, method, params).await {
            eprintln!("[SERVER] Error handling request: {:?}", err);
            return self
                .send_error_response(
                    transport,
                    id,
                    JsonRpcErrorCode::InternalError,
                    format!("Failed to handle request: {}", err),
                )
                .await;
        }
        Ok(())
    }

    async fn handle_request(
        &self,
        transport: &StdioTransport,
        id: u64,
        method: String,
        params: Option<serde_json::Value>,
    ) -> anyhow::Result<()> {
        match &*method {
            "initialize" => self.handle_initialize(transport, id).await?,
            "tools/list" => self.handle_tools_list(transport, id).await?,
            "tools/call" => {
                if let Some(params_val) = params {
                    self.handle_tools_call(transport, id, params_val).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_initialize(&self, transport: &StdioTransport, id: u64) -> anyhow::Result<()> {
        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!({
                "capabilities": {
                    "experimental": {},
                    "prompts": { "listChanged": false },
                    "resources": { "listChanged": false, "subscribe": false },
                    "tools": { "listChanged": false }
                },
                "protocolVersion": "2024-11-05",
                "serverInfo": {
                    "name": "cal2prompt",
                    "version": "0.1.0" // FIXME: get version from Cargo.toml
                }
            })),
            error: None,
        };
        transport.send(response).await?;
        Ok(())
    }

    async fn handle_tools_list(&self, transport: &StdioTransport, id: u64) -> anyhow::Result<()> {
        let tools_value: serde_json::Value =
            serde_json::from_str(TOOLS_JSON).expect("tools.json must be valid JSON");

        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(tools_value),
            error: None,
        };

        transport.send(response).await?;
        Ok(())
    }

    async fn handle_tools_call(
        &self,
        transport: &StdioTransport,
        id: u64,
        params_val: serde_json::Value,
    ) -> anyhow::Result<()> {
        let tool_name = match params_val.get("name").and_then(Value::as_str) {
            Some(name) => name,
            None => return Ok(()),
        };

        match tool_name {
            "list_calendar_events" => {
                self.handle_list_calendar_events(transport, id, &params_val)
                    .await?
            }
            "insert_calendar_event" => {
                self.handle_insert_calendar_event(transport, id, &params_val)
                    .await?
            }
            _ => {}
        }

        Ok(())
    }

    async fn handle_list_calendar_events(
        &self,
        transport: &StdioTransport,
        id: u64,
        params_val: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let since_str = params_val
            .pointer("/arguments/since")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'since' parameter"))?;

        let until_str = params_val
            .pointer("/arguments/until")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'until' parameter"))?;

        let account = params_val
            .pointer("/arguments/profile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        match self
            .cal2prompt
            .fetch_days(since_str, until_str, account.map(|p| p.to_string()))
            .await
        {
            Ok(days) => {
                let result = self.cal2prompt.render_days(days)?;
                self.send_text_response(transport, id, &result).await?;
            }
            Err(e) => {
                return self
                    .send_error_response(
                        transport,
                        id,
                        JsonRpcErrorCode::InternalError,
                        format!("Failed to fetch calendar events: {}", e),
                    )
                    .await;
            }
        }

        Ok(())
    }

    async fn handle_insert_calendar_event(
        &self,
        transport: &StdioTransport,
        id: u64,
        params_val: &serde_json::Value,
    ) -> anyhow::Result<()> {
        let summary_str = params_val
            .pointer("/arguments/summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'summary' parameter"))?;

        let start_str = params_val
            .pointer("/arguments/start")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'start' parameter"))?;

        let end_str = params_val
            .pointer("/arguments/end")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'end' parameter"))?;

        let description_str = params_val
            .pointer("/arguments/description")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let account: Option<String> = params_val
            .pointer("/arguments/profile")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        match self
            .cal2prompt
            .insert_event(summary_str, description_str, start_str, end_str, account)
            .await
        {
            Ok(response) => {
                let result = format!(
                    "Event created successfully!\nLink: {}",
                    response
                        .html_link
                        .unwrap_or_else(|| "No link available".to_string())
                );
                self.send_text_response(transport, id, &result).await?;
            }
            Err(e) => match e.downcast::<CalendarServiceError>() {
                Ok(CalendarServiceError::AccountNotFound(account)) => {
                    self.send_error_response(
                        transport,
                        id,
                        JsonRpcErrorCode::AccountNotFound,
                        format!("Account '{}' not found", account),
                    )
                    .await?;
                }
                Ok(CalendarServiceError::NoCalendarId) => {
                    self.send_error_response(
                        transport,
                        id,
                        JsonRpcErrorCode::InvalidParams,
                        "No calendar ID configured".to_string(),
                    )
                    .await?;
                }
                Err(e) => {
                    self.send_error_response(
                        transport,
                        id,
                        JsonRpcErrorCode::InternalError,
                        format!("Unexpected error: {}", e),
                    )
                    .await?;
                }
            },
        }

        Ok(())
    }

    async fn send_text_response(
        &self,
        transport: &StdioTransport,
        id: u64,
        text: &str,
    ) -> anyhow::Result<()> {
        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(json!({
                "content": [{
                    "type": "text",
                    "text": text,
                }],
            })),
            error: None,
        };
        transport.send(response).await?;
        Ok(())
    }

    async fn send_error_response(
        &self,
        transport: &StdioTransport,
        id: u64,
        code: JsonRpcErrorCode,
        message: String,
    ) -> anyhow::Result<()> {
        let response = Message::Response {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(json!({
                "code": code as i32,
                "message": message,
            })),
        };
        transport.send(response).await?;
        Ok(())
    }
}
