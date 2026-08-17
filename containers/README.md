# containers — GPU passthrough helpers

**Scaffold only (Phase 6).** Podman-preferred, Docker-compatible. Will wrap the
`--device=/dev/kfd --device=/dev/dri` + group-permission boilerplate so
`cameo docker-run <image>` "just works" with AMD GPU access, and will publish
Cameo base images pinned to the same ROCm/Vulkan versions the host ships.

Depends on a working core (Phase 2). Not built yet.
