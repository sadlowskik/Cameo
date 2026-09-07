//! Durable endpoint intent. Never stores commands, credentials, or live-process claims.
//! Immutable, checksummed generations keep an interrupted write from replacing the
//! committed state. An OS lock prevents concurrent daemon owners and dies with them.
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct IntentState {
    pub endpoints: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub leases: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sessions: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    generation: u64,
    sha256: String,
    state: IntentState,
}

pub struct EndpointStore {
    directory: PathBuf,
    _lock: File,
    generation: u64,
    pub state: IntentState,
}

fn check_path(path: &Path) -> Result<(), String> {
    for component in path.ancestors() {
        match fs::symlink_metadata(component) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("endpoint state must not traverse symlinks".into())
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn digest(state: &IntentState) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(state).map_err(|e| e.to_string())?)
    ))
}

fn read_snapshot(path: &Path) -> Result<Snapshot, String> {
    check_path(path)?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > LIMIT {
        return Err("invalid endpoint snapshot file".into());
    }
    let snapshot: Snapshot = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|_| "corrupt endpoint snapshot; explicit recovery is required".to_string())?;
    if !matches!(snapshot.version, 1..=3) || snapshot.sha256 != digest(&snapshot.state)? {
        return Err("endpoint snapshot version/integrity mismatch".into());
    }
    if snapshot.state.endpoints.len() > 256 {
        return Err("too many persisted endpoints".into());
    }
    if snapshot.state.leases.len() > 4096
        || (snapshot.version == 1 && !snapshot.state.leases.is_empty())
    {
        return Err("invalid persisted lease collection".into());
    }
    for (id, lease) in &snapshot.state.leases {
        if id.is_empty()
            || lease["session_id"].as_str() != Some(id.as_str())
            || lease["model"].as_str().is_none_or(str::is_empty)
            || lease["endpoint_id"].as_str().is_none_or(str::is_empty)
        {
            return Err("invalid persisted lease identity".into());
        }
    }
    if snapshot.state.sessions.len() > 4096
        || (snapshot.version < 3 && !snapshot.state.sessions.is_empty())
    {
        return Err("invalid persisted session collection".into());
    }
    for (id, session) in &snapshot.state.sessions {
        if id.is_empty() || session["id"].as_str() != Some(id.as_str()) {
            return Err("invalid persisted session identity".into());
        }
    }
    Ok(snapshot)
}

