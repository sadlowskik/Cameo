//! A minimal, dependency-free HTTP/1.1 server for the control plane.
//!
//! This is deliberately *not* a general-purpose web server. The only client is
//! Cameo's own dashboard, so it implements exactly what that needs: `GET`/`POST`,
//! `Content-Length` request bodies, and `Connection: close`. No keep-alive, no
//! chunked encoding, no TLS. Keeping it this small is what lets the daemon stay
//! dependency-light and self-contained, matching the rest of the project (the
//! same reason the CLI shells out to `curl` rather than linking an HTTP stack).
//!
//! Anything security-sensitive (who may reach the port, whether a key is
//! required) is the caller's decision — see [`crate::app`]. This layer only
//! moves bytes.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// Requests larger than this are refused with `413`. A control plane never needs
/// a big body, and an unbounded read is a trivial memory-exhaustion vector.
const MAX_BODY: usize = 1024 * 1024;

/// Ceiling on the request line + headers combined. `MAX_BODY` bounds the body,
/// but header lines were read with no limit — a client streaming an endless
/// header (or one gigantic line) grew memory without ever tripping the body cap.
/// 32 KiB is far beyond anything a browser or curl sends.
const MAX_HEAD: u64 = 32 * 1024;

/// How long a single connection may dawdle before we drop it, so a stalled or
/// half-open client cannot pin a worker thread forever.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Ceiling on concurrently served connections. One thread per connection is the
/// right simplicity for a control plane, but without a cap a connection flood
/// converts directly into unbounded threads and memory. Past the cap new
/// connections are dropped immediately (cheaper and safer under overload than
/// composing a 503 for an abuser); a handful of real clients never get near it.
const MAX_CONNECTIONS: usize = 64;

/// A parsed HTTP request. Header keys are lowercased; the path is already split
/// from the query string.
pub struct Request {
    pub method: String,
    pub path: String,
    /// Parsed query string. The current routes take everything in the JSON body,
    /// but the parser fills this (and tests cover it) so a future `?`-carrying
    /// route needs no plumbing change.
    #[allow(dead_code)]
    pub query: HashMap<String, String>,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    /// A request header by (case-insensitive) name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(|s| s.as_str())
    }

    /// Split `path` into non-empty `/`-separated segments.
    pub fn segments(&self) -> Vec<&str> {
        self.path.split('/').filter(|s| !s.is_empty()).collect()
    }
}

/// A closure that takes over an accepted connection and writes the entire HTTP
/// response itself, byte by byte. This is the escape hatch for streaming (the
/// `/v1` gateway's SSE passthrough): the normal `Fn(&Request) -> Response` path
/// builds a whole body first, which is wrong for `stream: true`, so a streaming
/// route hands back a `Response` whose `stream` field pumps the socket directly.
/// `Send` because the response is produced and consumed on the per-connection
/// worker thread.
pub type ResponseStream = Box<dyn FnOnce(&mut dyn Write) -> std::io::Result<()> + Send>;

/// A response to write back. Construct via the helpers rather than by hand so the
/// framing headers stay consistent.
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub extra_headers: Vec<(String, String)>,
    /// When `Some`, the connection is handed to this closure and the `status`/
    /// `content_type`/`body` fields above are ignored — the closure is fully
    /// responsible for framing (used by the streaming gateway).
    pub stream: Option<ResponseStream>,
}

impl Response {
    pub fn new(status: u16, content_type: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type: content_type.to_string(),
            body,
            extra_headers: Vec::new(),
            stream: None,
        }
    }

    /// A streaming response: `f` receives the client socket and writes a complete
    /// HTTP/1.1 response to it (status line, headers, and a progressively-flushed
    /// body). Used by the `/v1` gateway to relay an upstream SSE stream verbatim
    /// instead of buffering it.
    pub fn streaming<F>(f: F) -> Self
    where
        F: FnOnce(&mut dyn Write) -> std::io::Result<()> + Send + 'static,
    {
        Self {
            status: 200,
            content_type: String::new(),
            body: Vec::new(),
            extra_headers: Vec::new(),
            stream: Some(Box::new(f)),
        }
    }

    pub fn json(status: u16, value: &serde_json::Value) -> Self {
        let body = serde_json::to_vec_pretty(value).unwrap_or_else(|_| b"{}".to_vec());
        Self::new(status, "application/json; charset=utf-8", body)
    }

    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self::new(200, "text/html; charset=utf-8", body.into())
    }

    /// A `text/plain` response. Used by the framing tests and available for
    /// plain-text routes; the JSON API does not need it today.
    #[allow(dead_code)]
    pub fn text(status: u16, body: impl Into<String>) -> Self {
        Self::new(
            status,
            "text/plain; charset=utf-8",
            body.into().into_bytes(),
        )
    }

    /// A JSON `{ "error": ... }` body with a status code — the shape the
    /// dashboard expects for every failure.
    pub fn error(status: u16, message: impl Into<String>) -> Self {
        Self::json(
            status,
            &serde_json::json!({ "error": message.into(), "status": status }),
        )
    }

    fn header(mut self, k: &str, v: &str) -> Self {
        self.extra_headers.push((k.to_string(), v.to_string()));
        self
    }
}

