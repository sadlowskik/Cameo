"""Crash-safe orchestration for an installed Cameo A/B update.

The operating-system adapter is deliberately injected. Unit tests exercise the
entire journal and recovery contract with ordinary directories; the installed
adapter owns destructive block-device and boot-loader operations.
"""

from __future__ import annotations

import contextlib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
from typing import Protocol


SCHEMA = "cameo-update-journal/v2"
LAYOUT_SCHEMA = "cameo-update-layout/v2"
REQUIRED_COMPONENT_IDENTITIES = (
    "os",
    "kernel",
    "firmware",
    "mesa_vulkan",
    "rocm",
    "llama",
    "cameo",
    "knossos",
    "starter_model",
    "schemas",
)
PHASES = (
    "verified",
    "preflighted",
    "drained",
    "state_saved",
    "inactive_written",
    "boot_trial",
    "health_checked",
    "health_committed",
    "rolled_back",
)
TERMINAL_PHASES = {"health_committed", "rolled_back"}
MAX_JSON_BYTES = 1024 * 1024


class UpdateError(RuntimeError):
    """A fail-closed update contract violation."""


class HostOperations(Protocol):
    def observed_active_slot(self, layout: dict) -> str: ...
    def preflight(self, layout: dict, manifest: dict, inactive: str) -> None: ...
    def drain(self) -> None: ...
    def snapshot_state(self, destination: Path) -> str: ...
    def write_inactive(
        self, layout: dict, manifest: dict, bundle: Path, inactive: str
    ) -> str: ...
    def stage_boot_trial(
        self, layout: dict, manifest: dict, inactive: str, generation: str
    ) -> str: ...
    def health_check(self, journal: dict) -> None: ...
    def bless(self, journal: dict) -> None: ...
    def select_prior(self, journal: dict) -> None: ...
    def discard_inactive(self, journal: dict) -> None: ...
    def installed_state_support(self) -> dict: ...
    def persistent_state_schema(self) -> dict: ...
    def restore_state(self, backup: Path) -> None: ...
    def record_persistent_schema(self, writes: int) -> None: ...


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode() + b"\n"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _regular_bytes(path: Path, limit: int = MAX_JSON_BYTES) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as exc:
        raise UpdateError(f"cannot read {path}: {exc}") from exc
    if path.is_symlink() or not path.is_file() or metadata.st_size > limit:
        raise UpdateError(f"unsafe or oversized file: {path}")
    return path.read_bytes()


def load_json(path: Path, expected_schema: str) -> tuple[dict, bytes]:
    raw = _regular_bytes(path)
    try:
        value = json.loads(raw)
    except (UnicodeError, json.JSONDecodeError) as exc:
        raise UpdateError(f"invalid JSON in {path}: {exc}") from exc
    if not isinstance(value, dict) or value.get("schema") != expected_schema:
        raise UpdateError(f"unsupported schema in {path}")
    return value, raw


