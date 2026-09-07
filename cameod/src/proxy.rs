//! A minimal HTTP/1.1 reverse proxy for the `/v1` gateway (F8).
//!
//! The gateway is one stable OpenAI-compatible front door: a client hits
//! `cameod`'s `/v1/*`, and this module forwards the request to the supervised
//! `llama-server` that serves the requested model, returning its response. Like
//! the rest of `cameod` it links no HTTP stack — it speaks HTTP/1.1 over
//! [`std::net::TcpStream`] directly, the same dependency-light stance as
//! [`crate::http`].
//!
//! Two paths share one request-writer ([`connect_and_send`]):
//! - [`forward`] buffers the whole upstream response (read to EOF under
//!   `Connection: close`) and returns it as a [`BackendResponse`]. Correct for a
//!   normal, non-streaming completion.
//! - [`forward_streaming`] relays the upstream bytes verbatim to the client
//!   socket, flushing each chunk, so an OpenAI `stream: true` SSE response reaches
//!   the caller token-by-token instead of as one final blob.
//!
//! The wire parsing is pure and unit-tested; only the two forwarders touch a
//! socket.

use std::io::{Read, Write};
use std::net::TcpStream;
type Control<'a> = Option<(&'a crate::drain::Drain, u64)>;
fn cancelled(control: Control) -> bool {
    control.is_some_and(|(drain, epoch)| drain.cancelled(epoch))
}
fn cancelled_error() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Interrupted, "drain deadline exceeded")
}
use std::time::Duration;

/// How long the proxy waits on a backend before giving up — generous, because a
/// cold model load behind `llama-server` can take a while.
const PROXY_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_BUFFERED_RESPONSE: usize = 32 * 1024 * 1024;
const MAX_RESPONSE_HEADERS: usize = 64 * 1024;

fn invalid_response() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "invalid or oversized upstream response",
    )
}

/// A backend response, reduced to what the gateway re-emits to the client.
pub struct BackendResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// One upstream request, shared by the buffered and streaming forwarders. `path`
/// is passed through unchanged (the backend `llama-server` serves the same
/// `/v1/...` paths); `backend_key`, when set, is injected as a bearer token so
/// clients present one key to one door.
pub struct ProxyRequest<'a> {
    pub host: &'a str,
    pub port: u16,
    pub method: &'a str,
    pub path: &'a str,
    pub content_type: &'a str,
    pub body: &'a [u8],
    pub backend_key: Option<&'a str>,
}

/// Forward a request and return the backend's fully-buffered response.
#[cfg(test)]
pub fn forward(req: &ProxyRequest) -> std::io::Result<BackendResponse> {
    forward_controlled(req, None)
}

