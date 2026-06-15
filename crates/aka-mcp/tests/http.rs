//! Streamable HTTP MCP endpoint smoke tests.

use std::sync::Arc;

use aka_mcp::{serve_http, Backend};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

mod support;

use support::fixture_backend::FixtureBackend;

#[tokio::test]
async fn initialize_over_streamable_http() -> anyhow::Result<()> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let addr = listener.local_addr()?;
    drop(listener);

    let server = tokio::spawn(async move {
        let backend = Arc::new(FixtureBackend::fixture()) as Arc<dyn Backend>;
        let _ = serve_http(backend, addr).await;
    });

    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"aka-http-test","version":"1.0"}}}"#;
    let request = format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: {addr}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        body.len()
    );

    let mut last_err = None;
    for _ in 0..50 {
        match tokio::net::TcpStream::connect(addr).await {
            Ok(mut stream) => {
                stream.write_all(request.as_bytes()).await?;
                let mut text = String::new();
                stream.read_to_string(&mut text).await?;
                assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
                assert!(text.contains(r#""name":"aka-mcp""#), "{text}");
                server.abort();
                return Ok(());
            }
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }

    server.abort();
    Err(last_err.expect("request attempted").into())
}
