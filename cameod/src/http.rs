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
use std::io::{BufRead, BufReader, Read, Write};
use std::net::IpAddr;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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

/// How long the upstream connect to `knossos field` may take. Loopback, so a
/// refused or hung port shows up in milliseconds; 5 s only covers a Field that
/// is still starting.
const FIELD_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The `/field/` reverse proxy (`FIELD-REMOTE-001`): requests under this prefix
/// are spliced byte-for-byte to `knossos field` on loopback, so Field shares
/// the console's TLS certificate, key and origin without re-implementing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldProxy {
    /// Where `knossos field` listens. Loopback by construction (config refuses
    /// anything else); the `Host` header forwarded upstream is this authority.
    pub upstream: SocketAddr,
}

/// The URL prefix the proxy owns. `/field` redirects to `/field/`; everything
/// under `/field/` is forwarded with the prefix stripped.
const FIELD_PREFIX: &str = "/field";

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

/// Take the accepted connection over entirely and copy bytes in both directions
/// between it and `upstream` until either side closes. This is the transport
/// under the `/field/` proxy: one code path serves plain requests, SSE and
/// WebSocket upgrades because nothing here knows what the bytes mean.
///
/// `control` is a second handle on the same client socket (see
/// [`handle_connection`]); the plain-TCP splice writes through it while the
/// primary handle reads, and the TLS splice does the same at the socket level
/// under one shared `ServerConnection`.
trait Splice {
    fn splice(self, control: TcpStream, upstream: TcpStream) -> std::io::Result<()>;
}

impl Splice for TcpStream {
    fn splice(self, control: TcpStream, upstream: TcpStream) -> std::io::Result<()> {
        let done = Arc::new(AtomicBool::new(false));
        let mut client_reader = self;
        let mut upstream_writer = upstream.try_clone()?;
        let flag = Arc::clone(&done);
        let to_field = std::thread::spawn(move || {
            let result = pump(&mut client_reader, &mut upstream_writer, &flag);
            flag.store(true, Ordering::SeqCst);
            let _ = upstream_writer.shutdown(Shutdown::Write);
            result
        });
        let mut client_writer = control;
        let mut upstream_reader = upstream;
        let result = pump(&mut upstream_reader, &mut client_writer, &done);
        done.store(true, Ordering::SeqCst);
        // Both directions: FIN to the client, and unblock the reader thread's
        // pending `read` so the worker does not linger for a full timeout.
        let _ = client_writer.shutdown(Shutdown::Both);
        let _ = to_field.join();
        result
    }
}

impl Splice for rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    fn splice(self, control: TcpStream, upstream: TcpStream) -> std::io::Result<()> {
        // `StreamOwned` is not splittable, so the TLS state is shared under a
        // mutex and every socket read happens *outside* it: the lock is only
        // held to decrypt bytes already read or to encrypt bytes about to be
        // written, never across a blocking socket operation on the read side.
        let rustls::StreamOwned { mut conn, sock } = self;
        // The request parser's buffer was forwarded by the caller, but rustls
        // may hold further plaintext it decrypted beyond what that buffer took.
        let mut upstream_writer = upstream.try_clone()?;
        let mut pending = Vec::new();
        drain_plaintext(&mut conn, &mut pending);
        if !pending.is_empty() {
            upstream_writer.write_all(&pending)?;
            upstream_writer.flush()?;
        }
        let conn = Arc::new(Mutex::new(conn));
        let done = Arc::new(AtomicBool::new(false));

