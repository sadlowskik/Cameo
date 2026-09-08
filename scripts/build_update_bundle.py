#!/usr/bin/env python3
"""Build a deterministic, unsigned Cameo offline update bundle.

Signing is deliberately a separate release-pipeline operation: the private key
never enters this builder.  The detached Ed25519 signature covers the exact
canonical manifest, which in turn covers every copied component byte.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil

try:
    from update_host_transaction import (
        REQUIRED_COMPONENT_IDENTITIES,
        UpdateError,
        validate_component_identities,
    )
except ImportError:
    from scripts.update_host_transaction import (
        REQUIRED_COMPONENT_IDENTITIES,
        UpdateError,
        validate_component_identities,
    )


SCHEMA = "cameo-update/v1"
ALLOWED_TARGETS = (
    "/usr/local/bin/",
    "/usr/local/lib/cameo/",
    "/etc/systemd/system/cameo-",
    "/usr/share/cameo/",
)


class BundleError(RuntimeError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def canonical_json(value: dict) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode() + b"\n"


def normalize_target(value: str) -> str:
    target = PurePosixPath(value)
    if not target.is_absolute() or ".." in target.parts:
        raise BundleError(f"unsafe install target: {value}")
    normalized = str(target)
    if not normalized.startswith(ALLOWED_TARGETS):
        raise BundleError(f"target is outside the product allowlist: {value}")
    return normalized


def build_bundle(
    output: Path,
    *,
    release_id: str,
    compatibility_id: str,
    state_writes: int,
    state_reads: list[int],
    identities: dict,
    components: list[tuple[Path, str, int]],
) -> Path:
    output = Path(output).resolve()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,63}", release_id):
        raise BundleError("release_id contains unsafe characters")
    if not compatibility_id.strip():
        raise BundleError("compatibility_id is empty")
    if isinstance(state_writes, bool) or state_writes < 1:
        raise BundleError("state_writes must be a positive integer")
    if (
        not state_reads
        or state_writes not in state_reads
        or any(isinstance(item, bool) or item < 1 for item in state_reads)
    ):
        raise BundleError("state_reads must contain positive versions including state_writes")
    if not components:
        raise BundleError("at least one update component is required")
    try:
        identities = validate_component_identities(identities)
    except UpdateError as exc:
        raise BundleError(str(exc)) from exc
    if output.exists():
        raise BundleError(f"refusing to replace existing output: {output}")

    prepared = []
    seen_targets = set()
    for source_value, target_value, mode in components:
        source = Path(source_value).resolve()
        if source.is_symlink() or not source.is_file():
            raise BundleError(f"component is missing, linked, or not regular: {source_value}")
        target = normalize_target(target_value)
        if target in seen_targets:
            raise BundleError(f"duplicate install target: {target}")
        if mode < 0 or mode & ~0o755:
            raise BundleError(f"unsafe component mode for {target}: {mode:o}")
        seen_targets.add(target)
        prepared.append((target, source, mode))
    prepared.sort(key=lambda item: item[0])

    temporary = output.with_name(f".{output.name}.next-{os.getpid()}")
    if temporary.exists():
        raise BundleError(f"staging path already exists: {temporary}")
    payload = temporary / "payload"
    payload.mkdir(parents=True, mode=0o700)
    try:
        entries = []
        for index, (target, source, mode) in enumerate(prepared):
            safe_name = re.sub(r"[^A-Za-z0-9._-]", "_", source.name) or "component"
            relative = PurePosixPath("payload") / f"{index:03d}-{safe_name}"
            destination = temporary / Path(*relative.parts)
            with source.open("rb") as read, destination.open("xb") as write:
                shutil.copyfileobj(read, write, 1024 * 1024)
                write.flush()
                os.fsync(write.fileno())
            entries.append(
                {
                    "source": str(relative),
                    "target": target,
                    "sha256": sha256_file(destination),
                    "mode": f"{mode:04o}",
                }
            )
        manifest = {
            "schema": SCHEMA,
            "release_id": release_id,
            "compatibility": {
                "id": compatibility_id,
                "state": {
                    "writes": state_writes,
                    "reads": sorted(set(state_reads)),
                },
            },
            "identities": identities,
            "files": entries,
        }
        manifest_path = temporary / "manifest.json"
        with manifest_path.open("xb") as stream:
            stream.write(canonical_json(manifest))
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, output)
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return output / "manifest.json"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--compatibility-id", required=True)
    parser.add_argument("--state-writes", type=int, required=True)
    parser.add_argument("--state-reads", type=int, action="append", required=True)
    parser.add_argument(
        "--identity",
        action="append",
        required=True,
        metavar="NAME=ID",
        help="repeat for os, kernel, firmware, mesa_vulkan, rocm, llama, cameo, knossos, starter_model, schemas",
    )
    parser.add_argument(
        "--component",
        nargs=3,
        action="append",
        metavar=("SOURCE", "TARGET", "MODE"),
        required=True,
        help="repeat for each source file, absolute install target, and octal mode",
    )
    args = parser.parse_args()
    identities = {}
    for raw in args.identity:
        name, separator, rest = raw.partition("=")
        if not separator or not rest:
            parser.error(f"identity must be NAME=ID: {raw!r}")
        identities[name] = {"id": rest}
    components = []
    for source, target, raw_mode in args.component:
        try:
            mode = int(raw_mode, 8)
        except ValueError as exc:
            parser.error(f"invalid octal component mode {raw_mode!r}: {exc}")
        components.append((Path(source), target, mode))
    try:
        manifest = build_bundle(
            args.output,
            release_id=args.release_id,
            compatibility_id=args.compatibility_id,
            state_writes=args.state_writes,
            state_reads=args.state_reads,
            identities=identities,
            components=components,
        )
    except (BundleError, OSError) as exc:
        parser.error(str(exc))
    print(manifest)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
