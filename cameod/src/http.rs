//! A minimal, dependency-free HTTP/1.1 server for the control plane.
//!
//! This is deliberately *not* a general-purpose web server. The only client is
//! Cameo's own dashboard, so it implements exactly what that needs: `GET`/`POST`,
//! `Content-Length` request bodies, and `Connection: close`. No keep-alive, no
//! chunked encoding. TLS, when configured, wraps the accepted socket (see
//! [`crate::tls`]) and this layer never knows. Keeping it this small is what lets the daemon stay
//! dependency-light and self-contained, matching the rest of the project (the
//! same reason the CLI shells out to `curl` rather than linking an HTTP stack).
//!
//! Anything security-sensitive (who may reach the port, whether a key is
//! required) is the caller's decision — see [`crate::app`]. This layer only
//! moves bytes.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::IpAddr;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Requests larger than this are refused with `413`. A control plane never needs
/// a big body, and an unbounded read is a trivial memory-exhaustion vector.
/// Published through the engine contract so a harness can reject an oversized
/// tool result before it reaches the wire.
pub const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

/// Ceiling on the request line + headers combined. `MAX_REQUEST_BODY_BYTES` bounds the body,
/// but header lines were read with no limit — a client streaming an endless
/// header (or one gigantic line) grew memory without ever tripping the body cap.
/// 32 KiB is far beyond anything a browser or curl sends.
const MAX_HEAD: u64 = 32 * 1024;

/// How long a single write may dawdle before we drop the connection, so a
/// stalled or half-open client cannot pin a worker thread forever.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-read stall limit while receiving a request.
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Absolute budget for receiving one request (head and body). Per-read
/// timeouts alone let a client trickle one byte every few seconds and hold a
/// worker forever without ever tripping them; this bounds the whole receive.
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

/// Ceiling on concurrently served connections. One thread per connection is the
/// right simplicity for a control plane, but without a cap a connection flood
/// converts directly into unbounded threads and memory. Past the cap new
/// connections are dropped immediately (cheaper and safer under overload than
/// composing a 503 for an abuser); a handful of real clients never get near it.
const MAX_CONNECTIONS: usize = 64;

/// Ceiling per remote address, so one LAN host cannot consume the global cap
/// by itself. Loopback is exempt: a reverse proxy or the local updater funnels
/// every client through 127.0.0.1 and must not be capped as one peer.
const MAX_CONNECTIONS_PER_PEER: usize = 16;

/// Connection admission: a global ceiling plus a per-peer ceiling. Both counts
/// are released in [`ConnectionSlot::drop`] so a panicking handler cannot
/// permanently leak capacity and silently DoS the listener.
#[derive(Default)]
struct Admission {
    total: AtomicUsize,
    per_peer: std::sync::Mutex<HashMap<IpAddr, usize>>,
}

impl Admission {
    fn admit(self: &Arc<Self>, peer: Option<IpAddr>) -> Option<ConnectionSlot> {
        self.total
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current < MAX_CONNECTIONS).then_some(current + 1)
            })
            .ok()?;
        let counted = match peer {
            Some(ip) if !ip.is_loopback() => {
                let mut map = self
                    .per_peer
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let count = map.entry(ip).or_insert(0);
                if *count >= MAX_CONNECTIONS_PER_PEER {
                    drop(map);
                    self.total.fetch_sub(1, Ordering::Relaxed);
                    return None;
                }
                *count += 1;
                Some(ip)
            }
            _ => None,
        };
        Some(ConnectionSlot {
            admission: Arc::clone(self),
            peer: counted,
        })
    }
}

/// Owns one admitted connection slot.
struct ConnectionSlot {
    admission: Arc<Admission>,
    peer: Option<IpAddr>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.admission.total.fetch_sub(1, Ordering::Relaxed);
        if let Some(ip) = self.peer {
            let mut map = self
                .admission
                .per_peer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(count) = map.get_mut(&ip) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    map.remove(&ip);
                }
            }
        }
    }
}