        let mut tls_reader = sock;
        let mut tls_alert_writer = control.try_clone()?;
        let flag = Arc::clone(&done);
        let shared = Arc::clone(&conn);
        let to_field = std::thread::spawn(move || {
            let mut raw = [0u8; 16 * 1024];
            let mut plain = Vec::new();
            let result = loop {
                if flag.load(Ordering::SeqCst) {
                    break Ok(());
                }
                let n = match tls_reader.read(&mut raw) {
                    Ok(0) => break Ok(()),
                    Ok(n) => n,
                    Err(error) if is_retryable(&error) => continue,
                    Err(error) => break Err(error),
                };
                plain.clear();
                let closed = {
                    let mut conn = lock(&shared);
                    let mut slice = &raw[..n];
                    let mut closed = false;
                    while !slice.is_empty() {
                        match conn.read_tls(&mut slice) {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(_) => {
                                closed = true;
                                break;
                            }
                        }
                        if conn.process_new_packets().is_err() {
                            closed = true;
                            break;
                        }
                        if drain_plaintext(&mut conn, &mut plain) {
                            closed = true;
                            break;
                        }
                    }
                    // Handshake follow-ups and alerts the peer may have
                    // provoked go out while the state is still consistent.
                    let _ = flush_tls(&mut conn, &mut tls_alert_writer);
                    closed
                };
                if !plain.is_empty() {
                    if let Err(error) = upstream_writer
                        .write_all(&plain)
                        .and_then(|()| upstream_writer.flush())
                    {
                        break Err(error);
                    }
                }
                if closed {
                    break Ok(());
                }
            };
            flag.store(true, Ordering::SeqCst);
            let _ = upstream_writer.shutdown(Shutdown::Write);
            result
        });

        let mut tls_writer = control;
        let mut upstream_reader = upstream;
        let mut buf = [0u8; 16 * 1024];
        let result = loop {
            if done.load(Ordering::SeqCst) {
                break Ok(());
            }
            match upstream_reader.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => {
                    let mut conn = lock(&conn);
                    if let Err(error) = conn
                        .writer()
                        .write_all(&buf[..n])
                        .and_then(|()| flush_tls(&mut conn, &mut tls_writer))
                    {
                        break Err(error);
                    }
                }
                Err(error) if is_retryable(&error) => continue,
                Err(error) => break Err(error),
            }
        };
        {
            let mut conn = lock(&conn);
            conn.send_close_notify();
            let _ = flush_tls(&mut conn, &mut tls_writer);
        }
        done.store(true, Ordering::SeqCst);
        let _ = tls_writer.shutdown(Shutdown::Both);
        let _ = to_field.join();
        result
    }
}

fn lock(
    conn: &Mutex<rustls::ServerConnection>,
) -> std::sync::MutexGuard<'_, rustls::ServerConnection> {
    conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Encrypted bytes rustls has queued go to the socket now.
fn flush_tls(conn: &mut rustls::ServerConnection, sock: &mut TcpStream) -> std::io::Result<()> {
    while conn.wants_write() {
        conn.write_tls(sock)?;
    }
    sock.flush()
}

/// Move every decrypted byte rustls holds into `out`. Returns `true` once the
/// peer has closed the TLS session (cleanly or not) so the caller stops.
fn drain_plaintext(conn: &mut rustls::ServerConnection, out: &mut Vec<u8>) -> bool {
    let mut buf = [0u8; 16 * 1024];
    loop {
        match conn.reader().read(&mut buf) {
            Ok(0) => return true,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return false,
            Err(_) => return true,
        }
    }
}

/// A read that hit the per-read timeout (or a signal) is not the end of the
/// connection: an idle WebSocket stays spliced until a side actually closes.
fn is_retryable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::Interrupted
    )
}

/// Copy `from` to `to` until EOF, an error, or `done` (set by the opposite
/// direction) is observed at the next read timeout.
fn pump(from: &mut TcpStream, to: &mut TcpStream, done: &AtomicBool) -> std::io::Result<()> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        if done.load(Ordering::SeqCst) {
            return Ok(());
        }
        match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                to.write_all(&buf[..n])?;
                to.flush()?;
            }
            Err(error) if is_retryable(&error) => continue,
            Err(error) => return Err(error),
        }
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

    /// A `307 Temporary Redirect` to `location` with an empty body.
    pub fn redirect(location: &str) -> Self {
        Self::new(307, "text/plain; charset=utf-8", Vec::new()).with_header("Location", location)
    }
}

