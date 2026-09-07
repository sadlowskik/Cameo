//! Bound caller latency and outstanding OS resolver work. OS DNS calls themselves
//! cannot be cancelled, so abandoned work continues holding a bounded slot.
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static RESOLVERS: AtomicUsize = AtomicUsize::new(0);
const MAX_RESOLVERS: usize = 4;
struct Slot;
impl Drop for Slot {
    fn drop(&mut self) {
        RESOLVERS.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn backend(host: &str, port: u16, cancelled: impl Fn() -> bool) -> std::io::Result<SocketAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }
    let host = host.to_string();
    bounded(
        move || {
            (host.as_str(), port)
                .to_socket_addrs()?
                .next()
                .ok_or_else(|| std::io::Error::other("backend address unavailable"))
        },
        Duration::from_secs(2),
        cancelled,
    )
}

fn bounded(
    resolve: impl FnOnce() -> std::io::Result<SocketAddr> + Send + 'static,
    timeout: Duration,
    cancelled: impl Fn() -> bool,
) -> std::io::Result<SocketAddr> {
    RESOLVERS
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
            (n < MAX_RESOLVERS).then_some(n + 1)
        })
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "resolver capacity exhausted",
            )
        })?;
    let slot = Slot;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("cameo-resolver".into())
        .spawn(move || {
            let _slot = slot;
            let _ = tx.send(resolve());
        })?;
    let started = Instant::now();
    loop {
        if cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "drain deadline exceeded",
            ));
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "backend resolution deadline exceeded",
            ));
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(std::io::Error::other("backend resolver stopped")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_resolution_has_a_bounded_caller_wait() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let result = bounded(
            move || {
                std::thread::sleep(Duration::from_millis(80));
                done_tx.send(()).unwrap();
                Ok("127.0.0.1:80".parse().unwrap())
            },
            Duration::from_millis(5),
            || false,
        );
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(backend("127.0.0.1", 80, || false).unwrap().port(), 80);
    }
}
