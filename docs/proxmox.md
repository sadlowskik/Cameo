# Cameo on a Proxmox guest

Cameo is not a Proxmox plugin. It is a daemon you run **inside** a guest that
has a real AMD GPU. The dashboard and `/v1` then live at **that guest’s IP**.

## Guest

1. PCIe passthrough the card to the VM (`hostpci`, ROM bar, `x-vga` if it is
   the only display). Reboot and confirm `lspci -nn | findstr 1002` (or
   `lspci`) shows the Radeon inside the guest.
2. `/dev/dri` must exist. `/dev/kfd` exists only when ROCm’s stack is present;
   without it Cameo is **Tier 3 (Vulkan only)** and that is fine.
3. LXC: only if you passed the render node through and the unprivileged
   container can open it. If detection reports CPU-only, passthrough failed —
   do not pretend it is a GPU node.

## Run

```bash
# on the guest
cameod --host 127.0.0.1 --port 9090 --console-key "$CAMEO_CONSOLE_KEY"
```

The built-in listener is HTTP-only. Keep it on loopback and use an SSH/Tailscale
tunnel, or terminate TLS at a reverse proxy before exposing it to the LAN.

After tunneling port 9090, open `http://127.0.0.1:9090/` for cards, VRAM, and
load/unload. Inference uses `/v1/chat/completions` on the same protected origin.

## From the operator machine

```bash
cameo fleet status --node 192.168.4.20:9090 --key "$CAMEO_CONSOLE_KEY"
cameo fleet start qwen2.5-7b --node 192.168.4.20:9090 --key "$CAMEO_CONSOLE_KEY"
knossos task "…" --engine cameo --base-url http://192.168.4.20:9090/v1
```

If `start` fails without a key, the command prints the dashboard URL. Load
the model there. Do not SSH a `llama-server` by hand.

## What “not working” looks like

- Dashboard GPUs empty / `readyz` 503 → passthrough or detection fixtures.
- ROCm errors, Vulkan still plans → Tier 3; that is the product, not a crash.
- Two `llama-server`s for one GGUF → a bug; `fleet start` and `resolve_agents`
  must reuse the resident serve.
