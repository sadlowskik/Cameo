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
secret. The matching private key is the `CAMEO_UPDATE_PRIVATE_PEM` secret; tagged
ISO publication now fails closed unless it can sign both the application-update
manifest and `SHA256SUMS`. Never put the private key in git or
on the image. A public key supplied inside an untrusted bundle is not a trust
root.

To provision signing on GitHub: Settings → Secrets and variables → Actions →
New repository secret. Name `CAMEO_UPDATE_PRIVATE_PEM`, paste
`cameo-update-private.pem`. Then tag `v*` or run the ISO workflow. The runner
builds the OS and, on a tag, attaches the signed application bundle and signed
checksums to the GitHub Release.

`scripts/build_update_bundle.py` is the keyless producer used by CI. It creates
a deterministic directory, sorts targets, hashes the bytes it copied, rejects
links/duplicates/unsafe modes, and refuses to replace an existing output. A
release job signs the resulting canonical `manifest.json` only after the build;
the producer never accepts or reads a private key.

Verification checks the signature, manifest structure, file destinations and
component digests. Preflight additionally checks installed compatibility and
available space. Success does not install anything or certify that the system
can boot the bundle. The published application bundle covers the Cameo
CLI/daemon, Knossos binary, update runtime, and its systemd units. `/etc/cameo`
is deliberately excluded because it is persistent operator/appliance state, not
an inactive-slot target. A separately built full-OS payload (kernel, firmware, GPU/runtime packages
and installed root) is still missing, so application-bundle success must not be
described as a full appliance operating-system update. Models remain persistent
data rather than slot payloads.

Installed systems now have two 24 GiB OS slots plus separate boot and
persistent-state partitions. `cameo-update apply` stages the inactive slot and a
systemd-boot assessed trial; `commit` / `rollback` / `recover` follow the host
transaction journal. Legacy single-root images still report apply as unavailable.

A signed bundle must name identities for os, kernel, firmware, mesa_vulkan,
rocm, llama, cameo, knossos, starter_model, and schemas. That is the product
payload contract; overlay files remain allowlisted product paths. Installed
systems record `/etc/cameo/compatibility-id` and
`/etc/cameo/component-identities.json` at install.

Persistent Cameo state is not in either OS slot. Each release declares the
schema it writes and the schemas it can still read (`compatibility.state` in
the signed manifest, `/usr/share/cameo/state-schema.json` on the slot,
`/var/lib/cameo/state-schema.json` on the persistent partition). Apply is
refused if the new release cannot read the live persistent schema. If the new
release writes a schema the prior slot cannot read, rollback and
assessment-fallback restore the pre-update state snapshot *before* selecting
the old slot. A failed restore does not boot the old slot against unreadable
state.

Linux interruption, Secure Boot signing, and hardware soak remain qualification
gates. The regular-file simulator is not a substitute for those runs.

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