def validate_layout(layout: dict) -> None:
    if layout.get("schema") != LAYOUT_SCHEMA:
        raise UpdateError("unsupported installed update layout")
    if layout.get("firmware") not in {"uefi", "bios"}:
        raise UpdateError("unsupported firmware layout")
    if layout.get("bootloader") not in {"systemd-boot", "grub"}:
        raise UpdateError("unsupported bootloader")
    if layout["firmware"] == "uefi" and layout["bootloader"] != "systemd-boot":
        raise UpdateError("UEFI A/B requires systemd-boot assessment")
    if layout["firmware"] == "bios" and layout["bootloader"] != "grub":
        raise UpdateError("BIOS A/B requires the GRUB fallback adapter")
    slots = layout.get("slots")
    active = layout.get("active_slot")
    if not isinstance(slots, dict) or set(slots) != {"A", "B"} or active not in slots:
        raise UpdateError("layout must contain exactly slots A and B")
    identities = []
    for slot_name in ("A", "B"):
        slot = slots[slot_name]
        if not isinstance(slot, dict):
            raise UpdateError(f"invalid slot {slot_name}")
        for field in ("partuuid", "device_id"):
            value = slot.get(field)
            if not isinstance(value, str) or not value.strip():
                raise UpdateError(f"slot {slot_name} lacks {field}")
        identities.append(slot["partuuid"])
        boot_entry = slot.get("boot_entry")
        if boot_entry is not None and (
            not isinstance(boot_entry, str) or not boot_entry.strip()
        ):
            raise UpdateError(f"slot {slot_name} has an invalid boot entry")
    if not isinstance(slots[active].get("boot_entry"), str):
        raise UpdateError("active slot lacks a boot entry")
    persistent = layout.get("persistent")
    if not isinstance(persistent, dict):
        raise UpdateError("persistent partition is missing")
    for field in ("partuuid", "device_id"):
        value = persistent.get(field)
        if not isinstance(value, str) or not value.strip():
            raise UpdateError(f"persistent partition lacks {field}")
    identities.append(persistent["partuuid"])
    boot = layout.get("boot")
    if not isinstance(boot, dict):
        raise UpdateError("boot partition is missing")
    for field in ("partuuid", "device_id"):
        value = boot.get(field)
        if not isinstance(value, str) or not value.strip():
            raise UpdateError(f"boot partition lacks {field}")
    identities.append(boot["partuuid"])
    if len(set(identities)) != len(identities):
        raise UpdateError("slot and persistent partition identities alias")
    devices = {
        slots["A"]["device_id"],
        slots["B"]["device_id"],
        persistent["device_id"],
        boot["device_id"],
    }
    if len(devices) != 1:
        raise UpdateError("A/B and persistence must remain on the installed device")


def validate_state_support(value: dict, *, require_reads: bool) -> dict:
    writes = value.get("writes")
    if not isinstance(writes, int) or isinstance(writes, bool) or writes < 1:
        raise UpdateError("state schema writes must be a positive integer")
    raw_reads = value.get("reads", [writes] if not require_reads else None)
    if not isinstance(raw_reads, list) or not raw_reads:
        raise UpdateError("state schema reads must be a nonempty list")
    reads = []
    for item in raw_reads:
        if not isinstance(item, int) or isinstance(item, bool) or item < 1:
            raise UpdateError("state schema reads must be positive integers")
        if item not in reads:
            reads.append(item)
    if writes not in reads:
        raise UpdateError("a release must be able to read the schema it writes")
    return {"writes": writes, "reads": reads}


def validate_component_identities(value: dict) -> dict:
    if not isinstance(value, dict):
        raise UpdateError("manifest lacks the full-OS component identity set")
    missing = [name for name in REQUIRED_COMPONENT_IDENTITIES if name not in value]
    extra = [name for name in value if name not in REQUIRED_COMPONENT_IDENTITIES]
    if missing or extra:
        raise UpdateError(
            "manifest component identities must be exactly "
            + ", ".join(REQUIRED_COMPONENT_IDENTITIES)
        )
    identities = {}
    for name in REQUIRED_COMPONENT_IDENTITIES:
        item = value[name]
        if not isinstance(item, dict):
            raise UpdateError(f"component identity {name} is invalid")
        identity = item.get("id")
        if not isinstance(identity, str) or not identity.strip():
            raise UpdateError(f"component identity {name} lacks id")
        record = {"id": identity.strip()}
        digest = item.get("digest")
        if digest is not None:
            if (
                not isinstance(digest, str)
                or len(digest) != 64
                or any(character not in "0123456789abcdef" for character in digest.lower())
            ):
                raise UpdateError(f"component identity {name} has an invalid digest")
            record["digest"] = digest.lower()
        identities[name] = record
    return identities


def state_support_from_manifest(manifest: dict) -> dict:
    compatibility = manifest.get("compatibility")
    if not isinstance(compatibility, dict):
        raise UpdateError("manifest lacks compatibility identity")
    state = compatibility.get("state")
    if not isinstance(state, dict):
        raise UpdateError("manifest lacks a persistent-state schema contract")
    return validate_state_support(state, require_reads=True)