/// Serve connections forever, dispatching each through `handler`. One thread per
/// connection: a control plane sees a handful of concurrent clients, so a thread
/// pool would be complexity without payoff.
/// The daemon itself goes through [`serve_with`]; this is the proxy-less form
/// the tests and any future second listener use.
#[allow(dead_code)]
pub fn serve<F>(
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    handler: F,
    stop: impl FnMut() -> bool,
) -> std::io::Result<()>
where
    F: Fn(&Request) -> Response + Send + Sync + 'static,
{
    serve_with(listener, tls, None, handler, stop)
}

/// [`serve`] with the `/field/` proxy: when `field` is set, requests under
/// `/field/` never reach `handler` — they are spliced to Field on loopback
/// (see [`FieldProxy`]). Admission and everything else is unchanged.
pub fn serve_with<F>(
    listener: TcpListener,
    tls: Option<Arc<rustls::ServerConfig>>,
    field: Option<FieldProxy>,
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
            let _ = handle_connection(stream, tls.as_ref(), field, handler.as_ref());
        });
    }
    Ok(())
}

fn handle_connection<F>(
    stream: TcpStream,
    tls: Option<&Arc<rustls::ServerConfig>>,
    field: Option<FieldProxy>,
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
    let connection = Connection {
        control,
        peer_ip,
        secure: tls.is_some(),
        field,
    };
    match tls {
        Some(config) => {
            let tls_connection = rustls::ServerConnection::new(Arc::clone(config))
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            handle_stream(
                rustls::StreamOwned::new(tls_connection, stream),
                connection,
                handler,
            )
        }
        None => handle_stream(stream, connection, handler),
    }
}

/// Per-connection facts the stream handler needs beside the stream itself.
struct Connection {
    /// Second handle on the client socket (drain registration, splice writes).
    control: TcpStream,
    peer_ip: Option<IpAddr>,
    /// Whether the client speaks TLS to us — what `X-Forwarded-Proto` reports.
    secure: bool,
    field: Option<FieldProxy>,
}

