# Inference memory and speed controls

Cameo sizes and launches `llama-server` with explicit serving controls instead
of relying on upstream defaults.

## Context and KV allocation

- `context` is the context **per slot**. `0` reads native context from GGUF (or
  curated model metadata) and assigns 80%.
- `slots` is physical concurrency. Total llama.cpp context is `context * slots`,
  and the placement engine multiplies KV VRAM by the same value.
- `kv_cache` accepts `f16`, `bf16`, `q8_0`, or `q4_0`. Q8 is the serving default.
- `kv_heads`, `head_dim`, and `layers` normally come from the GGUF. Explicit
  request values override discovered values.
- A dormant Knossos agent does not require another server slot. Increase
  `slots` only for requests that must generate concurrently.

## Throughput and latency

The CLI and daemon expose `batch`, `ubatch`, flash attention, prompt-prefix
reuse, cache RAM, metrics/slot inspection, and an optional slot checkpoint
directory. The generated command enables continuous batching and emits the
corresponding llama.cpp flags.

Start conservative on a new GPU: Q8 KV, one slot, batch 2048, ubatch 512. Raise
slots only after measuring VRAM; lower ubatch first if prompt ingestion OOMs.
Use Q4 KV only after an accuracy comparison on the real task suite.

## Hardware validation

The pure placement and command boundary is covered on non-GPU hosts. A release
candidate still needs one on-device matrix for each packaged llama.cpp backend:

1. Record cold start, warm first-token latency, prompt tokens/s, generation
   tokens/s, peak VRAM/RSS, and `/metrics`/`/slots` output.
2. Compare F16 and Q8 KV at the same context; compare flash attention on/off.
3. Sweep ubatch 256/512/1024 and slots 1/2/4 without changing the task set.
4. Run a long Knossos trajectory past its compaction trigger and save/restore a
   slot when `slot_save_path` is configured.
5. Keep a profile only when it improves measured latency/throughput without an
   OOM, verifier regression, or task-quality loss.