def validate_manifest(manifest: dict) -> None:
    release = manifest.get("release_id")
    compatibility = manifest.get("compatibility")
    files = manifest.get("files")
    if not isinstance(release, str) or not release.strip():
        raise UpdateError("manifest lacks release_id")
    if not isinstance(compatibility, dict) or not isinstance(
        compatibility.get("id"), str
    ):
        raise UpdateError("manifest lacks compatibility identity")
    state_support_from_manifest(manifest)
    validate_component_identities(manifest.get("identities"))
    if not isinstance(files, list) or not files:
        raise UpdateError("manifest contains no components")
    destinations = set()
    for item in files:
        if not isinstance(item, dict):
            raise UpdateError("manifest contains an invalid component")
        source = item.get("source")
        target = item.get("target")
        digest = item.get("sha256")
        if not all(isinstance(value, str) and value for value in (source, target, digest)):
            raise UpdateError("manifest contains an incomplete component")
        source_path = PurePosixPath(source)
        target_path = PurePosixPath(target)
        if source_path.is_absolute() or ".." in source_path.parts:
            raise UpdateError("manifest contains an unsafe component source")
        if not target_path.is_absolute() or ".." in target_path.parts:
            raise UpdateError("manifest contains an unsafe install target")
        normalized = str(target_path)
        if normalized in destinations:
            raise UpdateError("manifest contains duplicate install targets")
        destinations.add(normalized)
        if len(digest) != 64 or any(character not in "0123456789abcdef" for character in digest.lower()):
            raise UpdateError("manifest contains an invalid component digest")
        try:
            mode = int(item.get("mode", "0644"), 8)
        except (TypeError, ValueError) as exc:
            raise UpdateError("manifest contains an invalid component mode") from exc
        if mode & ~0o755:
            raise UpdateError("manifest component mode grants unsafe permissions")


@contextlib.contextmanager
def writer_lock(path: Path):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.parent.chmod(0o700)
    stream = path.open("a+b")
    acquired = False
    try:
        if os.name == "nt":
            import msvcrt

            if stream.tell() == 0:
                stream.write(b"0")
                stream.flush()
            stream.seek(0)
            msvcrt.locking(stream.fileno(), msvcrt.LK_NBLCK, 1)
        else:
            import fcntl

            fcntl.flock(stream.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        acquired = True
        yield
    except (BlockingIOError, OSError) as exc:
        raise UpdateError("another update transaction owns the writer lock") from exc
    finally:
        if acquired and os.name == "nt":
            import msvcrt

            stream.seek(0)
            msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
        elif acquired:
            import fcntl

            fcntl.flock(stream.fileno(), fcntl.LOCK_UN)
        stream.close()


def _sync_directory(path: Path) -> None:
    if os.name != "nt":
        descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def atomic_json(path: Path, value: dict, *, replace: bool = True) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".next")
    if temporary.exists():
        temporary.unlink()
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(temporary, flags, 0o600)
    try:
        payload = json_bytes(value)
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
    except BaseException:
        with contextlib.suppress(OSError):
            os.close(descriptor)
        with contextlib.suppress(OSError):
            temporary.unlink()
        raise
    if not replace and path.exists():
        temporary.unlink()
        raise UpdateError("an update transaction is already recorded")
    os.replace(temporary, path)
    _sync_directory(path.parent)