fn handle_stream<S, F>(stream: S, connection: Connection, handler: &F) -> std::io::Result<()>
where
    S: std::io::Read + Write + Finish + Splice,
    F: Fn(&Request) -> Response,
{
    let Connection {
        control,
        peer_ip,
        secure,
        field,
    } = connection;
    let mut reader = BufReader::new(DeadlineStream {
        inner: stream,
        deadline: std::time::Instant::now() + REQUEST_DEADLINE,
    });

    let response = match parse_incoming(&mut reader, field.is_some()) {
        Ok(Parsed::Request(mut req)) => {
            req.peer_ip = peer_ip;
            handler(&req)
        }
        Ok(Parsed::FieldRedirect(location)) => Response::redirect(&location),
        Ok(Parsed::Field { head, target }) => {
            let field = field.expect("field routes are only parsed when the proxy is on");
            let upstream = match TcpStream::connect_timeout(&field.upstream, FIELD_CONNECT_TIMEOUT)
            {
                Ok(upstream) => upstream,
                Err(error) => {
                    tracing::warn!("field proxy: connecting {}: {error}", field.upstream);
                    let response = Response::error(
                        502,
                        format!("field is not reachable at {}", field.upstream),
                    );
                    let written = write_response(reader.get_mut(), response);
                    reader.get_mut().inner.finish();
                    return written;
                }
            };
            upstream.set_read_timeout(Some(READ_TIMEOUT))?;
            upstream.set_write_timeout(Some(IO_TIMEOUT))?;
            upstream.set_nodelay(true)?;
            return splice_to_field(
                reader, control, upstream, &head, &target, field, secure, peer_ip,
            );
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

/// Hand a `/field/` request to Field: the rewritten head first, then whatever
/// the parser had already pulled off the socket past the head (a body, or
/// pipelined bytes), then a raw two-way copy until either side closes.
#[allow(clippy::too_many_arguments)]
fn splice_to_field<S>(
    reader: BufReader<DeadlineStream<S>>,
    control: TcpStream,
    mut upstream: TcpStream,
    head: &Head,
    target: &str,
    field: FieldProxy,
    secure: bool,
    peer_ip: Option<IpAddr>,
) -> std::io::Result<()>
where
    S: std::io::Read + Write + Finish + Splice,
{
    let mut first = field_request_head(head, target, field.upstream, secure, peer_ip).into_bytes();
    first.extend_from_slice(reader.buffer());
    upstream.write_all(&first)?;
    upstream.flush()?;
    // The deadline wrapper only guards the receive of one request; a splice
    // lives as long as the two peers want it to.
    let stream = reader.into_inner().inner;
    stream.splice(control, upstream)
}

/// Where a request target falls relative to the `/field` prefix.
#[derive(Debug, PartialEq, Eq)]
enum FieldRoute {
    /// `/field` (optionally with a query): send the browser to `/field/` so the
    /// SPA's relative asset URLs resolve under the prefix.
    Redirect(String),
    /// `/field/...`: forward with the prefix stripped (`/field/` → `/`).
    Forward(String),
}

/// Classify a raw request target (path plus query, undecoded) against the
/// `/field` prefix. Anything else — including `/fieldx` — is `None`.
fn field_route(target: &str) -> Option<FieldRoute> {
    let rest = target.strip_prefix(FIELD_PREFIX)?;
    match rest.as_bytes().first() {
        None => Some(FieldRoute::Redirect(format!("{FIELD_PREFIX}/"))),
        Some(b'?') => Some(FieldRoute::Redirect(format!("{FIELD_PREFIX}/{rest}"))),
        Some(b'/') => Some(FieldRoute::Forward(rest.to_string())),
        Some(_) => None,
    }
}

/// Re-serialize a parsed head for Field. Every client header is forwarded;
/// `Host` becomes the loopback authority Field's security module expects,
/// with the original in `X-Forwarded-Host`/`X-Forwarded-Proto`/`X-Forwarded-For`
/// (ours, never the client's). Unless the request is an upgrade, `Connection:
/// close` makes Field end the connection after one response, which is what
/// keeps a keep-alive browser from pipelining a console request into Field.
fn field_request_head(
    head: &Head,
    target: &str,
    upstream: SocketAddr,
    secure: bool,
    peer_ip: Option<IpAddr>,
) -> String {
    let mut headers = head.headers.clone();
    match headers.insert("host".into(), upstream.to_string()) {
        Some(original) => headers.insert("x-forwarded-host".into(), original),
        None => headers.remove("x-forwarded-host"),
    };
    headers.insert(
        "x-forwarded-proto".into(),
        if secure { "https" } else { "http" }.into(),
    );
    match peer_ip {
        Some(ip) => headers.insert("x-forwarded-for".into(), ip.to_string()),
        None => headers.remove("x-forwarded-for"),
    };
    let upgrade = headers.get("connection").is_some_and(|value| {
        value
            .split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
    });
    if !upgrade {
        headers.insert("connection".into(), "close".into());
    }
    let mut ordered: Vec<_> = headers.into_iter().collect();
    ordered.sort();
    let mut out = format!("{} {} {}\r\n", head.method, target, head.version);
    for (name, value) in ordered {
        out.push_str(&name);
        out.push_str(": ");
        out.push_str(&value);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out
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

/// The request line and headers, before any body is read. What the `/field/`
/// proxy re-serializes; what [`Parsed::Request`] is completed from.
#[derive(Debug, Clone)]
struct Head {
    method: String,
    /// Raw request target (path plus query, undecoded) as the client sent it.
    target: String,
    version: String,
    /// Lowercased names; duplicates were rejected at parse time.
    headers: HashMap<String, String>,
}

/// What arrived on a connection, as far as the server needs to know.
enum Parsed {
    /// A complete request for the application handler.
    Request(Request),
    /// `/field` without the trailing slash: redirect to this location.
    FieldRedirect(String),
    /// `/field/...`: the head is parsed, the body is *not* read — the connection
    /// is spliced to Field and the body reaches it as raw bytes.
    Field { head: Head, target: String },
}

/// Parse a request from any buffered reader. Generic over the reader so it can be
/// unit-tested against an in-memory cursor with no socket.
/// The operator Unix socket (Unix only) and the tests use it; TCP goes through
/// [`parse_incoming`] so the `/field/` proxy can stop at the head.
#[allow(dead_code)]
fn parse_request<R: BufRead>(reader: &mut R) -> Result<Request, ParseError> {
    match parse_incoming(reader, false)? {
        Parsed::Request(request) => Ok(request),
        // Unreachable: with the proxy off, nothing is classified as a field route.
        Parsed::FieldRedirect(_) | Parsed::Field { .. } => Err(ParseError::Malformed),
    }
}

/// Parse the head and, unless it is a `/field` route with the proxy on, the
/// body too. Field routes stop at the head: their method and framing are
/// Field's business, so the method allow-list and the `Transfer-Encoding`
/// refusal below apply only to requests this server answers itself.
fn parse_incoming<R: BufRead>(reader: &mut R, field: bool) -> Result<Parsed, ParseError> {
    let head = parse_head(reader)?;
    if field {
        match field_route(&head.target) {
            Some(FieldRoute::Redirect(location)) => return Ok(Parsed::FieldRedirect(location)),
            Some(FieldRoute::Forward(target)) => return Ok(Parsed::Field { head, target }),
            None => {}
        }
    }
    if !matches!(head.method.as_str(), "GET" | "POST" | "DELETE") {
        return Err(ParseError::Malformed);
    }
    if head.headers.contains_key("transfer-encoding") {
        // This server only implements Content-Length. Accepting TE while ignoring it
        // creates a different message boundary than a reverse proxy may see.
        return Err(ParseError::Malformed);
    }
    let (path, query) = split_target(&head.target)?;
    // `parse_head`'s borrow of the reader ended there; the body is read from the
    // raw reader under its own `MAX_REQUEST_BODY_BYTES` check below.
    let body = match head.headers.get("content-length") {
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

    Ok(Parsed::Request(Request {
        method: head.method,
        path,
        query,
        headers: head.headers,
        body,
        from_unix: false,
        peer_ip: None,
    }))
}

/// Read and validate the request line and headers.
fn parse_head<R: BufRead>(reader: &mut R) -> Result<Head, ParseError> {
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
    let version = parts.next().ok_or(ParseError::Malformed)?.to_string();
    if parts.next().is_some()
        || !matches!(version.as_str(), "HTTP/1.0" | "HTTP/1.1")
        || method.is_empty()
        || !method
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        || !target.starts_with('/')
        || target.contains('#')
        || target.bytes().any(|b| b < 0x21 || b == 0x7f)
    {
        return Err(ParseError::Malformed);
    }

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
    Ok(Head {
        method,
        target,
        version,
        headers,
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
        307 => "Temporary Redirect",
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
            let result = super::handle_connection(stream, None, None, &|_| {
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

    fn head(raw: &str) -> Head {
        let mut cur = Cursor::new(raw.as_bytes().to_vec());
        parse_head(&mut cur).unwrap()
    }

    #[test]
    fn field_route_strips_exactly_the_prefix() {
        assert_eq!(
            field_route("/field/"),
            Some(FieldRoute::Forward("/".into()))
        );
        assert_eq!(
            field_route("/field/api/state?x=1"),
            Some(FieldRoute::Forward("/api/state?x=1".into()))
        );
        assert_eq!(
            field_route("/field/ws"),
            Some(FieldRoute::Forward("/ws".into()))
        );
        assert_eq!(
            field_route("/field"),
            Some(FieldRoute::Redirect("/field/".into()))
        );
        assert_eq!(
            field_route("/field?tab=log"),
            Some(FieldRoute::Redirect("/field/?tab=log".into()))
        );
        assert_eq!(field_route("/fieldnotes"), None);
        assert_eq!(field_route("/api/field/"), None);
        assert_eq!(field_route("/"), None);
    }

    #[test]
    fn field_routes_are_only_recognised_when_the_proxy_is_on() {
        let raw = "PUT /field/api/state HTTP/1.1\r\nHost: console:9090\r\n\r\n";
        let mut cur = Cursor::new(raw.as_bytes().to_vec());
        match parse_incoming(&mut cur, true).unwrap() {
            Parsed::Field { head, target } => {
                assert_eq!(target, "/api/state");
                assert_eq!(head.method, "PUT");
            }
            _ => panic!("expected a field route"),
        }
        // Off: PUT is not a method this server answers, exactly as before.
        let mut cur = Cursor::new(raw.as_bytes().to_vec());
        assert!(matches!(
            parse_incoming(&mut cur, false),
            Err(ParseError::Malformed)
        ));
        let raw = "GET /field HTTP/1.1\r\nHost: console:9090\r\n\r\n";
        let mut cur = Cursor::new(raw.as_bytes().to_vec());
        assert!(matches!(
            parse_incoming(&mut cur, true).unwrap(),
            Parsed::FieldRedirect(location) if location == "/field/"
        ));
        let mut cur = Cursor::new(raw.as_bytes().to_vec());
        assert!(matches!(
            parse_incoming(&mut cur, false).unwrap(),
            Parsed::Request(req) if req.path == "/field"
        ));
    }

    #[test]
    fn field_head_rewrites_host_and_adds_forwarding_headers() {
        let upstream: SocketAddr = "127.0.0.1:7749".parse().unwrap();
        let peer: IpAddr = "10.0.0.8".parse().unwrap();
        let parsed = head(
            "POST /field/api/missions?x=1 HTTP/1.1\r\nHost: console.lan:8443\r\nContent-Length: 2\r\nCookie: field=abc\r\nX-Forwarded-For: 1.2.3.4\r\nConnection: keep-alive\r\n\r\n{}",
        );
        let out = field_request_head(&parsed, "/api/missions?x=1", upstream, true, Some(peer));
        assert!(
            out.starts_with("POST /api/missions?x=1 HTTP/1.1\r\n"),
            "{out}"
        );
        assert!(out.contains("\r\nhost: 127.0.0.1:7749\r\n"), "{out}");
        assert!(
            out.contains("\r\nx-forwarded-host: console.lan:8443\r\n"),
            "{out}"
        );
        assert!(out.contains("\r\nx-forwarded-proto: https\r\n"), "{out}");
        // Ours, not the client's claim.
        assert!(out.contains("\r\nx-forwarded-for: 10.0.0.8\r\n"), "{out}");
        assert!(!out.contains("1.2.3.4"), "{out}");
        assert!(out.contains("\r\ncookie: field=abc\r\n"), "{out}");
        assert!(out.contains("\r\ncontent-length: 2\r\n"), "{out}");
        // One request per spliced connection.
        assert!(out.contains("\r\nconnection: close\r\n"), "{out}");
        assert!(!out.contains("keep-alive"), "{out}");
        assert!(out.ends_with("\r\n\r\n"), "{out}");
        assert_eq!(out.matches("\r\n\r\n").count(), 1, "{out}");
    }

    #[test]
    fn field_head_keeps_upgrade_and_reports_plain_http() {
        let upstream: SocketAddr = "127.0.0.1:7749".parse().unwrap();
        let parsed = head(
            "GET /field/ws HTTP/1.1\r\nHost: console:9090\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: abc\r\n\r\n",
        );
        let out = field_request_head(&parsed, "/ws", upstream, false, None);
        assert!(out.starts_with("GET /ws HTTP/1.1\r\n"), "{out}");
        assert!(
            out.contains("\r\nconnection: keep-alive, Upgrade\r\n"),
            "{out}"
        );
        assert!(out.contains("\r\nupgrade: websocket\r\n"), "{out}");
        assert!(out.contains("\r\nsec-websocket-key: abc\r\n"), "{out}");
        assert!(out.contains("\r\nx-forwarded-proto: http\r\n"), "{out}");
        assert!(!out.contains("x-forwarded-for"), "{out}");
    }

    /// A stand-in for `knossos field`: answers a plain request with its own
    /// received head and body as the response body, and honours an
    /// `Upgrade: websocket` request by switching to a raw echo.
    fn fake_field() -> (std::net::SocketAddr, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                std::thread::spawn(move || {
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut received = Vec::new();
                    let mut byte = [0u8; 1];
                    while !received.ends_with(b"\r\n\r\n") {
                        if stream.read(&mut byte).unwrap_or(0) == 0 {
                            return;
                        }
                        received.push(byte[0]);
                    }
                    let head = String::from_utf8_lossy(&received).to_ascii_lowercase();
                    if head.contains("upgrade: websocket") {
                        stream
                            .write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n")
                            .unwrap();
                        let mut buf = [0u8; 1024];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    if stream.write_all(&buf[..n]).is_err() {
                                        break;
                                    }
                                }
                            }
                        }
                        return;
                    }
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    let mut body = vec![0u8; length];
                    stream.read_exact(&mut body).unwrap();
                    received.extend_from_slice(&body);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        received.len()
                    );
                    stream.write_all(response.as_bytes()).unwrap();
                    stream.write_all(&received).unwrap();
                });
            }
        });
        (address, server)
    }

    fn start_daemon(
        tls: Option<Arc<rustls::ServerConfig>>,
        field: Option<FieldProxy>,
    ) -> (
        std::net::SocketAddr,
        Arc<std::sync::atomic::AtomicBool>,
        std::thread::JoinHandle<std::io::Result<()>>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            serve_with(
                listener,
                tls,
                field,
                |req| Response::error(404, format!("not found: {}", req.path)),
                move || stop_flag.load(Ordering::SeqCst),
            )
        });
        (address, stop, server)
    }

    fn connect(address: std::net::SocketAddr) -> std::net::TcpStream {
        let client = std::net::TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        client
    }

    fn read_until_blank_line(stream: &mut impl Read) -> String {
        let mut received = Vec::new();
        let mut byte = [0u8; 1];
        while !received.ends_with(b"\r\n\r\n") {
            assert_ne!(stream.read(&mut byte).unwrap(), 0, "eof before head end");
            received.push(byte[0]);
        }
        String::from_utf8(received).unwrap()
    }

    /// Exercise the whole proxy over one already-connected client stream:
    /// a plain GET with a body pipelined into the same write, then an upgrade.
    fn check_forwarded_get(mut client: impl Read + Write, upstream: SocketAddr, proto: &str) {
        client
            .write_all(
                b"POST /field/api/state?x=1 HTTP/1.1\r\nHost: console.example:8443\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhello",
            )
            .unwrap();
        let mut reply = Vec::new();
        let _ = client.read_to_end(&mut reply);
        let reply = String::from_utf8(reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        let body = reply.split_once("\r\n\r\n").map_or("", |(_, body)| body);
        assert!(
            body.starts_with("POST /api/state?x=1 HTTP/1.1\r\n"),
            "{body:?}"
        );
        assert!(
            body.contains(&format!("\r\nhost: {upstream}\r\n")),
            "{body:?}"
        );
        assert!(
            body.contains("\r\nx-forwarded-host: console.example:8443\r\n"),
            "{body:?}"
        );
        assert!(
            body.contains(&format!("\r\nx-forwarded-proto: {proto}\r\n")),
            "{body:?}"
        );
        assert!(
            body.contains("\r\nx-forwarded-for: 127.0.0.1\r\n"),
            "{body:?}"
        );
        assert!(
            body.contains("\r\ncontent-type: text/plain\r\n"),
            "{body:?}"
        );
        // The body was buffered by the parser's reader and forwarded first.
        assert!(body.ends_with("\r\n\r\nhello"), "{body:?}");
    }

    fn check_websocket_splice(mut client: impl Read + Write) {
        // Early bytes after the head ride along with the upgrade.
        client
            .write_all(b"GET /field/ws HTTP/1.1\r\nHost: console\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\nearly")
            .unwrap();
        let head = read_until_blank_line(&mut client);
        assert!(
            head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
            "{head}"
        );
        let mut echoed = [0u8; 5];
        client.read_exact(&mut echoed).unwrap();
        assert_eq!(&echoed, b"early");
        for frame in [&b"\x81\x05hello"[..], b"\x88\x00"] {
            client.write_all(frame).unwrap();
            let mut echoed = vec![0u8; frame.len()];
            client.read_exact(&mut echoed).unwrap();
            assert_eq!(echoed, frame);
        }
    }

    #[test]
    fn field_proxy_splices_plain_requests_and_websockets() {
        let (upstream, _fake) = fake_field();
        let (address, stop, server) = start_daemon(None, Some(FieldProxy { upstream }));

        check_forwarded_get(connect(address), upstream, "http");

        let mut client = connect(address);
        check_websocket_splice(&mut client);
        // Client hangs up → Field sees EOF and closes → we close towards the client.
        client.shutdown(Shutdown::Write).unwrap();
        let mut rest = Vec::new();
        client.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty(), "{rest:?}");

        // `/field` itself sends the browser to `/field/`.
        let mut client = connect(address);
        client
            .write_all(b"GET /field?tab=log HTTP/1.1\r\nHost: console\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(
            reply.starts_with("HTTP/1.1 307 Temporary Redirect\r\n"),
            "{reply}"
        );
        assert!(
            reply.contains("\r\nLocation: /field/?tab=log\r\n"),
            "{reply}"
        );

        // Anything outside the prefix still reaches the handler.
        let mut client = connect(address);
        client
            .write_all(b"GET /fieldnotes HTTP/1.1\r\nHost: console\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
        assert!(reply.contains("not found: /fieldnotes"), "{reply}");

        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn field_proxy_splices_through_tls() {
        let dir = std::env::temp_dir().join(format!("cameo-field-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let configured = crate::tls::configure(Some(&dir), "127.0.0.1")
            .unwrap()
            .unwrap();
        let (upstream, _fake) = fake_field();
        let (address, stop, server) =
            start_daemon(Some(configured.config), Some(FieldProxy { upstream }));

        let client_config = Arc::new(
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(TrustAnything))
                .with_no_client_auth(),
        );
        let tls_client = || {
            let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
            let connection =
                rustls::ClientConnection::new(Arc::clone(&client_config), name).unwrap();
            rustls::StreamOwned::new(connection, connect(address))
        };

        check_forwarded_get(tls_client(), upstream, "https");

        let mut client = tls_client();
        check_websocket_splice(&mut client);
        client.conn.send_close_notify();
        let _ = client.flush();
        client.sock.shutdown(Shutdown::Write).unwrap();
        let mut rest = Vec::new();
        let _ = client.read_to_end(&mut rest);
        assert!(rest.is_empty(), "{rest:?}");

        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn field_prefix_is_an_ordinary_route_when_the_proxy_is_off() {
        let (address, stop, server) = start_daemon(None, None);
        for target in ["/field", "/field/", "/field/api/state"] {
            let mut client = connect(address);
            client
                .write_all(format!("GET {target} HTTP/1.1\r\nHost: console\r\n\r\n").as_bytes())
                .unwrap();
            let mut reply = String::new();
            client.read_to_string(&mut reply).unwrap();
            assert!(reply.starts_with("HTTP/1.1 404 Not Found\r\n"), "{reply}");
            assert!(reply.contains(&format!("not found: {target}")), "{reply}");
        }
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().unwrap();
    }

    #[test]
    fn field_proxy_answers_502_when_field_is_down() {
        // A port nothing listens on: bind, note the address, release it.
        let upstream = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let (address, stop, server) = start_daemon(None, Some(FieldProxy { upstream }));
        let mut client = connect(address);
        client
            .write_all(b"GET /field/ HTTP/1.1\r\nHost: console\r\n\r\n")
            .unwrap();
        let mut reply = String::new();
        client.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{reply}");
        let body = reply.split_once("\r\n\r\n").map_or("", |(_, body)| body);
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["status"], 502, "{body:?}");
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("field is not reachable"),
            "{body:?}"
        );
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap().unwrap();
    }
}
