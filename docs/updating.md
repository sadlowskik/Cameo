# Updating Cameo

The current checkout does not yet provide a qualified transactional appliance
update. `GET /api/version` reports the daemon version; it does not establish
compatibility, update availability, or successful rollback.

## Installed systems

`cameo-update` now verifies an offline signed bundle and can perform a
non-mutating compatibility/disk-space preflight. It no longer runs
`pacman -Syu` or falls back to rolling Arch repositories.

```bash
cameo-update status
cameo-update verify /path/to/bundle
cameo-update preflight /path/to/bundle
```

A bundle contains `manifest.json`, its detached `manifest.sig`, and the
listed component files. Signature verification requires the independently trusted
release public key at `/etc/cameo/update-root.pem`. Preflight also requires an
installed `/etc/cameo/compatibility-id` matching the signed manifest. Release
ISOs ship that public key from `archiso/airootfs/etc/cameo/update-root.pem`.
GitHub Actions can overwrite it with the `CAMEO_UPDATE_ROOT_PEM` repository
secret. The matching private key is the `CAMEO_UPDATE_PRIVATE_PEM` secret; the
ISO publish job signs `SHA256SUMS` with it. Never put the private key in git or
on the image. A public key supplied inside an untrusted bundle is not a trust
root.

To provision signing on GitHub: Settings → Secrets and variables → Actions →
New repository secret. Name `CAMEO_UPDATE_PRIVATE_PEM`, paste
`cameo-update-private.pem`. Then tag `v*` or run the ISO workflow. The runner
builds the OS and, on a tag, attaches signed checksums to the GitHub Release.

Verification checks the signature, manifest structure, file destinations and
component digests. Preflight additionally checks installed compatibility and
available space. Success does not install anything or certify that the system
can boot the bundle. The current file-bundle schema is an initial increment;
full OS/runtime/model component provenance and migration policy remain open.

`apply`, `commit`, and `rollback` reject the current single-root installation.
The installer uses ext4 with separate boot storage; a filesystem snapshot alone
cannot guarantee rollback of the kernel, bootloader and package database together.
No automatic repartition or destructive migration is provided.

The [Cameo completion plan](completion-cameo.md) specifies the pending A/B design:
separate OS slots, persistent identity/configuration/models/state, bounded trial
boots, representative health probes, and prior-slot fallback. The regular-file
simulator exercises proposed decisions; it is not a bootable implementation.

## Containers

Container replacement must retain the exact previous image digest, complete run
configuration, device mappings, credentials, and persistent volumes. Model-volume
persistence alone does not preserve daemon state or configuration. An image tag
can change and is not evidence of an immutable or available release artifact.

Before a release can advertise this upgrade path, qualify the exact old/new image
digests and state-schema compatibility, drain active requests, back up mutable
state, launch the new image with the retained configuration, and check inference.
Rollback requires the previous image and a compatible state backup; starting an
older binary against a migrated volume is not automatically safe. Publication
workflows are configured for Vulkan and ROCm, but this local audit has not run them
or verified registry artifacts.

## Live ISO media

Updating live media means booting a separately verified replacement ISO. Keep a
verified backup of models, configuration, credentials and persistent state first.
Writing an image to a USB device can destroy partitions on that device, including
a persistence partition. Do not assume re-flashing preserves it. The clean-media
restore and installed-system migration walkthroughs remain qualification gates.

## Required release evidence

Before this update path is called production-ready, retain actual signed bundle
verification, Linux wrapper integration, interrupted staging/journal/boot tests,
health-triggered fallback, previous-version migration and downgrade tests, and
restores preserving models, configuration and user state. Current local simulator
checks do not replace Linux VM or physical appliance evidence.
