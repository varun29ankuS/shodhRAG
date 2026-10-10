use super::*;
use tokio::io::{AsyncReadExt, DuplexStream};
use tokio::net::TcpListener;

/// A scripted stdio server on the other end of `io`: answers initialize,
/// lists tools over two pages and echoes calls. Before answering the first
/// `tools/list` it sends a notification, a log line and a `ping`.
async fn scripted_server(io: DuplexStream, seen: Arc<Mutex<Vec<Value>>>) {
    let (reader, mut writer) = tokio::io::split(io);
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let message: Value = serde_json::from_str(&line).unwrap();
        lock(&seen).push(message.clone());
        let id = message["id"].clone();
        let reply = match message["method"].as_str() {
            Some("initialize") => json!({"jsonrpc": "2.0", "id": id, "result": {
                "protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                "serverInfo": {"name": "scripted", "version": "1"}}}),
            Some("tools/list") if message["params"]["cursor"].is_null() => {
                let notification =
                    json!({"jsonrpc": "2.0", "method": "notifications/message", "params": {}});
                let ping = json!({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"});
                let noise = format!("{notification}\nnot json at all\n{ping}\n");
                writer.write_all(noise.as_bytes()).await.unwrap();
                json!({"jsonrpc": "2.0", "id": id, "result": {
                    "tools": [{"name": "search", "inputSchema": {"type": "object"},
                               "annotations": {"readOnlyHint": true}}],
                    "nextCursor": "page-2"}})
            }
            Some("tools/list") => json!({"jsonrpc": "2.0", "id": id, "result": {
                "tools": [{"name": "delete", "description": "Deletes."}]}}),
            Some("tools/call") if message["params"]["name"] == "fail" => json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32602, "message": "bad token sk-live-secret"}}),
            Some("tools/call") => json!({"jsonrpc": "2.0", "id": id, "result": {
                "content": [{"type": "text", "text": format!("echo {}", message["params"]["arguments"])}]}}),
            Some("exit") => break,
            _ => continue,
        };
        let line = format!("{reply}\n");
        writer.write_all(line.as_bytes()).await.unwrap();
    }
}

async fn connected() -> (McpClient, Arc<Mutex<Vec<Value>>>) {
    let (ours, theirs) = tokio::io::duplex(64 * 1024);
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    tokio::spawn(scripted_server(theirs, seen.clone()));
    let (reader, writer) = tokio::io::split(ours);
    let client = McpClient::over_io(
        reader,
        writer,
        None,
        ["sk-live-secret".to_string()].into_iter(),
    )
    .initialise()
    .await
    .unwrap();
    (client, seen)
}

#[tokio::test]
async fn stdio_servers_are_initialised_listed_and_called() {
    let (client, seen) = connected().await;
    assert_eq!(client.server_name.as_deref(), Some("scripted"));
    let tools = client.list_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["search", "delete"], "both pages");
    assert!(tools[0].read_only() && !tools[0].destructive());
    assert!(!tools[1].read_only() && tools[1].destructive());
    assert_eq!(tools[1].input_schema, json!({"type": "object"}));
    let result = client.call_tool("search", json!({"q": 1})).await.unwrap();
    assert_eq!(
        result,
        CallResult {
            text: "echo {\"q\":1}".into(),
            is_error: false
        }
    );
    // Errors name the method and never carry a secret.
    match client.call_tool("fail", json!({})).await {
        Err(McpError::Rpc { method, message }) => {
            assert_eq!(method, "tools/call");
            assert_eq!(message, "bad token [REDACTED]");
        }
        other => panic!("{other:?}"),
    }
    let seen = lock(&seen).clone();
    assert_eq!(seen[0]["params"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(seen[1]["method"], "notifications/initialized");
    assert!(seen[1].get("id").is_none());
    // The server's ping was answered.
    assert!(seen
        .iter()
        .any(|m| m["id"] == "srv-1" && m["result"] == json!({})));
    assert!(client.is_alive());
}

#[tokio::test]
async fn a_server_that_exits_fails_pending_calls() {
    let (client, _) = connected().await;
    let outcome = client
        .request("exit", json!({}), Duration::from_secs(5))
        .await;
    assert!(matches!(outcome, Err(McpError::Closed(_))), "{outcome:?}");
    assert!(!client.is_alive());
    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Closed(_))
    ));
}

#[tokio::test]
async fn silent_servers_time_out() {
    let (client, _) = connected().await;
    let outcome = client
        .request("unknown/method", json!({}), Duration::from_millis(50))
        .await;
    assert_eq!(outcome, Err(McpError::Timeout("unknown/method".into())));
}

