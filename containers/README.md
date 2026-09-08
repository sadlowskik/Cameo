# containers — Cameo as a container

The **Cameo serving image exists**: [`Containerfile`](Containerfile) builds
`cameod` + the `cameo` CLI + llama.cpp into a portable developer/deployment image.
It carries the same daemon as the ISO and can join Cameo Mesh as a node. The ISO
remains the primary appliance delivery; release requirements live in
[`PRODUCTIZATION_PLAN.md`](../PRODUCTIZATION_PLAN.md).

## Build

    containers/build.sh          # cameo:vulkan  (universal — any GPU, or CPU)
    containers/build.sh rocm     # + cameo:rocm  (AMD Tier 1/2 accelerator)

Or directly:

    podman build -f containers/Containerfile -t cameo:vulkan .
    podman build -f containers/Containerfile --build-arg EDITION=rocm -t cameo:rocm .

## Run

    # Console on :9090, models on a named volume. Name the container so the
    # generated bearer key can be read without exposing it in shared logs.
    podman run --rm --name cameo -p 9090:9090 \
      -v cameo-models:/var/lib/cameo/models cameo:vulkan
    podman exec cameo cat /var/lib/cameo/models/.console-key

    # Starter model is in the image; the entrypoint seeds it into the volume.
    podman run --rm -v cameo-models:/var/lib/cameo/models cameo:vulkan cameo serve qwen2.5-0.5b

    # Extra models, when you have a network:
    podman run --rm -v cameo-models:/var/lib/cameo/models cameo:vulkan cameo pull tinyllama

### GPU passthrough (run time, vendor-specific)

The image is the universal Vulkan build; giving it a GPU is the caller's job:

    # AMD (the recipe core/containers builds programmatically):
    podman run --device /dev/kfd --device /dev/dri \
      --group-add keep-groups --security-opt seccomp=unconfined \
      -p 9090:9090 -v cameo-models:/var/lib/cameo/models cameo:rocm

NVIDIA uses the nvidia-container-toolkit; Intel exposes `/dev/dri` like AMD.

The image runs as the unprivileged `cameo` account (UID/GID 10001). For a bind
mount instead of a named volume, make the host model directory writable by that
ID. The console is plain HTTP; keep it on a trusted LAN or terminate TLS in a
reverse proxy before exposing it beyond that network.

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