impl EndpointStore {
    pub fn has_history(&self) -> bool {
        self.generation > 0
    }
    pub fn open(directory: &Path) -> Result<Self, String> {
        check_path(directory)?;
        fs::create_dir_all(directory).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        let lock_path = directory.join("owner.lock");
        check_path(&lock_path)?;
        if let Ok(meta) = fs::symlink_metadata(&lock_path) {
            if !meta.is_file() {
                return Err("endpoint lock must be a regular file".into());
            }
        }
        let lock = options()
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| e.to_string())?;
        if !lock.metadata().map_err(|e| e.to_string())?.is_file() {
            return Err("invalid endpoint lock file".into());
        }
        fs2::FileExt::try_lock_exclusive(&lock)
            .map_err(|_| "endpoint state is already owned by another daemon".to_string())?;
        let mut store = Self {
            directory: directory.into(),
            _lock: lock,
            generation: 0,
            state: IntentState::default(),
        };
        let generations = store.generations()?;
        if let Some((generation, path)) = generations.last() {
            let snapshot = read_snapshot(path)?;
            if snapshot.generation != *generation {
                return Err(
                    "endpoint snapshot version/integrity mismatch; explicit recovery is required"
                        .into(),
                );
            }
            if snapshot.state.endpoints.len() > 256 {
                return Err("too many persisted endpoints".into());
            }
            store.generation = *generation;
            store.state = snapshot.state;
        }
        Ok(store)
    }

    /// Offline verification takes the same exclusive lock as the daemon.
    pub fn inspect(directory: &Path) -> Result<Value, String> {
        if !directory.is_dir() {
            return Err("endpoint state directory does not exist".into());
        }
        let store = Self::open(directory)?;
        Ok(
            serde_json::json!({"version": 3, "generation": store.generation,
            "endpoints": store.state.endpoints.len(), "integrity": "verified",
            "scope": "endpoint intents only; no model or live process verification"}),
        )
    }

    pub fn backup(directory: &Path, destination: &Path) -> Result<(), String> {
        if !directory.is_dir() {
            return Err("endpoint state directory does not exist".into());
        }
        let store = Self::open(directory)?;
        check_path(destination)?;
        let snapshot = Snapshot {
            version: 3,
            generation: store.generation,
            sha256: digest(&store.state)?,
            state: store.state.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
        let mut file = options()
            .create_new(true)
            .open(destination)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        read_snapshot(destination)?;
        Ok(())
    }

    /// Restore to a new directory only. Never overwrite a running or corrupt store.
    pub fn restore(source: &Path, destination: &Path) -> Result<(), String> {
        let snapshot = read_snapshot(source)?;
        check_path(destination)?;
        if fs::symlink_metadata(destination).is_ok() {
            return Err("restore requires a new destination directory".into());
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|e| e.to_string())?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let staging = parent.join(format!(".cameo-restore-{suffix}"));
        fs::create_dir(&staging).map_err(|e| e.to_string())?;
        let mut store = Self::open(&staging)?;
        store.commit(snapshot.state)?;
        drop(store);
        Self::inspect(&staging)?;
        if fs::symlink_metadata(destination).is_ok() {
            return Err("restore destination appeared during verification".into());
        }
        fs::rename(&staging, destination).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| e.to_string())?;
        Self::inspect(destination)?;
        Ok(())
    }

    fn generations(&self) -> Result<Vec<(u64, PathBuf)>, String> {
        let mut entries = Vec::new();
        for item in fs::read_dir(&self.directory).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            let name = item.file_name().to_string_lossy().into_owned();
            if let Some(number) = name
                .strip_prefix("state-")
                .and_then(|s| s.strip_suffix(".json"))
            {
                let generation = number
                    .parse::<u64>()
                    .map_err(|_| "invalid endpoint generation filename".to_string())?;
                entries.push((generation, item.path()));
            }
        }
        entries.sort_by_key(|(generation, _)| *generation);
        Ok(entries)
    }

    pub fn commit(&mut self, state: IntentState) -> Result<(), String> {
        if state.sessions.len() > 4096 {
            return Err("session limit is 4096".into());
        }
        if state.leases.len() > 4096 {
            return Err("lease limit is 4096".into());
        }
        if state.endpoints.len() > 256 {
            return Err("endpoint limit is 256".into());
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or("endpoint generation exhausted")?;
        let snapshot = Snapshot {
            version: 3,
            generation,
            sha256: digest(&state)?,
            state: state.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > LIMIT {
            return Err("endpoint state exceeds 4 MiB".into());
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|e| e.to_string())?;
        let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let temp = self.directory.join(format!(".pending-{suffix}"));
        let target = self.directory.join(format!("state-{generation:020}.json"));
        let result = (|| -> Result<(), String> {
            let mut file = options()
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|e| e.to_string())?;
            drop(file);
            if target.exists() {
                return Err("endpoint generation already exists".into());
            }
            fs::rename(&temp, &target).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            File::open(&self.directory)
                .and_then(|dir| dir.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        self.generation = generation;
        self.state = state;
        // Keep the preceding committed snapshot for explicit operator recovery.
        // Cleanup failure does not invalidate an already durable transaction.
        if let Ok(entries) = self.generations() {
            for (_, path) in entries.iter().take(entries.len().saturating_sub(2)) {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn directory() -> PathBuf {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).unwrap();
        std::env::temp_dir().join(format!("cameo-state-{:x}", u64::from_le_bytes(bytes)))
    }
    #[test]
    fn durable_generations_lock_and_torn_write() {
        let dir = directory();
        let mut store = EndpointStore::open(&dir).unwrap();
        assert!(EndpointStore::open(&dir).is_err());
        let mut state = IntentState::default();
        state
            .endpoints
            .insert("test".into(), serde_json::json!({"model":"test"}));
        store.commit(state).unwrap();
        fs::write(dir.join(".pending-interrupted"), b"{").unwrap();
        drop(store);
        let mut reopened = EndpointStore::open(&dir).unwrap();
        assert_eq!(reopened.state.endpoints["test"]["model"], "test");
        reopened.commit(IntentState::default()).unwrap();
        drop(reopened);
        assert!(EndpointStore::open(&dir)
            .unwrap()
            .state
            .endpoints
            .is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn corruption_is_not_silently_rolled_back() {
        let dir = directory();
        let mut store = EndpointStore::open(&dir).unwrap();
        store.commit(IntentState::default()).unwrap();
        let path = store.generations().unwrap().pop().unwrap().1;
        drop(store);
        fs::write(path, b"{}").unwrap();
        assert!(EndpointStore::open(&dir).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn offline_backup_restore_and_refusals() {
        let root = directory();
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        let backup = root.join("backup.json");
        let restored = root.join("restored");
        let mut store = EndpointStore::open(&source).unwrap();
        let mut state = IntentState::default();
        state.endpoints.insert(
            "model".into(),
            serde_json::json!({"model":"fixture", "model_sha256":"abc"}),
        );
        store.commit(state.clone()).unwrap();
        assert!(EndpointStore::backup(&source, &backup).is_err());
        assert!(!backup.exists());
        drop(store);
        EndpointStore::backup(&source, &backup).unwrap();
        assert!(EndpointStore::backup(&source, &backup).is_err());
        EndpointStore::restore(&backup, &restored).unwrap();
        assert_eq!(
            EndpointStore::open(&restored).unwrap().state.endpoints,
            state.endpoints
        );
        assert!(EndpointStore::restore(&backup, &restored).is_err());
        fs::write(&backup, b"{}").unwrap();
        let rejected = root.join("rejected");
        assert!(EndpointStore::restore(&backup, &rejected).is_err());
        assert!(
            !rejected.exists(),
            "invalid backups cannot create usable empty state"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_v1_is_verified_and_upgraded_on_commit() {
        let dir = directory();
        fs::create_dir(&dir).unwrap();
        let state = IntentState::default();
        let legacy = Snapshot {
            version: 1,
            generation: 1,
            sha256: digest(&state).unwrap(),
            state,
        };
        fs::write(
            dir.join("state-00000000000000000001.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let mut store = EndpointStore::open(&dir).unwrap();
        assert!(store.state.leases.is_empty());
        store.commit(store.state.clone()).unwrap();
        drop(store);
        assert_eq!(
            read_snapshot(&dir.join("state-00000000000000000002.json"))
                .unwrap()
                .version,
            3
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