#[test]
fn call_results_become_text() {
    let result = call_result(&json!({
        "content": [
            {"type": "text", "text": "one"},
            {"type": "image", "data": "...", "mimeType": "image/png"},
            {"type": "resource", "resource": {"uri": "file:///a", "text": "two"}},
            {"type": "resource_link", "uri": "file:///b", "name": "b"}
        ],
        "isError": true
    }));
    assert_eq!(
        result.text,
        "one\n[image omitted]\ntwo\n[resource file:///b]"
    );
    assert!(result.is_error);
    let structured = call_result(&json!({"content": [], "structuredContent": {"n": 1}}));
    assert_eq!(structured.text, "{\"n\":1}");
}

#[test]
fn sse_events_are_split_and_decoded() {
    let stream: &[u8] =
        b"event: message\ndata: {\"id\":1,\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\r\n\r\n";
    let first = event_end(stream).unwrap();
    assert!(sse_message(&stream[..first]).is_none(), "broken JSON");
    let rest = &stream[first..];
    let end = event_end(rest).unwrap();
    assert_eq!(sse_message(&rest[..end]).unwrap()["id"], 2);
    assert_eq!(event_end(b"data: x"), None);
}

type HttpRequest = (String, Vec<(String, String)>, Value);

/// One HTTP request: (request line, headers lower-cased, body).
async fn read_http(stream: &mut tokio::net::TcpStream) -> HttpRequest {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let mut lines = head.lines();
    let request = lines.next().unwrap().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).await.unwrap();
    (
        request,
        headers,
        serde_json::from_slice(&body).unwrap_or(Value::Null),
    )
}

fn http_response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn streamable_http_servers_answer_with_json_or_sse() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Arc<Mutex<Vec<HttpRequest>>> = Arc::default();
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (request, headers, body) = read_http(&mut stream).await;
            lock(&log).push((request, headers, body.clone()));
            let id = body["id"].clone();
            let response = match body["method"].as_str() {
                Some("initialize") => {
                    let json = json!({"jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": "2025-03-26", "serverInfo": {"name": "web"}}});
                    http_response(
                        "200 OK",
                        "Content-Type: application/json\r\nMcp-Session-Id: sess-9\r\n",
                        &json.to_string(),
                    )
                }
                Some("tools/list") => {
                    let other =
                        json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}});
                    let reply = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "lookup"}]}});
                    let sse = format!(
                        "event: message\ndata: {other}\n\nevent: message\ndata: {reply}\n\n"
                    );
                    http_response("200 OK", "Content-Type: text/event-stream\r\n", &sse)
                }
                _ => http_response("202 Accepted", "", ""),
            };
            stream.write_all(response.as_bytes()).await.unwrap();
            let _ = stream.shutdown().await;
        }
    });
    let transport = Transport::Http {
        url: format!("http://127.0.0.1:{port}/mcp"),
        headers: [("Authorization".to_string(), "Bearer tok-secret".to_string())].into(),
    };
    let client = McpClient::connect(&transport).await.unwrap();
    assert_eq!(client.server_name.as_deref(), Some("web"));
    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools[0].name, "lookup");
    let seen = lock(&seen).clone();
    let header = |i: usize, name: &str| {
        seen[i]
            .1
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        header(0, "authorization").as_deref(),
        Some("Bearer tok-secret")
    );
    assert_eq!(header(0, "mcp-session-id"), None);
    // The notification and later requests carry the session and protocol.
    assert_eq!(seen[1].2["method"], "notifications/initialized");
    assert_eq!(header(2, "mcp-session-id").as_deref(), Some("sess-9"));
    assert_eq!(
        header(2, "mcp-protocol-version").as_deref(),
        Some("2025-03-26")
    );
    assert!(!format!("{transport:?}").contains("tok-secret"));
}

#[test]
fn bare_commands_are_found_with_their_windows_extension() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("npx.cmd"), "@echo off").unwrap();
    let path = dir.path().display().to_string();
    let resolved = resolve_command("npx", Some(&path), Some(".EXE;.CMD"));
    if cfg!(windows) {
        assert_eq!(resolved, dir.path().join("npx.cmd"));
    } else {
        assert_eq!(resolved, PathBuf::from("npx"));
    }
    // Paths and names with an extension are used as given.
    assert_eq!(
        resolve_command("C:/x/y.exe", Some(&path), None),
        PathBuf::from("C:/x/y.exe")
    );
    assert_eq!(
        resolve_command("missing", Some(&path), None),
        PathBuf::from("missing")
    );
}