/// End-of-response hook: a TLS stream sends `close_notify` so the client can
/// tell a complete response from a truncated one; plain TCP needs nothing.
trait Finish {
    fn finish(&mut self) {}
}

impl Finish for TcpStream {}

impl Finish for rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    fn finish(&mut self) {
        self.conn.send_close_notify();
        let _ = self.flush();
    }
}

/// Enforces [`REQUEST_DEADLINE`] on reads; writes pass straight through so a
/// long streaming response is not cut off by the receive budget.
struct DeadlineStream<S> {
    inner: S,
    deadline: std::time::Instant,
}

impl<S: std::io::Read> std::io::Read for DeadlineStream<S> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if std::time::Instant::now() >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "request deadline exceeded",
            ));
        }
        self.inner.read(buf)
    }
}

impl<S: Write> Write for DeadlineStream<S> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

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
    /// True when this request arrived on the host-only operator socket
    /// (`/run/cameo/cameo.sock`). Self-host posture treats that as operator.
    pub from_unix: bool,
    /// Remote address for per-client admission. Unix-socket requests have no IP.
    pub peer_ip: Option<IpAddr>,
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
    pub completion: Option<crate::drain::Permit>,
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
            completion: None,
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
            completion: None,
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

    pub fn with_header(mut self, k: &str, v: &str) -> Self {
        self.extra_headers.push((k.to_string(), v.to_string()));
        self
    }
}

/// Serve connections forever, dispatching each through `handler`. One thread per
/// connection: a control plane sees a handful of concurrent clients, so a thread
/// pool would be complexity without payoff.
pub fn serve<F>(
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    handler: F,
    mut stop: impl FnMut() -> bool,
) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let admission = Arc::new(Admission::default());
    listener.set_nonblocking(true)?;
    while !stop() {
        let (stream, peer) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(error) => return Err(error),
        };
        stream.set_nonblocking(false)?;
        // Over the cap, drop the connection on the floor: the accept loop stays
        // fast and no thread is spent on the excess. Admission is atomic, so
        // even a connection burst cannot overshoot the thread ceiling.
        let Some(slot) = admission.admit(Some(peer.ip())) else {
            drop(stream);
            continue;
        };
        let handler = Arc::clone(&handler);
        let tls = tls.clone();
        std::thread::spawn(move || {
            let _slot = slot;
            let _ = handle_connection(stream, tls.as_ref(), handler.as_ref());
        });
    }
    Ok(())
}

fn handle_connection<F>(
    stream: TcpStream,
    tls: Option<&Arc<rustls::ServerConfig>>,
    handler: &F,
) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response,
{
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let peer_ip = stream.peer_addr().ok().map(|address| address.ip());
    // A second handle on the same socket for drain registration and the
    // write-timeout tweak, whether or not TLS wraps the primary one.
    let control = stream.try_clone()?;
    match tls {
        Some(config) => {
            let connection = rustls::ServerConnection::new(Arc::clone(config))
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            handle_stream(
                rustls::StreamOwned::new(connection, stream),
                control,
                peer_ip,
                handler,
            )
        }
        None => handle_stream(stream, control, peer_ip, handler),
    }
}

