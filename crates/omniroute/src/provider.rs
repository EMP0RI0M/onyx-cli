use crate::protocol::{ChatCompletionRequest, ChatCompletionResponse};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

pub trait ProviderClient: Send + Sync {
    fn id(&self) -> &str;
    fn forward(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse, String>;
}

pub struct OpenAiCompatClient {
    pub id: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub api_key: String,
}

impl OpenAiCompatClient {
    pub fn new(id: impl Into<String>, host: impl Into<String>, port: u16, path: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            host: host.into(),
            port,
            path: path.into(),
            api_key: api_key.into(),
        }
    }
}

impl ProviderClient for OpenAiCompatClient {
    fn id(&self) -> &str {
        &self.id
    }

    fn forward(&self, req: &ChatCompletionRequest) -> Result<ChatCompletionResponse, String> {
        let body = serde_json::to_string(req).map_err(|e| e.to_string())?;
        let addr = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|e| format!("resolve: {e}"))?
            .next()
            .ok_or_else(|| "no address".to_string())?;

        let mut sock = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
            .map_err(|e| format!("connect: {e}"))?;
        sock.set_read_timeout(Some(Duration::from_secs(60))).ok();
        sock.set_write_timeout(Some(Duration::from_secs(15))).ok();

        let mut req_str = format!(
            "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.path, self.host, body.len()
        );
        if !self.api_key.is_empty() {
            req_str.push_str(&format!("Authorization: Bearer {}\r\n", self.api_key));
        }
        req_str.push_str("\r\n");
        req_str.push_str(&body);

        sock.write_all(req_str.as_bytes()).map_err(|e| e.to_string())?;

        let mut resp_bytes = Vec::new();
        sock.read_to_end(&mut resp_bytes).map_err(|e| e.to_string())?;
        let resp_str = String::from_utf8_lossy(&resp_bytes);

        let parts: Vec<&str> = resp_str.splitn(2, "\r\n\r\n").collect();
        if parts.len() < 2 {
            return Err("Malformed HTTP response".into());
        }

        let resp: ChatCompletionResponse = serde_json::from_str(parts[1])
            .map_err(|e| format!("parse response: {e}"))?;
        Ok(resp)
    }
}
