//! Knossos (and any other harness) registers live sessions here so the deck
//! can paint soldiers next to GPUs.
//!
//! This is the Knossos *plugin surface* on a Cameo node: upsert, list, drop.
//! Cameo does not run the agent loop; it only remembers who is fighting.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const STALE: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub role: String,
    /// `ask` | `preview` | `write`
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub state: String,
    /// Stable name of the selected agent engine, e.g. `cameo`.
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub model: String,
    /// Human-readable current plan step; Cameo records it but never interprets it.
    #[serde(default)]
    pub plan_step: String,
    #[serde(default)]
    pub halt: String,
    /// Agent-supplied verification outcome (pass/fail/skipped plus a summary).
    #[serde(default)]
    pub verification: String,
    #[serde(default)]
    pub files: Vec<String>,
    /// Explicit changed-file list for newer harnesses. `files` remains for
    /// compatibility with existing session reporters.
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub summary: String,
    /// Opaque link/id for the agent's trace. It is display-only to Cameo.
    #[serde(default)]
    pub trace_ref: String,
}

fn default_mode() -> String {
    "ask".into()
}

struct Live {
    session: Session,
    seen: Instant,
}

pub struct Board {
    inner: Mutex<BTreeMap<String, Live>>,
}

impl Board {
    pub fn new() -> Self {
        Board {
            inner: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn upsert(&self, mut session: Session) -> Session {
        if session.id.is_empty() {
            session.id = format!("s{}", now_millis());
        }
        if session.name.is_empty() {
            session.name = session.id.clone();
        }
        let id = session.id.clone();
        self.inner.lock().unwrap().insert(
            id,
            Live {
                session: session.clone(),
                seen: Instant::now(),
            },
        );
        session
    }

    pub fn remove(&self, id: &str) -> bool {
        self.inner.lock().unwrap().remove(id).is_some()
    }

    /// Whether a session is live enough to make an explicit resource claim.
    /// The lease API checks this before reserving an endpoint, so a typo cannot
    /// pin VRAM forever without a corresponding session heartbeat.
    pub fn contains(&self, id: &str) -> bool {
        self.inner
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|live| live.seen.elapsed() <= STALE)
    }

    /// Session ids whose last heartbeat is too old to retain a resource lease.
    /// Stale sessions remain visible on the board for diagnosis, but callers
    /// should release their resource claims immediately.
    pub fn stale_ids(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, live)| live.seen.elapsed() > STALE)
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub fn list(&self) -> Vec<Value> {
        let now = Instant::now();
        let mut map = self.inner.lock().unwrap();
        map.retain(|_, live| now.duration_since(live.seen) < STALE * 4);
        map.values()
            .map(|live| {
                let stale = now.duration_since(live.seen) > STALE;
                let mut v = serde_json::to_value(&live.session).unwrap_or(json!({}));
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("stale".into(), json!(stale));
                    obj.insert(
                        "age_secs".into(),
                        json!(now.duration_since(live.seen).as_secs()),
                    );
                }
                v
            })
            .collect()
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a Session from JSON so the serde defaults (`mode` = "ask", empty
    /// vectors) are exercised the same way `POST /api/sessions` hits them.
    fn sess(v: Value) -> Session {
        serde_json::from_value(v).expect("valid session json")
    }

    #[test]
    fn upsert_fills_blank_id_and_name_and_defaults_mode() {
        let board = Board::new();
        let saved = board.upsert(sess(json!({ "id": "", "name": "" })));
        assert!(!saved.id.is_empty(), "a blank id is generated");
        assert_eq!(saved.name, saved.id, "a blank name falls back to the id");
        assert_eq!(saved.mode, "ask", "mode defaults to ask");
    }

    #[test]
    fn upsert_preserves_a_supplied_id_and_updates_in_place() {
        let board = Board::new();
        board.upsert(sess(json!({ "id": "sol-1", "state": "thinking" })));
        board.upsert(sess(json!({
            "id": "sol-1",
            "state": "writing",
            "engine": "cameo",
            "plan_step": "verify parser",
            "verification": "passed",
            "changed_files": ["parser.rs"],
            "trace_ref": "trace-42"
        })));
        let list = board.list();
        assert_eq!(list.len(), 1, "same id updates, not appends");
        assert_eq!(list[0]["id"], "sol-1");
        assert_eq!(list[0]["state"], "writing", "the latest upsert wins");
        assert_eq!(list[0]["engine"], "cameo");
        assert_eq!(list[0]["plan_step"], "verify parser");
        assert_eq!(list[0]["verification"], "passed");
        assert_eq!(list[0]["changed_files"], json!(["parser.rs"]));
        assert_eq!(list[0]["trace_ref"], "trace-42");
    }

    #[test]
    fn list_marks_a_fresh_session_not_stale_and_carries_age() {
        let board = Board::new();
        board.upsert(sess(json!({ "id": "sol-2", "name": "hands" })));
        let list = board.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["stale"], json!(false), "just-seen is not stale");
        assert!(list[0]["age_secs"].is_u64(), "age is reported");
        assert_eq!(list[0]["name"], "hands");
    }

    #[test]
    fn remove_reports_whether_it_deleted() {
        let board = Board::new();
        board.upsert(sess(json!({ "id": "sol-3" })));
        assert!(board.remove("sol-3"), "removing a present id returns true");
        assert!(!board.remove("sol-3"), "removing again returns false");
        assert!(board.list().is_empty());
    }

    #[test]
    fn contains_tracks_the_live_session_id() {
        let board = Board::new();
        board.upsert(sess(json!({ "id": "sol-4" })));
        assert!(board.contains("sol-4"));
        board.remove("sol-4");
        assert!(!board.contains("sol-4"));
    }

    #[test]
    fn stale_sessions_are_not_eligible_for_leases_but_remain_visible() {
        let board = Board::new();
        board.upsert(sess(json!({ "id": "sol-5" })));
        board.inner.lock().unwrap().get_mut("sol-5").unwrap().seen =
            Instant::now() - STALE - Duration::from_secs(1);

        assert!(!board.contains("sol-5"));
        assert_eq!(board.stale_ids(), vec!["sol-5"]);
        assert_eq!(board.list().len(), 1, "the deck retains stale diagnostics");
    }
}