/// Serve connections forever, dispatching each through `handler`. One thread per
/// connection: a control plane sees a handful of concurrent clients, so a thread
/// pool would be complexity without payoff.
pub fn serve<F>(listener: TcpListener, handler: F)
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    use std::sync::atomic::{AtomicUsize, Ordering};

    let handler = Arc::new(handler);
    let in_flight = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        // Over the cap, drop the connection on the floor: the accept loop stays
        // fast and no thread is spent on the excess. (The check-then-add is
        // benignly racy — the cap is a shed point, not an exact quota.)
        if in_flight.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            drop(stream);
            continue;
        }
        in_flight.fetch_add(1, Ordering::Relaxed);
        let handler = Arc::clone(&handler);
        let in_flight = Arc::clone(&in_flight);
        std::thread::spawn(move || {
            let _ = handle_connection(stream, handler.as_ref());
            in_flight.fetch_sub(1, Ordering::Relaxed);
        });
    }
}

fn handle_connection<F>(stream: TcpStream, handler: &F) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response,
{
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream);

    let response = match parse_request(&mut reader) {
        Ok(req) => handler(&req),
        Err(ParseError::TooLarge) => Response::error(413, "request body too large"),
        Err(ParseError::Malformed) => Response::error(400, "malformed request"),
        // A closed/empty connection is not worth a reply.
        Err(ParseError::Empty) => return Ok(()),
        Err(ParseError::Io(e)) => return Err(e),
    };

    write_response(reader.get_mut(), response)
}

#[derive(Debug)]
enum ParseError {
    Empty,
    Malformed,
    TooLarge,
    Io(std::io::Error),
}

impl From<std::io::Error> for ParseError {
    fn from(e: std::io::Error) -> Self {
        ParseError::Io(e)
    }
}

/// Parse a request from any buffered reader. Generic over the reader so it can be
/// unit-tested against an in-memory cursor with no socket.
fn parse_request<R: BufRead>(reader: &mut R) -> Result<Request, ParseError> {
    // The request line + headers are read through a hard byte budget
    // (`MAX_HEAD`): without it, a client streaming endless headers — or one
    // endless line — grows memory without ever touching the body cap.
    let mut head = std::io::Read::take(&mut *reader, MAX_HEAD);

    let mut line = String::new();
    if head.read_line(&mut line)? == 0 {
        return Err(ParseError::Empty);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(ParseError::Malformed)?.to_string();
    let target = parts.next().ok_or(ParseError::Malformed)?.to_string();
    // A version token must be present; we don't otherwise care which.
    parts.next().ok_or(ParseError::Malformed)?;

    let (path, query) = split_target(&target);

    let mut headers = HashMap::new();
    let mut head_complete = false;
    loop {
        let mut h = String::new();
        if head.read_line(&mut h)? == 0 {
            break;
        }
        let trimmed = h.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            head_complete = true;
            break;
        }
        if let Some((k, v)) = trimmed.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    if !head_complete {
        // The blank line ending the head never arrived: budget exhausted means
        // an oversized head; a stream that just stopped is malformed.
        return Err(if head.limit() == 0 {
            ParseError::TooLarge
        } else {
            ParseError::Malformed
        });
    }
    // `head`'s borrow of the reader ends here; the body is read from the raw
    // reader under its own `MAX_BODY` check below.
    let body = match headers.get("content-length") {
        Some(len) => {
            let len: usize = len.parse().map_err(|_| ParseError::Malformed)?;
            if len > MAX_BODY {
                return Err(ParseError::TooLarge);
            }
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf)?;
            buf
        }
        None => Vec::new(),
    };

    Ok(Request {
        method,
        path,
        query,
        headers,
        body,
    })
}

