//! Atomic gateway admission and request lifetime accounting for planned drain.
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    shutdown: bool,
    active: usize,
    started: Option<Instant>,
    deadline: Duration,
    generation: u64,
    cancelled_through: Option<u64>,
    next_socket: u64,
    sockets: std::collections::BTreeMap<u64, (u64, Connection)>,
}

enum Connection {
    Tcp(std::net::TcpStream),
    #[cfg(unix)]
    Unix(std::os::unix::net::UnixStream),
}
impl Connection {
    fn shutdown(&self) {
        // Separate directions also cover platforms where Both after a half-close fails.
        for direction in [std::net::Shutdown::Read, std::net::Shutdown::Write] {
            match self {
                Self::Tcp(socket) => {
                    let _ = socket.shutdown(direction);
                }
                #[cfg(unix)]
                Self::Unix(socket) => {
                    let _ = socket.shutdown(direction);
                }
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct Drain(Arc<Mutex<State>>);

pub struct Permit {
    drain: Drain,
    generation: u64,
}

impl Permit {
    pub fn cancelled(&self) -> bool {
        self.drain.cancelled(self.generation)
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn register_client(
        &self,
        stream: &std::net::TcpStream,
    ) -> std::io::Result<SocketRegistration> {
        self.drain.register(stream, self.generation)
    }
    #[cfg(unix)]
    pub fn register_unix_client(
        &self,
        stream: &std::os::unix::net::UnixStream,
    ) -> std::io::Result<SocketRegistration> {
        self.drain
            .register_connection(Connection::Unix(stream.try_clone()?), self.generation)
    }
}

pub struct SocketRegistration {
    drain: Drain,
    id: u64,
}
impl Drop for SocketRegistration {
    fn drop(&mut self) {
        self.drain.0.lock().unwrap().sockets.remove(&self.id);
    }
}

impl Drain {
    pub fn admit(&self) -> Option<Permit> {
        let mut state = self.0.lock().unwrap();
        if state.started.is_some() {
            return None;
        }
        state.active += 1;
        Some(Permit {
            drain: self.clone(),
            generation: state.generation,
        })
    }

    pub fn begin(&self, deadline: Duration) -> Value {
        let mut state = self.0.lock().unwrap();
        // Retried drain requests cannot indefinitely extend an existing deadline.
        if state.started.is_none() {
            state.started = Some(Instant::now());
            state.deadline = deadline;
        }
        drop(state);
        self.status()
    }

    pub fn resume(&self) -> Value {
        self.poll();
        let mut state = self.0.lock().unwrap();
        if state.shutdown {
            drop(state);
            return self.status();
        }
        if state.started.take().is_some() {
            state.generation = state.generation.saturating_add(1);
        }
        drop(state);
        self.status()
    }

    pub fn begin_shutdown(&self, deadline: Duration) {
        self.0.lock().unwrap().shutdown = true;
        self.begin(deadline);
    }
    pub fn shutting_down(&self) -> bool {
        self.0.lock().unwrap().shutdown
    }

    pub fn cancelled(&self, generation: u64) -> bool {
        self.poll();
        self.0
            .lock()
            .unwrap()
            .cancelled_through
            .is_some_and(|last| generation <= last)
    }

    pub fn register(
        &self,
        stream: &std::net::TcpStream,
        generation: u64,
    ) -> std::io::Result<SocketRegistration> {
        self.register_connection(Connection::Tcp(stream.try_clone()?), generation)
    }

    fn register_connection(
        &self,
        stream: Connection,
        generation: u64,
    ) -> std::io::Result<SocketRegistration> {
        self.poll();
        let mut state = self.0.lock().unwrap();
        if state
            .cancelled_through
            .is_some_and(|last| generation <= last)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "drain deadline exceeded",
            ));
        }
        let id = state.next_socket;
        state.next_socket = id
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("socket registration exhausted"))?;
        state.sockets.insert(id, (generation, stream));
        Ok(SocketRegistration {
            drain: self.clone(),
            id,
        })
    }

    pub fn poll(&self) {
        let mut state = self.0.lock().unwrap();
        if state
            .started
            .is_some_and(|at| at.elapsed() >= state.deadline)
        {
            let generation = state.generation;
            state.cancelled_through = Some(generation);
            for (owner, socket) in state.sockets.values() {
                if *owner <= generation {
                    socket.shutdown();
                }
            }
        }
    }

    pub fn draining(&self) -> bool {
        self.0.lock().unwrap().started.is_some()
    }

    pub fn status(&self) -> Value {
        self.poll();
        let state = self.0.lock().unwrap();
        let status = match state.started {
            None => "accepting",
            Some(_) if state.active == 0 => "drained",
            Some(started) if started.elapsed() >= state.deadline => "deadline_exceeded",
            Some(_) => "draining",
        };
        json!({"scope":"cameo_gateway", "state":status, "active_requests":state.active,
            "accepting":state.started.is_none(),
            "gateway_idle":state.started.is_some() && state.active == 0,
            "deadline_seconds":state.started.map(|_| state.deadline.as_secs())})
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.drain.0.lock().unwrap().active -= 1;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shutdown_drain_cannot_be_reopened() {
        let drain = Drain::default();
        drain.begin_shutdown(Duration::from_secs(30));
        drain.resume();
        assert!(drain.shutting_down());
        assert!(drain.admit().is_none());
    }
    use super::*;
    #[test]
    fn draining_refuses_new_work_and_waits_for_owned_requests() {
        let drain = Drain::default();
        let first = drain.admit().unwrap();
        let second = drain.admit().unwrap();
        assert_eq!(drain.begin(Duration::ZERO)["state"], "deadline_exceeded");
        assert!(drain.admit().is_none());
        assert_eq!(drain.begin(Duration::from_secs(60))["deadline_seconds"], 0);
        drop(first);
        assert_eq!(drain.status()["active_requests"], 1);
        assert_eq!(drain.status()["gateway_idle"], false);
        drop(second);
        assert_eq!(drain.status()["state"], "drained");
        drain.resume();
        let next = drain.admit().unwrap();
        assert!(
            drain.cancelled(0),
            "resume cannot revive cancelled request generations"
        );
        assert!(!drain.cancelled(next.generation()));
    }
}