class HostTransaction:
    def __init__(
        self,
        layout_path: Path,
        journal_path: Path,
        state_directory: Path,
        operations: HostOperations,
    ):
        self.layout_path = Path(layout_path)
        self.journal_path = Path(journal_path)
        self.state_directory = Path(state_directory)
        self.operations = operations
        self.lock_path = self.journal_path.with_suffix(".lock")

    def _layout(self) -> tuple[dict, bytes]:
        layout, raw = load_json(self.layout_path, LAYOUT_SCHEMA)
        validate_layout(layout)
        return layout, raw

    def _journal(self) -> dict:
        journal, _ = load_json(self.journal_path, SCHEMA)
        if journal.get("phase") not in PHASES:
            raise UpdateError("invalid transaction phase")
        layout, raw = self._layout()
        accepted_layouts = {journal.get("layout_sha256")}
        if journal.get("phase") == "health_checked":
            next_layout = journal.get("next_layout")
            if not isinstance(next_layout, dict):
                raise UpdateError("health commit lacks its prepared layout")
            if sha256_bytes(json_bytes(next_layout)) != journal.get(
                "next_layout_sha256"
            ):
                raise UpdateError("prepared layout hash mismatch")
            accepted_layouts.add(journal.get("next_layout_sha256"))
        observed_layout = sha256_bytes(raw)
        if observed_layout not in accepted_layouts:
            raise UpdateError(
                "installed layout changed during the transaction "
                f"(observed {observed_layout}, expected {sorted(str(value) for value in accepted_layouts)})"
            )
        observed = self.operations.observed_active_slot(layout)
        allowed = {journal["prior_healthy_slot"], journal["inactive_slot"]}
        if observed not in allowed:
            raise UpdateError("booted slot does not belong to this transaction")
        if PHASES.index(journal["phase"]) >= PHASES.index("state_saved"):
            backup_value = journal.get("state_backup")
            backup_hash = journal.get("state_backup_sha256")
            if not isinstance(backup_value, str) or not isinstance(backup_hash, str):
                raise UpdateError("transaction lacks its state backup evidence")
            backup = Path(backup_value)
            try:
                backup.resolve().relative_to((self.state_directory / "backups").resolve())
            except ValueError as exc:
                raise UpdateError("state backup escaped the update directory") from exc
            if backup.is_symlink() or not backup.is_file() or sha256_file(backup) != backup_hash:
                raise UpdateError("state backup evidence changed")
        return journal

    def status(self) -> dict:
        with writer_lock(self.lock_path):
            if not self.journal_path.exists():
                layout, _ = self._layout()
                return {
                    "status": "idle",
                    "active_slot": self.operations.observed_active_slot(layout),
                }
            return dict(self._journal())

    def apply(self, bundle: Path, manifest_path: Path) -> dict:
        bundle = Path(bundle).resolve()
        manifest_path = Path(manifest_path)
        with writer_lock(self.lock_path):
            if self.journal_path.exists():
                old, _ = load_json(self.journal_path, SCHEMA)
                if old.get("phase") not in TERMINAL_PHASES:
                    raise UpdateError("unfinished update requires recover or rollback")
            layout, layout_raw = self._layout()
            manifest, manifest_raw = load_json(manifest_path, "cameo-update/v1")
            validate_manifest(manifest)
            prior_support = validate_state_support(
                self.operations.installed_state_support(), require_reads=True
            )
            persistent = validate_state_support(
                self.operations.persistent_state_schema(), require_reads=False
            )
            target_support = state_support_from_manifest(manifest)
            if persistent["writes"] not in target_support["reads"]:
                raise UpdateError(
                    "refusing update; persistent state schema "
                    f"{persistent['writes']} cannot be read by release {manifest['release_id']}"
                )
            active = self.operations.observed_active_slot(layout)
            if active != layout["active_slot"]:
                raise UpdateError("recorded and booted active slots differ")
            inactive = "B" if active == "A" else "A"
            journal = {
                "schema": SCHEMA,
                "phase": "verified",
                "release_id": manifest["release_id"],
                "active_slot": active,
                "inactive_slot": inactive,
                "prior_healthy_slot": active,
                "prior_boot_entry": layout["slots"][active]["boot_entry"],
                "layout_sha256": sha256_bytes(layout_raw),
                "manifest_sha256": sha256_bytes(manifest_raw),
                "state_backup": None,
                "state_backup_sha256": None,
                "generation": None,
                "boot_entry": None,
                "prior_state_writes": prior_support["writes"],
                "prior_state_reads": prior_support["reads"],
                "target_state_writes": target_support["writes"],
                "target_state_reads": target_support["reads"],
                "rollback_requires_state_restore": target_support["writes"]
                not in prior_support["reads"],
            }
            atomic_json(self.journal_path, journal)
            runtime_manifest = dict(manifest)
            runtime_manifest["_bundle"] = str(bundle)
            self.operations.preflight(layout, runtime_manifest, inactive)
            journal = self._advance(journal, "preflighted")
            self.operations.drain()
            journal = self._advance(journal, "drained")
            backup = self.state_directory / "backups" / (
                sha256_bytes(manifest_raw) + ".json"
            )
            backup.parent.mkdir(parents=True, exist_ok=True)
            backup_hash = self.operations.snapshot_state(backup)
            journal["state_backup"] = str(backup)
            journal["state_backup_sha256"] = backup_hash
            journal = self._advance(journal, "state_saved")
            generation = self.operations.write_inactive(
                layout, manifest, bundle, inactive
            )
            journal["generation"] = generation
            journal = self._advance(journal, "inactive_written")
            entry = self.operations.stage_boot_trial(
                layout, manifest, inactive, generation
            )
            journal["boot_entry"] = entry
            journal = self._advance(journal, "boot_trial")
            return dict(journal)

    def commit(self) -> dict:
        with writer_lock(self.lock_path):
            journal = self._journal()
            if journal["phase"] not in {"boot_trial", "health_checked"}:
                raise UpdateError("only a boot trial can be committed")
            layout, _ = self._layout()
            if self.operations.observed_active_slot(layout) != journal["inactive_slot"]:
                raise UpdateError("health commit must run from the trial slot")
            if journal["phase"] == "boot_trial":
                self.operations.health_check(journal)
                next_layout = dict(layout)
                next_layout["slots"] = {
                    name: dict(slot) for name, slot in layout["slots"].items()
                }
                next_layout["active_slot"] = journal["inactive_slot"]
                next_layout["slots"][journal["inactive_slot"]]["boot_entry"] = journal[
                    "boot_entry"
                ]
                journal["next_layout_sha256"] = sha256_bytes(json_bytes(next_layout))
                journal["next_layout"] = next_layout
                journal = self._advance(journal, "health_checked")
                layout = next_layout
            else:
                layout = journal["next_layout"]
            self.operations.bless(journal)
            if layout["active_slot"] != journal["inactive_slot"]:
                layout["active_slot"] = journal["inactive_slot"]
                layout["slots"][journal["inactive_slot"]]["boot_entry"] = journal[
                    "boot_entry"
                ]
            atomic_json(self.layout_path, layout)
            _, layout_raw = self._layout()
            journal["layout_sha256"] = sha256_bytes(layout_raw)
            journal.pop("next_layout_sha256", None)
            journal.pop("next_layout", None)
            self.operations.record_persistent_schema(int(journal["target_state_writes"]))
            return self._advance(journal, "health_committed")

    def rollback(self) -> dict:
        with writer_lock(self.lock_path):
            journal = self._journal()
            if journal["phase"] in TERMINAL_PHASES:
                raise UpdateError("transaction is already terminal")
            layout, _ = self._layout()
            if layout["active_slot"] != journal["prior_healthy_slot"]:
                layout["active_slot"] = journal["prior_healthy_slot"]
                atomic_json(self.layout_path, layout)
                _, layout_raw = self._layout()
                journal["layout_sha256"] = sha256_bytes(layout_raw)
                journal.pop("next_layout_sha256", None)
                journal.pop("next_layout", None)
            self._restore_prior_state(journal)
            self.operations.select_prior(journal)
            self.operations.discard_inactive(journal)
            return self._advance(journal, "rolled_back", sequential=False)

    def recover(self) -> dict:
        with writer_lock(self.lock_path):
            journal = self._journal()
            if journal["phase"] in TERMINAL_PHASES:
                return dict(journal)
            if PHASES.index(journal["phase"]) < PHASES.index("boot_trial"):
                self._restore_prior_state(journal)
                self.operations.select_prior(journal)
                self.operations.discard_inactive(journal)
                return self._advance(journal, "rolled_back", sequential=False)
            layout, _ = self._layout()
            if (
                self.operations.observed_active_slot(layout)
                == journal["prior_healthy_slot"]
            ):
                # Boot assessment already returned to the old slot. Restore
                # first so that slot never observes a schema it cannot read.
                self._restore_prior_state(journal)
            return dict(journal)

    def _restore_prior_state(self, journal: dict) -> None:
        if not journal.get("rollback_requires_state_restore"):
            return
        if PHASES.index(journal["phase"]) < PHASES.index("boot_trial"):
            return
        backup_value = journal.get("state_backup")
        if not isinstance(backup_value, str):
            raise UpdateError(
                "refusing to boot the prior slot; persistent state was migrated "
                "and no restore snapshot exists"
            )
        self.operations.restore_state(Path(backup_value))
        self.operations.record_persistent_schema(int(journal["prior_state_writes"]))

    def _advance(self, journal: dict, phase: str, *, sequential: bool = True) -> dict:
        if sequential and PHASES.index(phase) != PHASES.index(journal["phase"]) + 1:
            raise UpdateError(f"non-sequential transition {journal['phase']} -> {phase}")
        updated = dict(journal)
        updated["phase"] = phase
        atomic_json(self.journal_path, updated)
        return updated