fn handle_stream<S, F>(
    stream: S,
    control: TcpStream,
    peer_ip: Option<IpAddr>,
    handler: &F,
) -> std::io::Result<()>
where
    S: std::io::Read + Write + Finish,
    F: Fn(&Request) -> Response,
{
    let mut reader = BufReader::new(DeadlineStream {
        inner: stream,
        deadline: std::time::Instant::now() + REQUEST_DEADLINE,
    });

    let response = match parse_request(&mut reader) {
        Ok(mut req) => {
            req.peer_ip = peer_ip;
            handler(&req)
        }
        Err(ParseError::TooLarge) => Response::error(413, "request body too large"),
        Err(ParseError::Malformed) => Response::error(400, "malformed request"),
        // A closed/empty connection is not worth a reply.
        Err(ParseError::Empty) => return Ok(()),
        Err(ParseError::Io(e)) => return Err(e),
    };

    if response.completion.is_some() {
        control.set_write_timeout(Some(Duration::from_millis(250)))?;
    }
    let _registration = response
        .completion
        .as_ref()
        .map(|permit| permit.register_client(&control))
        .transpose()?;
    let written = write_response(reader.get_mut(), response);
    reader.get_mut().inner.finish();
    written
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
    if !line.ends_with("\r\n") {
        return Err(if head.limit() == 0 {
            ParseError::TooLarge
        } else {
            ParseError::Malformed
        });
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().ok_or(ParseError::Malformed)?.to_string();
    let target = parts.next().ok_or(ParseError::Malformed)?.to_string();
    let version = parts.next().ok_or(ParseError::Malformed)?;
    if parts.next().is_some()
        || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
        || !matches!(method.as_str(), "GET" | "POST" | "DELETE")
        || !target.starts_with('/')
        || target.contains('#')
    {
        return Err(ParseError::Malformed);
    }

    let (path, query) = split_target(&target)?;

    let mut headers = HashMap::new();
    let mut head_complete = false;
    loop {
        let mut h = String::new();
        if head.read_line(&mut h)? == 0 {
            break;
        }
        if !h.ends_with("\r\n") {
            return Err(if head.limit() == 0 {
                ParseError::TooLarge
            } else {
                ParseError::Malformed
            });
        }
        let trimmed = h.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            head_complete = true;
            break;
        }
        let (k, v) = trimmed.split_once(':').ok_or(ParseError::Malformed)?;
        let key = k.trim().to_ascii_lowercase();
        let value = v.trim();
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || value.bytes().any(|b| b < 0x20 && b != b'\t')
            || headers.insert(key, value.to_string()).is_some()
        {
            return Err(ParseError::Malformed);
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
    if version == "HTTP/1.1" && !headers.contains_key("host") {
        return Err(ParseError::Malformed);
    }
    if headers.contains_key("transfer-encoding") {
        // This server only implements Content-Length. Accepting TE while ignoring it
        // creates a different message boundary than a reverse proxy may see.
        return Err(ParseError::Malformed);
    }
    // `head`'s borrow of the reader ends here; the body is read from the raw
    // reader under its own `MAX_REQUEST_BODY_BYTES` check below.
    let body = match headers.get("content-length") {
        Some(len) => {
            let len: usize = len.parse().map_err(|_| ParseError::Malformed)?;
            if len > MAX_REQUEST_BODY_BYTES {
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
        from_unix: false,
        peer_ip: None,
    })
}

/// Split a request target into a decoded path and a decoded query map.
fn split_target(target: &str) -> Result<(String, HashMap<String, String>), ParseError> {
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let mut map = HashMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(percent_decode(k, true)?, percent_decode(v, true)?);
    }
    let path = percent_decode(path, false)?;
    if path
        .bytes()
        .any(|byte| byte == 0 || byte < 0x20 || byte == 0x7f)
    {
        return Err(ParseError::Malformed);
    }
    Ok((path, map))
}

/// Minimal `application/x-www-form-urlencoded` decode: `%XX` and `+` → space.
fn percent_decode(s: &str, plus_as_space: bool) -> Result<String, ParseError> {
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
                return Err(ParseError::Malformed);
            }
            b'%' => return Err(ParseError::Malformed),
            b'+' if plus_as_space => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    if out
        .iter()
        .any(|byte| *byte == 0 || *byte == b'\r' || *byte == b'\n' || *byte == 0x7f)
    {
        return Err(ParseError::Malformed);
    }
    String::from_utf8(out).map_err(|_| ParseError::Malformed)
}

fn write_response<W: Write>(w: &mut W, resp: Response) -> std::io::Result<()> {
    let _completion = resp.completion;
    let mut w = ControlledWriter {
        inner: w,
        permit: _completion.as_ref(),
        progress: std::time::Instant::now(),
    };
    // Streaming route: the closure owns framing and body entirely.
    if let Some(stream) = resp.stream {
        return stream(&mut w);
    }
    let reason = reason_phrase(resp.status);
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n",
        resp.status,
        reason,
        safe_header_value(&resp.content_type).unwrap_or("application/octet-stream"),
        resp.body.len()
    );
    for (k, v) in &resp.extra_headers {
        if let (Some(k), Some(v)) = (safe_header_name(k), safe_header_value(v)) {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
    }
    head.push_str("\r\n");
    w.write_all(head.as_bytes())?;
    w.write_all(&resp.body)?;
    w.flush()
}

struct ControlledWriter<'a, W> {
    inner: &'a mut W,
    permit: Option<&'a crate::drain::Permit>,
    progress: std::time::Instant,
}
impl<W: Write> Write for ControlledWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            if self.permit.is_some_and(|permit| permit.cancelled()) {
                // Interrupted is retried by write_all, so cancellation must use a terminal kind.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "drain deadline exceeded",
                ));
            }
            match self.inner.write(bytes) {
                Ok(n) => {
                    self.progress = std::time::Instant::now();
                    return Ok(n);
                }
                Err(error)
                    if self.permit.is_some()
                        && matches!(
                            error.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        )
                        && self.progress.elapsed() < IO_TIMEOUT =>
                {
                    continue
                }
                Err(error) => return Err(error),
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn safe_header_name(value: &str) -> Option<&str> {
    (!value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)))
    .then_some(value)
}

