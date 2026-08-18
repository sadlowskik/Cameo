//! A minimal HTTP/1.1 reverse proxy for the `/v1` gateway (F8).
//!
//! The gateway is one stable OpenAI-compatible front door: a client hits
//! `cameod`'s `/v1/*`, and this module forwards the request to the supervised
//! `llama-server` that serves the requested model, returning its response. Like
//! the rest of `cameod` it links no HTTP stack — it speaks HTTP/1.1 over
//! [`std::net::TcpStream`] directly, the same dependency-light stance as
//! [`crate::http`].
//!
//! The response is buffered (read to EOF under `Connection: close`) rather than
//! streamed, because the server side ([`crate::http`]) hands back a whole
//! [`crate::http::Response`]. That is correct for the common non-streaming
//! completion; token streaming through the gateway is a later refinement noted in
//! the roadmap. The wire parsing is pure and unit-tested; only [`forward`] touches
//! a socket.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// How long the proxy waits on a backend before giving up — generous, because a
/// cold model load behind `llama-server` can take a while.
const PROXY_TIMEOUT: Duration = Duration::from_secs(300);

/// A backend response, reduced to what the gateway re-emits to the client.
pub struct BackendResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// Forward a request to `host:port` and return the backend's response.
///
/// `path` is passed through unchanged (the backend `llama-server` serves the same
/// `/v1/...` paths). `backend_key`, when set, is injected as a bearer token — the
/// gateway holds the endpoint's serve key so clients present one key to one door.
pub fn forward(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    content_type: &str,
    body: &[u8],
    backend_key: Option<&str>,
) -> std::io::Result<BackendResponse> {
    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(PROXY_TIMEOUT))?;
    stream.set_write_timeout(Some(PROXY_TIMEOUT))?;

    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\n\
         Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(key) = backend_key {
        head.push_str(&format!("Authorization: Bearer {key}\r\n"));
    }
    head.push_str("\r\n");

    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()?;
    // The request is complete; half-close the write side so a `Connection: close`
    // backend reads a clean EOF and then sends its response.
    let _ = stream.shutdown(std::net::Shutdown::Write);

    // Read the response to the end. A backend that drops the socket after writing
    // surfaces as a clean EOF on POSIX but as `ConnectionReset` on Windows; both
    // mean "the response is complete," so treat the reset as end-of-body.
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => return Err(e),
        }
    }
    Ok(parse_backend_response(&raw))
}

/// Parse a raw backend HTTP response into status + content-type + body. Pure, so
/// it is unit-tested without a socket. Handles `Content-Length`, `chunked`, and
/// close-delimited bodies.
fn parse_backend_response(raw: &[u8]) -> BackendResponse {
    let (head, body_raw) = match find_subslice(raw, b"\r\n\r\n") {
        Some(i) => (&raw[..i], &raw[i + 4..]),
        None => (raw, &[][..]),
    };

    let head_str = String::from_utf8_lossy(head);
    let mut lines = head_str.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(502);

    let mut content_type = "application/json".to_string();
    let mut chunked = false;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            if key == "content-type" {
                content_type = val.to_string();
            } else if key == "transfer-encoding" && val.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }

    let body = if chunked {
        dechunk(body_raw)
    } else {
        body_raw.to_vec()
    };
    BackendResponse {
        status,
        content_type,
        body,
    }
}

/// Decode an HTTP `chunked` transfer body into the raw bytes.
fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(nl) = find_subslice(data, b"\r\n") {
        let size_line = String::from_utf8_lossy(&data[..nl]);
        // A chunk size may carry `;ext` extensions; the size is the hex before it.
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16)
            .unwrap_or(0);
        data = &data[nl + 2..];
        if size == 0 {
            break;
        }
        if data.len() < size {
            out.extend_from_slice(data);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    out
}

/// First index of `needle` in `hay`, or `None`.
fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn parses_a_content_length_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"ok\":true}\r\n";
        let r = parse_backend_response(raw);
        assert_eq!(r.status, 200);
        assert_eq!(r.content_type, "application/json");
        assert!(String::from_utf8_lossy(&r.body).contains("\"ok\":true"));
    }

    #[test]
    fn decodes_a_chunked_response() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let r = parse_backend_response(raw);
        assert_eq!(r.status, 200);
        assert_eq!(String::from_utf8_lossy(&r.body), "hello world");
    }

    #[test]
    fn missing_status_line_is_a_bad_gateway() {
        let r = parse_backend_response(b"garbage with no status");
        assert_eq!(r.status, 502);
    }

    #[test]
    fn forward_round_trips_against_a_mock_backend() {
        // A one-shot backend that echoes a canned OpenAI-ish response and records
        // that the request carried the injected bearer token and the body.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // forward() half-closes its write side after the request, so reading to
            // EOF here yields the whole request cleanly.
            let mut req_bytes = Vec::new();
            let mut buf = [0u8; 1024];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => req_bytes.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
            let req = String::from_utf8_lossy(&req_bytes).to_string();
            let body = br#"{"id":"chatcmpl-1","object":"chat.completion"}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(resp.as_bytes()).unwrap();
            sock.write_all(body).unwrap();
            req
        });

        let r = forward(
            "127.0.0.1",
            addr.port(),
            "POST",
            "/v1/chat/completions",
            "application/json",
            br#"{"model":"tinyllama"}"#,
            Some("s3cret"),
        )
        .unwrap();

        assert_eq!(r.status, 200);
        assert!(String::from_utf8_lossy(&r.body).contains("chat.completion"));

        let seen = handle.join().unwrap();
        assert!(seen.contains("POST /v1/chat/completions HTTP/1.1"));
        assert!(seen.contains("Authorization: Bearer s3cret"));
        assert!(seen.contains(r#"{"model":"tinyllama"}"#));
    }
}
