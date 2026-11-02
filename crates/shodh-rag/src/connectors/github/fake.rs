//! A local HTTP server that answers like GitHub, so the connector is tested without
//! GitHub (no test ever calls github.com).

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One request the server received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub head: String,
    pub body: String,
}

pub type Log = Arc<Mutex<Vec<Seen>>>;

/// One answer: a status, extra headers and a body.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    pub fn empty(status: u16) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: String::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Status",
    }
}

/// Serves `replies` in order, one per request; returns the base URL (`http://…/`) and
/// the requests seen.
pub async fn serve(replies: Vec<Reply>) -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/", listener.local_addr().unwrap());
    let seen: Log = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        for reply in replies {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buffer = Vec::new();
            let mut chunk = [0u8; 4096];
            let (head, mut rest) = loop {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break (String::from_utf8_lossy(&buffer).to_string(), Vec::new());
                }
                buffer.extend_from_slice(&chunk[..n]);
                if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buffer[..at]).to_string();
                    break (head, buffer[at + 4..].to_vec());
                }
            };
            let length = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while rest.len() < length {
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                rest.extend_from_slice(&chunk[..n]);
            }
            let mut first = head.lines().next().unwrap_or_default().split_whitespace();
            let method = first.next().unwrap_or_default().to_string();
            let path = first.next().unwrap_or_default().to_string();
            log.lock().unwrap().push(Seen {
                method,
                path,
                head: head.clone(),
                body: String::from_utf8_lossy(&rest).to_string(),
            });
            let mut out = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                reply.status,
                reason(reply.status),
                reply.body.len()
            );
            for (k, v) in &reply.headers {
                out.push_str(&format!("{k}: {v}\r\n"));
            }
            out.push_str("\r\n");
            out.push_str(&reply.body);
            socket.write_all(out.as_bytes()).await.unwrap();
            socket.shutdown().await.ok();
        }
    });
    (base, seen)
}