fn safe_header_value(value: &str) -> Option<&str> {
    (!value
        .bytes()
        .any(|b| b == b'\r' || b == b'\n' || (b < 0x20 && b != b'\t')))
    .then_some(value)
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
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        507 => "Insufficient Storage",
        _ => "OK",
    }
}

#[allow(dead_code)]
impl Response {
    /// Mark a response as never-cache, used for API bodies.
    pub fn no_store(self) -> Self {
        self.with_header("Cache-Control", "no-store")
    }
}

/// Host-only operator listener. Same HTTP app as TCP; [`Request::from_unix`] is
/// set so `self-host` posture grants operator without a bearer key.
/// LAN / TCP is unchanged.
#[cfg(unix)]
pub fn serve_unix<F>(listener: std::os::unix::net::UnixListener, handler: F)
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let admission = Arc::new(Admission::default());
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Some(slot) = admission.admit(None) else {
            drop(stream);
            continue;
        };
        let handler = Arc::clone(&handler);
        std::thread::spawn(move || {
            let _slot = slot;
            let _ = handle_unix(stream, handler.as_ref());
        });
    }
}

#[cfg(unix)]
fn handle_unix<F>(stream: std::os::unix::net::UnixStream, handler: &F) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response,
{
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    let mut reader = BufReader::new(stream);
    let response = match parse_request(&mut reader) {
        Ok(mut req) => {
            req.from_unix = true;
            handler(&req)
        }
        Err(ParseError::TooLarge) => Response::error(413, "request body too large"),
        Err(ParseError::Malformed) => Response::error(400, "malformed request"),
        Err(ParseError::Empty) => return Ok(()),
        Err(ParseError::Io(e)) => return Err(e),
    };
    if response.completion.is_some() {
        reader
            .get_ref()
            .set_write_timeout(Some(Duration::from_millis(250)))?;
    }
    let _registration = response
        .completion
        .as_ref()
        .map(|permit| permit.register_unix_client(reader.get_ref()))
        .transpose()?;
    write_response(reader.get_mut(), response)
}

#[cfg(test)]
mod tests {
    #[test]
    fn connection_slot_releases_capacity_during_unwind() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        use std::sync::atomic::Ordering;