/// Split a request target into a decoded path and a decoded query map.
fn split_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let mut map = HashMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k), percent_decode(v));
    }
    (percent_decode(path), map)
}

/// Minimal `application/x-www-form-urlencoded` decode: `%XX` and `+` → space.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_response<W: Write>(w: &mut W, resp: Response) -> std::io::Result<()> {
    // Streaming route: the closure owns framing and body entirely.
    if let Some(stream) = resp.stream {
        return stream(w);
    }
    let reason = reason_phrase(resp.status);
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        resp.status,
        reason,
        resp.content_type,
        resp.body.len()
    );
    for (k, v) in &resp.extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    w.write_all(&resp.body)?;
    w.flush()
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        507 => "Insufficient Storage",
        _ => "OK",
    }
}

#[allow(dead_code)]
impl Response {
    /// Mark a response as never-cache, used for API bodies.
    pub fn no_store(self) -> Self {
        self.header("Cache-Control", "no-store")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &str) -> Result<Request, ParseError> {
        let mut cur = Cursor::new(raw.as_bytes().to_vec());
        parse_request(&mut cur)
    }

    #[test]
    fn parses_get_with_query() {
        let req = parse("GET /api/models?spec=tinyllama&x=1 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/api/models");
        assert_eq!(req.query.get("spec").map(String::as_str), Some("tinyllama"));
        assert_eq!(req.header("host"), Some("x"));
        assert!(req.body.is_empty());
    }

    #[test]
    fn parses_post_body_by_content_length() {
        let body = r#"{"model":"a"}"#;
        let raw = format!(
            "POST /api/servers HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let req = parse(&raw).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.body, body.as_bytes());
    }

    #[test]
    fn segments_ignore_empty() {
        let req = parse("DELETE /api/servers/abc123/ HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(req.segments(), vec!["api", "servers", "abc123"]);
    }

    #[test]
    fn oversized_head_is_rejected() {
        // One endless header line: must trip the head budget, not grow memory.
        let raw = format!(
            "GET / HTTP/1.1\r\nX-Junk: {}\r\n\r\n",
            "a".repeat(MAX_HEAD as usize)
        );
        assert!(matches!(parse(&raw), Err(ParseError::TooLarge)));
    }

    #[test]
    fn truncated_head_is_malformed_not_served() {
        // The stream ends before the blank line that terminates the head.
        assert!(matches!(
            parse("GET / HTTP/1.1\r\nHost: x\r\n"),
            Err(ParseError::Malformed)
        ));
    }

    #[test]
    fn oversized_body_is_rejected() {
        let raw = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(matches!(parse(&raw), Err(ParseError::TooLarge)));
    }

    #[test]
    fn empty_connection_is_distinguished_from_malformed() {
        assert!(matches!(parse(""), Err(ParseError::Empty)));
        assert!(matches!(
            parse("GARBAGE\r\n\r\n"),
            Err(ParseError::Malformed)
        ));
    }

    #[test]
    fn percent_and_plus_decode() {
        assert_eq!(percent_decode("a%2Fb+c"), "a/b c");
        assert_eq!(percent_decode("plain"), "plain");
        // A stray percent is left as-is rather than dropped.
        assert_eq!(percent_decode("100%done"), "100%done");
    }

    #[test]
    fn response_framing_has_length_and_close() {
        let mut out = Vec::new();
        write_response(&mut out, Response::text(404, "nope")).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(s.contains("Content-Length: 4\r\n"));
        assert!(s.contains("Connection: close\r\n"));
        assert!(s.ends_with("\r\n\r\nnope"));
    }

    #[test]
    fn streaming_response_owns_the_socket() {
        // A streaming Response bypasses normal framing entirely: whatever the
        // closure writes is exactly what lands on the wire.
        let mut out = Vec::new();
        let resp = Response::streaming(|w| {
            w.write_all(b"HTTP/1.1 200 OK\r\n\r\ndata: one\n\n")?;
            w.write_all(b"data: two\n\n")?;
            Ok(())
        });
        write_response(&mut out, resp).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert_eq!(s, "HTTP/1.1 200 OK\r\n\r\ndata: one\n\ndata: two\n\n");
        // No Content-Length was injected — the closure is fully in charge.
        assert!(!s.contains("Content-Length"));
    }
}
