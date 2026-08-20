# Updating Cameo — F5

A product has to update safely, and the three delivery forms update differently.
`cameod` exposes `GET /api/version` (unauthenticated, like the health probes) so a
console or an external checker can compare the running version against the latest
release and flag "update available".

## Container (hero) — immutable image tags

The container is the safe, trivially-rollback-able path.

```bash
# pull the new image and restart; models live in a volume, so they persist
podman pull ghcr.io/sadlowskik/cameo:1.x
podman stop cameo && podman rm cameo
podman run -d --name cameo -p 9090:9090 \
  -v cameo-models:/var/lib/cameo/models \
  --device=/dev/kfd --device=/dev/dri --group-add video --group-add render \
  ghcr.io/sadlowskik/cameo:1.x
```

Rollback is `podman run … cameo:1.(x-1)` — the previous tag is still there.
Because the model cache is a named volume (F2), nothing is re-downloaded.

## Installed (bare metal) — pinned pacman transaction

An installed system updates through `pacman`, pinned to the same Arch archive
snapshot the image was built against (see [F4](remediation-plan.md)), so an update
is reproducible and a serving box is never surprised by a rolling package:

```bash
# point pacman at the release's pinned snapshot, then update
sudo cameo-update            # wrapper: sets the snapshot mirror, runs pacman -Syu
```

The wrapper is a thin, auditable script; the snapshot pin is what makes the
transaction match a known-good release rather than "whatever is newest today".
Take a filesystem snapshot (btrfs/ZFS) first if the root supports it — then a bad
update is one rollback away.

## ISO appliance — re-flash

The live ISO is immutable by design: to update, download the new ISO and re-flash
the USB. Persistent data lives on the data partition chosen at first boot (F2), not
on the medium, so re-flashing does not lose models or config.

## What the daemon exposes

- `GET /api/version` → `{ "name": "cameod", "version": "…" }`. The console polls
  this and the latest-release version to show an update banner; it never updates
  itself in place (that is the delivery layer's job, above).
- A serving box is never updated mid-flight by Cameo: you pull/restart (container),
  run the pinned transaction (installed), or re-flash (ISO) deliberately.

> Status: the `/api/version` endpoint and the `cameo-update` wrapper both ship now
> (the wrapper reads the snapshot the build recorded at `/etc/cameo/snapshot`, or
> `CAMEO_ARCH_SNAPSHOT`, and falls back to a rolling update with a warning). What
> remains is delivery-layer, not code: publishing the pinned `ghcr.io` image tags
> the container path pulls.