pub fn forward_controlled(
    req: &ProxyRequest,
    control: Control,
) -> std::io::Result<BackendResponse> {
    let (mut stream, _registration) = connect_and_send(req, control)?;

    // Read the response to the end. A backend that drops the socket after writing
    // surfaces as a clean EOF on POSIX but as `ConnectionReset` on Windows; both
    // mean "the response is complete," so treat the reset as end-of-body.
    let mut raw = Vec::new();
    let mut buf = [0u8; 8192];
    let mut last_progress = std::time::Instant::now();
    loop {
        let read = stream.read(&mut buf);
        if cancelled(control) {
            return Err(cancelled_error());
        }
        if matches!(&read, Err(error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))
            && last_progress.elapsed() < PROXY_TIMEOUT
        {
            continue;
        }
        match read {
            Ok(0) => break,
            Ok(n) => {
                last_progress = std::time::Instant::now();
                if raw.len() + n > MAX_BUFFERED_RESPONSE {
                    return Err(invalid_response());
                }
                raw.extend_from_slice(&buf[..n]);
                if find_subslice(&raw, b"\r\n\r\n").unwrap_or(raw.len()) > MAX_RESPONSE_HEADERS {
                    return Err(invalid_response());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => return Err(e),
        }
    }
    parse_backend_response(&raw)
}

/// Forward a request and relay the backend's response **verbatim** to `sink`,
/// flushing after every read so a `text/event-stream` (SSE) body reaches the
/// client token-by-token instead of as one blob (F8 streaming). Unlike [`forward`]
/// this does not parse or re-frame: the backend's own status line, headers, and
/// (chunked) body are copied straight through, which is exactly what an
/// OpenAI-compatible `stream: true` client expects.
///
/// Errors that occur *before* any backend byte is relayed (a dead or unreachable
/// endpoint) are turned into a self-contained `502` written to `sink`, so the
/// client still gets a well-formed HTTP response. An error mid-stream can only be
/// signalled by dropping the connection — the headers are already gone — so it is
/// best-effort from there.
#[cfg(test)]
pub fn forward_streaming(req: &ProxyRequest, sink: &mut dyn Write) -> std::io::Result<()> {
    forward_streaming_controlled(req, sink, None)
}

pub fn forward_streaming_controlled(
    req: &ProxyRequest,
    sink: &mut dyn Write,
    control: Control,
) -> std::io::Result<()> {
    let (mut stream, _registration) = match connect_and_send(req, control) {
        Ok(s) => s,
        Err(_) if cancelled(control) => return write_drain_error(sink),
        Err(_) => return write_gateway_error(sink),
    };

    // Relay as bytes arrive; flush each chunk so SSE events are not held back.
    let mut buf = [0u8; 8192];
    let mut last_progress = std::time::Instant::now();
    let mut relayed = false;
    loop {
        let read = stream.read(&mut buf);
        if cancelled(control) {
            if !relayed {
                return write_drain_error(sink);
            }
            return Err(cancelled_error());
        }
        if matches!(&read, Err(error) if matches!(error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock))
            && last_progress.elapsed() < PROXY_TIMEOUT
        {
            continue;
        }
        match read {
            Ok(0) => break,
            Ok(n) => {
                last_progress = std::time::Instant::now();
                relayed = true;
                sink.write_all(&buf[..n])?;
                sink.flush()?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            // Mid-stream backend error: headers are already relayed, so the only
            // honest signal left is to stop and let the connection close.
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Open a connection to the backend and write the request head + body, half-closing
/// the write side so a `Connection: close` backend reads a clean EOF. Shared by the
/// buffered and streaming paths.
fn connect_and_send(
    req: &ProxyRequest,
    control: Control,
) -> std::io::Result<(TcpStream, Option<crate::drain::SocketRegistration>)> {
    if cancelled(control) {
        return Err(cancelled_error());
    }
    let address = crate::resolve::backend(req.host, req.port, || cancelled(control))?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    let registration = control
        .map(|(drain, epoch)| drain.register(&stream, epoch))
        .transpose()?;
    stream.set_read_timeout(Some(if control.is_some() {
        Duration::from_millis(250)
    } else {
        PROXY_TIMEOUT
    }))?;
    stream.set_write_timeout(Some(PROXY_TIMEOUT))?;

    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\n\
         Content-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        req.method,
        req.path,
        req.host,
        req.port,
        req.content_type,
        req.body.len()
    );
    if let Some(key) = req.backend_key {
        head.push_str(&format!("Authorization: Bearer {key}\r\n"));
    }
    head.push_str("\r\n");

    stream.write_all(head.as_bytes())?;
    stream.write_all(req.body)?;
    stream.flush()?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok((stream, registration))
}

/// Write a self-contained `502` HTTP response to a streaming client whose upstream
/// could not be reached before any bytes were relayed.
fn write_drain_error(sink: &mut dyn Write) -> std::io::Result<()> {
    let body = r#"{"error":{"message":"drain deadline exceeded","type":"server_error","code":"node_draining"}}"#;
    write!(sink, "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nRetry-After: 5\r\n\r\n{}", body.len(), body)?;
    sink.flush()
}

fn write_gateway_error(sink: &mut dyn Write) -> std::io::Result<()> {
    let body = "{\"error\":{\"message\":\"upstream unavailable\",\"type\":\"server_error\",\"code\":\"upstream_unavailable\"}}";
    let head = format!(
        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    sink.write_all(head.as_bytes())?;
    sink.write_all(body.as_bytes())?;
    sink.flush()
}

/// Parse a raw backend HTTP response into status + content-type + body. Pure, so
/// it is unit-tested without a socket. Handles `Content-Length`, `chunked`, and
/// close-delimited bodies.
fn parse_backend_response(raw: &[u8]) -> std::io::Result<BackendResponse> {
    let (head, body_raw) = match find_subslice(raw, b"\r\n\r\n") {
        Some(i) => (&raw[..i], &raw[i + 4..]),
        None => return Err(invalid_response()),
    };
    if head.len() > MAX_RESPONSE_HEADERS || raw.len() > MAX_BUFFERED_RESPONSE {
        return Err(invalid_response());
    }
    let head_str = std::str::from_utf8(head).map_err(|_| invalid_response())?;
    let mut lines = head_str.split("\r\n");
    let mut status_line = lines
        .next()
        .ok_or_else(invalid_response)?
        .split_whitespace();
    if !matches!(status_line.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err(invalid_response());
    }
    let status: u16 = status_line
        .next()
        .and_then(|s| s.parse().ok())
        .filter(|s| (200..=599).contains(s))
        .ok_or_else(invalid_response)?;

    let mut content_type = "application/json".to_string();
    let mut chunked = false;
    let mut content_length = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim();
            if key == "content-type" {
                content_type = val.to_string();
            } else if key == "transfer-encoding" {
                if chunked || !val.eq_ignore_ascii_case("chunked") {
                    return Err(invalid_response());
                }
                chunked = true;
            } else if key == "content-length" {
                if content_length.is_some()
                    || val.is_empty()
                    || !val.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(invalid_response());
                }
                content_length = Some(val.parse::<usize>().map_err(|_| invalid_response())?);
            }
        } else {
            return Err(invalid_response());
        }
    }
    if chunked && content_length.is_some() {
        return Err(invalid_response());
    }
    let body = if chunked {
        dechunk(body_raw)?
    } else {
        if content_length.is_some_and(|length| length != body_raw.len()) {
            return Err(invalid_response());
        }
        body_raw.to_vec()
    };
    Ok(BackendResponse {
        status,
        content_type,
        body,
    })
}

/// Decode an HTTP `chunked` transfer body into the raw bytes.
fn dechunk(mut data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(nl) = find_subslice(data, b"\r\n") {
        let size_line = String::from_utf8_lossy(&data[..nl]);
        // A chunk size may carry `;ext` extensions; the size is the hex before it.
        let size_text = size_line.split(';').next().unwrap_or("");
        if size_text.is_empty() || !size_text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid_response());
        }
        let size = usize::from_str_radix(size_text, 16).map_err(|_| invalid_response())?;
        data = &data[nl + 2..];
        if size == 0 {
            // The zero chunk is followed by a trailer section and its empty line.
            loop {
                let end = find_subslice(data, b"\r\n").ok_or_else(invalid_response)?;
                if end == 0 {
                    return if data.len() == 2 {
                        Ok(out)
                    } else {
                        Err(invalid_response())
                    };
                }
                if !data[..end].contains(&b':') {
                    return Err(invalid_response());
                }
                data = &data[end + 2..];
            }
        }
        if data.len() < size || out.len().saturating_add(size) > MAX_BUFFERED_RESPONSE {
            return Err(invalid_response());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        } else {
            return Err(invalid_response());
        }
    }
    Err(invalid_response())
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
    #[test]
    fn midstream_drain_ends_with_error_without_appending_success_or_second_response() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let backend = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            std::io::Read::read_to_end(&mut socket, &mut Vec::new()).unwrap();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\ndata: partial\n\n",
                )
                .unwrap();
            let _ = stop_rx.recv_timeout(std::time::Duration::from_secs(3));
        });
        struct Sink {
            bytes: Vec<u8>,
            drain: crate::drain::Drain,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.bytes.extend_from_slice(bytes);
                self.drain.begin(std::time::Duration::ZERO);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let drain = crate::drain::Drain::default();
        let permit = drain.admit().unwrap();
        let mut sink = Sink {
            bytes: Vec::new(),
            drain: drain.clone(),
        };
        let req = super::ProxyRequest {
            host: "127.0.0.1",
            port,
            method: "POST",
            path: "/v1/chat/completions",
            content_type: "application/json",
            body: b"{}",
            backend_key: None,
        };
        let result = super::forward_streaming_controlled(
            &req,
            &mut sink,
            Some((&drain, permit.generation())),
        );
        stop_tx.send(()).unwrap();
        backend.join().unwrap();
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        let output = String::from_utf8(sink.bytes).unwrap();
        assert!(output.starts_with("HTTP/1.1 200"));
        assert!(!output.contains("503"));
        assert!(!output.contains("[DONE]"));
    }
    #[test]
    fn drain_cancels_silent_upstream_in_buffered_and_streaming_paths() {
        use std::sync::mpsc;
        for streaming in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let (ready_tx, ready_rx) = mpsc::channel();
            let (stop_tx, stop_rx) = mpsc::channel();
            let backend = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                std::io::Read::read_to_end(&mut socket, &mut request).unwrap();
                ready_tx.send(()).unwrap();
                let _ = stop_rx.recv_timeout(std::time::Duration::from_secs(3));
            });
            let drain = crate::drain::Drain::default();
            let permit = drain.admit().unwrap();
            let control = drain.clone();
            let (result_tx, result_rx) = mpsc::channel();
            let client = std::thread::spawn(move || {
                let req = super::ProxyRequest {
                    host: "127.0.0.1",
                    port,
                    method: "POST",
                    path: "/v1/chat/completions",
                    content_type: "application/json",
                    body: b"{}",
                    backend_key: None,
                };
                if streaming {
                    let mut sink = Vec::new();
                    super::forward_streaming_controlled(
                        &req,
                        &mut sink,
                        Some((&control, permit.generation())),
                    )
                    .unwrap();
                    assert!(String::from_utf8(sink).unwrap().starts_with("HTTP/1.1 503"));
                } else {
                    let error =
                        super::forward_controlled(&req, Some((&control, permit.generation())))
                            .err()
                            .unwrap();
                    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
                }
                drop(permit);
                result_tx.send(()).unwrap();
            });
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            drain.begin(std::time::Duration::ZERO);
            result_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert_eq!(drain.status()["state"], "drained");
            stop_tx.send(()).unwrap();
            client.join().unwrap();
            backend.join().unwrap();
        }
    }
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn parses_a_content_length_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"ok\":true}\r\n";
        let r = parse_backend_response(raw).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.content_type, "application/json");
        assert!(String::from_utf8_lossy(&r.body).contains("\"ok\":true"));
    }

    #[test]
    fn decodes_a_chunked_response() {
        let raw =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let r = parse_backend_response(raw).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(String::from_utf8_lossy(&r.body), "hello world");
    }

    #[test]
    fn missing_status_line_is_a_bad_gateway() {
        assert!(parse_backend_response(b"garbage with no status").is_err());
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

        let r = forward(&ProxyRequest {
            host: "127.0.0.1",
            port: addr.port(),
            method: "POST",
            path: "/v1/chat/completions",
            content_type: "application/json",
            body: br#"{"model":"tinyllama"}"#,
            backend_key: Some("s3cret"),
        })
        .unwrap();

        assert_eq!(r.status, 200);
        assert!(String::from_utf8_lossy(&r.body).contains("chat.completion"));

        let seen = handle.join().unwrap();
        assert!(seen.contains("POST /v1/chat/completions HTTP/1.1"));
        assert!(seen.contains("Authorization: Bearer s3cret"));
        assert!(seen.contains(r#"{"model":"tinyllama"}"#));
    }

    #[test]
    fn forward_streaming_relays_sse_verbatim() {
        // A backend that emits a chunked SSE stream. The relay must copy the whole
        // response — status line, headers, and all events — through untouched.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // Drain the request to EOF (the client half-closes its write side) before
            // replying, so the response is not raced by an abrupt socket close.
            let mut buf = [0u8; 1024];
            loop {
                match sock.read(&mut buf) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            let resp = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                        Connection: close\r\n\r\n\
                        data: {\"delta\":\"hel\"}\n\n\
                        data: {\"delta\":\"lo\"}\n\n\
                        data: [DONE]\n\n";
            sock.write_all(resp.as_bytes()).unwrap();
        });

        let mut sink = Vec::new();
        forward_streaming(
            &ProxyRequest {
                host: "127.0.0.1",
                port: addr.port(),
                method: "POST",
                path: "/v1/chat/completions",
                content_type: "application/json",
                body: br#"{"model":"tinyllama","stream":true}"#,
                backend_key: None,
            },
            &mut sink,
        )
        .unwrap();
        handle.join().unwrap();

        let out = String::from_utf8(sink).unwrap();
        assert!(out.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(out.contains("Content-Type: text/event-stream"));
        assert!(out.contains(r#"data: {"delta":"hel"}"#));
        assert!(out.contains(r#"data: {"delta":"lo"}"#));
        assert!(out.contains("data: [DONE]"));
    }

    #[test]
    fn forward_streaming_writes_502_when_upstream_is_dead() {
        // Bind then drop a listener to get a port nothing is listening on.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut sink = Vec::new();
        forward_streaming(
            &ProxyRequest {
                host: "127.0.0.1",
                port,
                method: "POST",
                path: "/v1/chat/completions",
                content_type: "application/json",
                body: br#"{"stream":true}"#,
                backend_key: None,
            },
            &mut sink,
        )
        .unwrap();

        let out = String::from_utf8(sink).unwrap();
        assert!(out.starts_with("HTTP/1.1 502 Bad Gateway\r\n"));
        assert!(out.contains("upstream_unavailable"));
        assert!(!out.contains("127.0.0.1"));
    }

    #[test]
    fn rejects_incomplete_or_ambiguous_framing() {
        for raw in [
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhi",
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\nhi",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nhi",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: notchunked\r\n\r\nhi",
            "BOGUS 200 OK\r\n\r\nhi",
        ] {
            assert!(parse_backend_response(raw.as_bytes()).is_err(), "{raw:?}");
        }
        for body in [
            "2\r\nhi\r\n",
            "2\r\nh",
            "2\r\nhiXX0\r\n\r\n",
            "nope\r\n",
            "0\r\n",
            "0\r\n\r\nextra",
        ] {
            assert!(dechunk(body.as_bytes()).is_err(), "{body:?}");
        }
        assert_eq!(
            dechunk(b"2;ext=1\r\nhi\r\n0\r\nX-Test: yes\r\n\r\n").unwrap(),
            b"hi"
        );
    }

    #[test]
    fn rejects_oversized_headers() {
        let raw = format!(
            "HTTP/1.1 200 OK\r\nX-Padding: {}\r\n\r\n",
            "x".repeat(MAX_RESPONSE_HEADERS)
        );
        assert!(parse_backend_response(raw.as_bytes()).is_err());
    }

    #[test]
    fn truncated_response_fails_over_a_real_socket() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.read_to_end(&mut Vec::new()).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n{}")
                .unwrap();
        });
        let response = forward(&ProxyRequest {
            host: "127.0.0.1",
            port,
            method: "POST",
            path: "/v1/chat/completions",
            content_type: "application/json",
            body: b"{}",
            backend_key: None,
        });
        assert!(matches!(response, Err(e) if e.kind() == std::io::ErrorKind::InvalidData));
        backend.join().unwrap();
    }
}
