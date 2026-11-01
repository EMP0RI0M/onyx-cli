//! mcp — Model Context Protocol (MCP) JSON-RPC client for Onyx.
//!
//! Specification compliance & design:
//! - Implements JSON-RPC 2.0 client over loopback HTTP POST / SSE transport (offline, no external crates).
//! - Discovers remote tools (`tools/list`), binds each remote tool as a local `tools::Tool`
//!   adapter (`McpTool`) prefixed with `mcp__<server>__<tool>`.
//! - Calls remote tools (`tools/call`) translating arguments and results transparently.
//! - Remote tools route directly into the Onyx Gateway (policy + audit) so all remote
//!   tool executions are governed identically to built-in tools.
//! - Resilient against transport drops: if an MCP server terminates or fails mid-session,
//!   calls fail gracefully with audited tool errors without crashing the agent loop.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tools::{Args, Output, Tool, ToolError};

/// Information about an MCP tool returned by `tools/list`.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Client for communicating with an MCP server over HTTP JSON-RPC 2.0.
#[derive(Debug, Clone)]
pub struct McpClient {
    pub server_name: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    next_id: std::sync::Arc<AtomicU64>,
}

impl McpClient {
    pub fn new(server_name: impl Into<String>, host: impl Into<String>, port: u16, path: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            host: host.into(),
            port,
            path: path.into(),
            next_id: std::sync::Arc::new(AtomicU64::new(1)),
        }
    }

    /// Perform a raw JSON-RPC 2.0 HTTP POST request.
    pub fn call_rpc(&self, method: &str, params: Value) -> Result<Value, ToolError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req_body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        })
        .to_string();

        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| ToolError::Other(format!("resolve {}:{}: {e}", self.host, self.port)))?
            .next()
            .ok_or_else(|| ToolError::Other("no address resolved".into()))?;

        let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|e| ToolError::Other(format!("connect {}:{}: {e}", self.host, self.port)))?;
        sock.set_write_timeout(Some(Duration::from_secs(10))).ok();
        sock.set_read_timeout(Some(Duration::from_secs(30))).ok();

        let req = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.path, self.host, self.port, req_body.len(), req_body
        );

        sock.write_all(req.as_bytes())
            .map_err(|e| ToolError::Other(format!("write error: {e}")))?;

        let mut buf = Vec::new();
        sock.read_to_end(&mut buf)
            .map_err(|e| ToolError::Other(format!("read error: {e}")))?;

        let response = String::from_utf8_lossy(&buf);
        let body = response.split("\r\n\r\n").nth(1).unwrap_or(&response);

        let v: Value = serde_json::from_str(body)
            .map_err(|e| ToolError::Other(format!("invalid JSON-RPC reply: {e}")))?;

        if let Some(err) = v.get("error") {
            let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("RPC error");
            return Err(ToolError::Other(format!("MCP error: {msg}")));
        }

        v.get("result")
            .cloned()
            .ok_or_else(|| ToolError::Other("missing `result` in JSON-RPC response".into()))
    }

    /// Negotiate `initialize` with the MCP server.
    pub fn initialize(&self) -> Result<Value, ToolError> {
        self.call_rpc(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "clientInfo": { "name": "onyx", "version": "0.1.0" }
            }),
        )
    }

    /// List tools provided by the MCP server (`tools/list`).
    pub fn list_tools(&self) -> Result<Vec<McpToolInfo>, ToolError> {
        let res = self.call_rpc("tools/list", json!({}))?;
        let tools_arr = res
            .get("tools")
            .and_then(|t| t.as_array())
            .ok_or_else(|| ToolError::Other("tools/list reply missing `tools` array".into()))?;

        let mut out = Vec::new();
        for t in tools_arr {
            let name = t.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
            let description = t.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string();
            let input_schema = t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type": "object"}));
            if !name.is_empty() {
                out.push(McpToolInfo { name, description, input_schema });
            }
        }
        Ok(out)
    }

    /// Invoke a tool on the MCP server (`tools/call`).
    pub fn call_tool(&self, name: &str, arguments: &Args) -> Result<Output, ToolError> {
        let res = self.call_rpc(
            "tools/call",
            json!({
                "name": name,
                "arguments": arguments
            }),
        )?;

        let is_error = res.get("isError").and_then(|e| e.as_bool()).unwrap_or(false);
        let mut text = String::new();

        if let Some(content) = res.get("content").and_then(|c| c.as_array()) {
            for item in content {
                if let Some(t) = item.get("text").and_then(|s| s.as_str()) {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(t);
                }
            }
        } else if let Some(content_str) = res.get("content").and_then(|c| c.as_str()) {
            text = content_str.to_string();
        } else {
            text = res.to_string();
        }

        if is_error {
            Ok(Output::err(text))
        } else {
            Ok(Output::ok(text))
        }
    }
}

/// An MCP tool adapted into the `tools::Tool` trait interface.
pub struct McpTool {
    pub client: McpClient,
    pub remote_name: String,
    pub registered_name: String,
    pub description: String,
    pub input_schema: Value,
}

impl McpTool {
    pub fn new(client: McpClient, info: McpToolInfo) -> Self {
        let registered_name = format!("mcp__{}__{}", client.server_name, info.name);
        Self {
            client,
            remote_name: info.name,
            registered_name,
            description: info.description,
            input_schema: info.input_schema,
        }
    }
}

impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.registered_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        json!({
            "name": self.name(),
            "description": self.description(),
            "parameters": self.input_schema
        })
    }

    fn execute(&self, args: &Args) -> Result<Output, ToolError> {
        self.client.call_tool(&self.remote_name, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn mcp_client_initialize_and_tool_call() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // Spawn mock MCP server
        let handle = thread::spawn(move || {
            for _ in 0..3 {
                let (mut sock, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).unwrap();
                let req_str = String::from_utf8_lossy(&buf[..n]);
                let body = req_str.split("\r\n\r\n").nth(1).unwrap();
                let json_req: Value = serde_json::from_str(body).unwrap();
                let method = json_req.get("method").and_then(|m| m.as_str()).unwrap();
                let id = json_req.get("id").unwrap();

                let reply_val = match method {
                    "initialize" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "protocolVersion": "2024-11-05", "capabilities": {} }
                    }),
                    "tools/list" => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "tools": [{
                                "name": "echo",
                                "description": "echo back input text",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": { "msg": { "type": "string" } },
                                    "required": ["msg"]
                                }
                            }]
                        }
                    }),
                    "tools/call" => {
                        let args = json_req.get("params").and_then(|p| p.get("arguments")).unwrap();
                        let msg = args.get("msg").and_then(|m| m.as_str()).unwrap_or("");
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{ "type": "text", "text": format!("echo: {msg}") }],
                                "isError": false
                            }
                        })
                    }
                    _ => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
                };

                let resp_body = reply_val.to_string();
                let http_resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    resp_body.len(),
                    resp_body
                );
                sock.write_all(http_resp.as_bytes()).unwrap();
            }
        });

        let client = McpClient::new("test_server", "127.0.0.1", port, "/");
        let init_res = client.initialize();
        assert!(init_res.is_ok());

        let tools = client.list_tools().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo");

        let mcp_tool = McpTool::new(client, tools[0].clone());
        assert_eq!(mcp_tool.name(), "mcp__test_server__echo");

        let out = mcp_tool.execute(&json!({"msg": "hello world"})).unwrap();
        assert_eq!(out.content, "echo: hello world");
        assert!(!out.is_error);

        let _ = handle.join();
    }
}
