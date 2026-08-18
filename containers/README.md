# containers — Cameo as a container

The **Cameo serving image now exists**: [`Containerfile`](Containerfile) builds
`cameod` + the `cameo` CLI + llama.cpp into a portable image (F1 in
`docs/remediation-plan.md`). This is the hero delivery artifact — the same daemon
the ISO ships, packaged to run on any host with a container runtime and to join a
cluster as a node.

## Build

    containers/build.sh          # cameo:vulkan  (universal — any GPU, or CPU)
    containers/build.sh rocm     # + cameo:rocm  (AMD Tier 1/2 accelerator)

Or directly:

    podman build -f containers/Containerfile -t cameo:vulkan .
    podman build -f containers/Containerfile --build-arg EDITION=rocm -t cameo:rocm .

## Run

    # Console on :9090, models on a named volume. The entrypoint prints a
    # generated bearer key on first start (never unauthenticated on a network).
    podman run --rm -p 9090:9090 -v cameo-models:/var/lib/cameo/models cameo:vulkan

    # Use it as the CLI, too:
    podman run --rm -v cameo-models:/var/lib/cameo/models cameo:vulkan cameo pull tinyllama

### GPU passthrough (run time, vendor-specific)

The image is the universal Vulkan build; giving it a GPU is the caller's job:

    # AMD (the recipe core/containers builds programmatically):
    podman run --device /dev/kfd --device /dev/dri \
      --group-add keep-groups --security-opt seccomp=unconfined \
      -p 9090:9090 -v cameo-models:/var/lib/cameo/models cameo:rocm

NVIDIA uses the nvidia-container-toolkit; Intel exposes `/dev/dri` like AMD.

## Two integration boundaries (don't confuse them)

`core/containers` (`cameo-containers`) is the *other* direction: the AMD
GPU-passthrough recipe and `podman`/`docker run` argument builder for a future
`cameo docker-run <image>` — Cameo running *guest* containers with GPU awareness.
That is a library today, distinct from this image (Cameo *as* a container).

## Still to build
- Reproducible builds: pin an Arch archive snapshot + base image digest (F4).
- Publish base images pinned to the host's ROCm/Vulkan versions.
- The socket adapter over the Podman/Docker REST socket + the console's
  Containers tab.
