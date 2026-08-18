# containers — container tooling notes

The **GPU-passthrough logic now exists** as a real, tested crate:
[`core/containers`](../core/containers) (`cameo-containers`). It builds the AMD
recipe — `--device=/dev/kfd --device=/dev/dri`, the `video`/`render` groups, and
`--security-opt seccomp=unconfined` — and the full `podman`/`docker run` argument
vector for a future `cameo docker-run <image>`. Pure and unit-tested on any OS;
the seccomp/group specifics are a Phase-1 hardware-confirm item.

This directory tracks the remaining, **not-yet-built** container work:
- Packaging a container runtime (Podman preferred) into the image, and wiring a
  `cameo docker-run` command / console action to the builder above. Until then the
  recipe is a library, not a shipping command.
- The socket adapter that lists/inspects/starts running containers (a Linux-gated
  boundary over the Podman/Docker REST socket) and the console's Containers tab.
- Publishing Cameo base images pinned to the same ROCm/Vulkan versions the host
  ships.
