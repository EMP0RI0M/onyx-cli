use crate::protocol::ChatCompletionRequest;
use crate::router::Router;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

pub struct OmniServer {
    pub addr: String,
    pub router: Arc<Router>,
}

impl OmniServer {
    pub fn new(addr: impl Into<String>, router: Arc<Router>) -> Self {
        Self {
            addr: addr.into(),
            router,
        }
    }

    pub fn run(&self) -> Result<(), String> {
        let listener = TcpListener::bind(&self.addr).map_err(|e| format!("bind {}: {e}", self.addr))?;
        println!("OmniRoute daemon listening on {}", self.addr);

        for stream in listener.incoming() {
            if let Ok(stream) = stream {
                let r = self.router.clone();
                thread::spawn(move || {
                    let _ = handle_client(stream, r);
                });
            }
        }
        Ok(())
    }
}

fn handle_client(mut stream: TcpStream, router: Arc<Router>) -> Result<(), String> {
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(|e| e.to_string())?;

    let parts: Vec<String> = request_line.split_whitespace().map(|s| s.to_string()).collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = &parts[0];
    let path = &parts[1];

    let mut content_length = 0;
    let mut header_line = String::new();
    loop {
        header_line.clear();
        reader.read_line(&mut header_line).map_err(|e| e.to_string())?;
        if header_line == "\r\n" || header_line == "\n" || header_line.is_empty() {
            break;
        }
        if let Some((k, v)) = header_line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }

    if method == "GET" && (path == "/health" || path == "/v1/health") {
        let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK";
        stream.write_all(resp.as_bytes()).ok();
        return Ok(());
    }

    if method == "POST" && (path == "/v1/chat/completions" || path == "/chat/completions") {
        let mut body = vec![0u8; content_length];
        std::io::Read::read_exact(&mut reader, &mut body).map_err(|e| e.to_string())?;

        let req: ChatCompletionRequest = match serde_json::from_slice(&body) {
            Ok(r) => r,
            Err(e) => {
                let err_msg = format!("HTTP/1.1 400 Bad Request\r\nContent-Length: {}\r\n\r\n{e}", e.to_string().len());
                stream.write_all(err_msg.as_bytes()).ok();
                return Ok(());
            }
        };

        let decision = match router.route(&req) {
            Ok(d) => d,
            Err(e) => {
                let err_msg = format!("HTTP/1.1 502 Bad Gateway\r\nContent-Length: {}\r\n\r\n{e}", e.len());
                stream.write_all(err_msg.as_bytes()).ok();
                return Ok(());
            }
        };

        let mut forward_req = req.clone();
        forward_req.model = decision.target.mapped_model.clone();

        match decision.target.provider.forward(&forward_req) {
            Ok(res) => {
                let res_json = serde_json::to_string(&res).unwrap_or_default();
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    res_json.len(), res_json
                );
                stream.write_all(out.as_bytes()).ok();
            }
            Err(e) => {
                let err_msg = format!("HTTP/1.1 502 Bad Gateway\r\nContent-Length: {}\r\n\r\n{e}", e.len());
                stream.write_all(err_msg.as_bytes()).ok();
            }
        }
    }

    Ok(())
}