        let admission = std::sync::Arc::new(super::Admission::default());
        let peer: std::net::IpAddr = "10.0.0.7".parse().unwrap();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let _slot = admission.admit(Some(peer)).unwrap();
            assert_eq!(admission.total.load(Ordering::Relaxed), 1);
            panic!("handler fixture");
        }));
        assert!(result.is_err());
        assert_eq!(admission.total.load(Ordering::Relaxed), 0);
        assert!(admission.per_peer.lock().unwrap().is_empty());
    }

    /// Accepts any server certificate: the test talks to the daemon's own
    /// self-signed leaf and only checks that HTTP flows through TLS.
    #[derive(Debug)]
    struct TrustAnything;

    impl rustls::client::danger::ServerCertVerifier for TrustAnything {
        fn verify_server_cert(
            &self,
            _end_entity: &rustls::pki_types::CertificateDer<'_>,
            _intermediates: &[rustls::pki_types::CertificateDer<'_>],
            _server_name: &rustls::pki_types::ServerName<'_>,
            _ocsp_response: &[u8],
            _now: rustls::pki_types::UnixTime,
        ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn verify_tls13_signature(
            &self,
            _message: &[u8],
            _cert: &rustls::pki_types::CertificateDer<'_>,
            _dss: &rustls::DigitallySignedStruct,
        ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes()
        }
    }

    #[test]
    fn serves_https_with_a_minted_certificate_and_refuses_plain_http() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!("cameo-https-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let configured = crate::tls::configure(Some(&dir), "127.0.0.1")
            .unwrap()
            .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            super::serve(
                listener,
                Some(configured.config),
                |req| super::Response::text(200, format!("secure {}", req.path)),
                move || stop_flag.load(Ordering::SeqCst),
            )
        });

        // A TLS client gets a normal HTTP response.
        let client_config = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(TrustAnything))
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let connection = rustls::ClientConnection::new(Arc::new(client_config), name).unwrap();
        let socket = std::net::TcpStream::connect(address).unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut tls = rustls::StreamOwned::new(connection, socket);
        tls.write_all(b"GET /ping HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        let _ = tls.read_to_string(&mut reply);
        assert!(reply.starts_with("HTTP/1.1 200"), "got: {reply}");
        assert!(reply.ends_with("secure /ping"), "got: {reply}");

        // Plain HTTP on the TLS port is not a request the server will answer.
        let mut plain = std::net::TcpStream::connect(address).unwrap();
        plain
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        plain
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut junk = Vec::new();
        let _ = plain.read_to_end(&mut junk);
        assert!(
            !junk.starts_with(b"HTTP/1.1 200"),
            "plain HTTP was served on the TLS port"
        );

        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_peer_cannot_take_the_whole_listener() {
        let admission = std::sync::Arc::new(super::Admission::default());
        let peer: std::net::IpAddr = "10.0.0.8".parse().unwrap();
        let held: Vec<_> = (0..super::MAX_CONNECTIONS_PER_PEER)
            .map(|_| admission.admit(Some(peer)).unwrap())
            .collect();
        assert!(admission.admit(Some(peer)).is_none(), "per-peer cap");
        let other: std::net::IpAddr = "10.0.0.9".parse().unwrap();
        assert!(
            admission.admit(Some(other)).is_some(),
            "others still admitted"
        );
        let loopback: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert!(admission.admit(Some(loopback)).is_some(), "loopback exempt");
        drop(held);
        assert!(admission.admit(Some(peer)).is_some(), "released on drop");
    }

    #[test]
    fn serve_can_stop_without_waiting_for_an_incoming_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        super::serve(
            listener,
            None,
            |_| super::Response::text(200, "unused"),
            || true,
        )
        .unwrap();
    }

    #[test]
    fn stoppable_listener_serves_blocking_client_io() {
        use std::io::{Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let observed = stop.clone();
        let server = std::thread::spawn(move || {
            super::serve(
                listener,
                None,
                |_| super::Response::text(200, "shutdown-fixture"),
                || observed.load(Ordering::SeqCst),
            )
            .unwrap();
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        assert!(response.ends_with("shutdown-fixture"));
    }
    #[test]
    fn drain_releases_response_when_downstream_stops_reading() {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let drain = crate::drain::Drain::default();
        let server_drain = drain.clone();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let result = super::handle_connection(stream, None, &|_| {
                let started = started_tx.clone();
                let mut response = super::Response::streaming(move |sink| {
                    sink.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n")?;
                    started.send(()).unwrap();
                    let block = [b'x'; 64 * 1024];
                    for _ in 0..1024 {
                        sink.write_all(&block)?;
                    }
                    Ok(())
                });
                response.completion = server_drain.admit();
                response
            });
            done_tx.send(result.is_err()).unwrap();
        });
        let mut client = std::net::TcpStream::connect(address).unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .unwrap();
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        drain.begin(std::time::Duration::ZERO);
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap());
        assert_eq!(drain.status()["state"], "drained");
        drop(client);
        server.join().unwrap();
    }
    #[test]
    fn drain_permit_lives_until_stream_finishes_and_releases_on_error() {
        let drain = crate::drain::Drain::default();
        let permit = drain.admit().unwrap();
        let observed = drain.clone();
        let mut response = super::Response::streaming(move |_| {
            assert_eq!(observed.status()["active_requests"], 1);
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "client disconnected",
            ))
        });
        response.completion = Some(permit);
        drain.begin(std::time::Duration::from_secs(60));
        assert!(super::write_response(&mut Vec::new(), response).is_err());
        assert_eq!(drain.status()["state"], "drained");
    }

    #[test]
    fn dropped_response_releases_unstarted_work() {
        let drain = crate::drain::Drain::default();
        let mut response = super::Response::json(200, &serde_json::json!({}));
        response.completion = drain.admit();
        drain.begin(std::time::Duration::from_secs(60));
        drop(response);
        assert_eq!(drain.status()["active_requests"], 0);
    }
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
            "POST /api/servers HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let req = parse(&raw).unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.body, body.as_bytes());
    }

    #[test]
    fn segments_ignore_empty() {
        let req = parse("DELETE /api/servers/abc123/ HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
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
            "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n",
            MAX_REQUEST_BODY_BYTES + 1
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
        assert_eq!(percent_decode("a%2Fb+c", true).unwrap(), "a/b c");
        assert_eq!(percent_decode("a%2Fb+c", false).unwrap(), "a/b+c");
        assert_eq!(percent_decode("plain", true).unwrap(), "plain");
        assert!(matches!(
            percent_decode("100%done", true),
            Err(ParseError::Malformed)
        ));
    }

    #[test]
    fn rejects_ambiguous_or_malformed_framing() {
        for raw in [
            "POST /api HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nContent-Length: 1\r\n\r\nX",
            "POST /api HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            "GET / HTTP/1.1\r\nHost x\r\n\r\n",
            "GET / HTTP/2\r\nHost: x\r\n\r\n",
            "GET http://example.com/ HTTP/1.1\r\nHost: x\r\n\r\n",
            "GET /bad%zz HTTP/1.1\r\nHost: x\r\n\r\n",
            "GET /?key=bad%0dvalue HTTP/1.1\r\nHost: x\r\n\r\n",
            "GET / HTTP/1.1\r\n\r\n",
        ] {
            assert!(
                matches!(parse(raw), Err(ParseError::Malformed)),
                "accepted {raw:?}"
            );
        }
    }

    #[test]
    fn response_headers_cannot_inject_lines() {
        assert_eq!(
            safe_header_value("application/json"),
            Some("application/json")
        );
        assert_eq!(safe_header_value("ok\r\nX-Evil: yes"), None);
        assert_eq!(safe_header_name("X-Test"), Some("X-Test"));
        assert_eq!(safe_header_name("X-Test\r\n"), None);
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
