# Cameo Mesh

Cameo is the local inference appliance and control plane. **Cameo Mesh** is its
request-level pool of machines. **Cameo Link** is the outbound node agent that
joins a machine to that pool. Knossos owns the agent loop; Cameo chooses and
operates the inference capacity behind it.

Mesh is preview functionality. It routes each complete inference workload to one
eligible node. It does not combine VRAM across machines or split one in-flight
model execution across a LAN.

## Security model

- The hub's operator creates a high-entropy, single-use pairing code. It expires
  after ten minutes and can be redeemed once.
- Redemption returns a unique 256-bit device credential. The node stores it in an
  owner-only file; the hub stores only its SHA-256 digest.
- Hub and node identity survive restart. A restored node remains offline and
  ineligible for work until it proves possession in a fresh heartbeat.
- Removing a paired node durably revokes its credential before success is
  returned.
- Node callbacks and hub enrollment require HTTPS. The callback also uses the
  node's operator bearer key. Mutual TLS and certificate-bound device identity
  are not implemented yet.
- The legacy shared farm token is an explicit migration path. Strict dispatch
  excludes legacy nodes unless `allow_legacy_token` is set to `true`.
- Pairing credentials, callback keys, and request bodies are sent to `curl`
  through standard input, not process arguments. Managed `llama-server` API keys
  use a redacted, non-serialized environment field rather than argv.

The hub state contains the node callback key because the hub needs it to manage
that node. Keep the state directory owner-only and back it up like any other
operator credential store. Windows ACL qualification and OS keychain-backed
storage remain release work.

## Pair a node

Start a pairing-only hub behind a TLS endpoint. A farm token is optional and is
needed only for legacy nodes.

```bash
cameod --hub \
  --console-key '<operator key of at least 16 bytes>' \
  --state-dir /var/lib/cameo
```

Create an offer from the operator surface:

```bash
curl -X POST https://hub.example/hub/pairings \
  -H "Authorization: Bearer $CAMEO_CONSOLE_KEY"
```

Run Cameo Link on the node with the returned code. The advertised callback must
be HTTPS and the node key must authorize the hub's later `/api` calls.

```bash
cameod \
  --hub-url https://hub.example \
  --pairing-code '<single-use code>' \
  --mesh-credential-file /var/lib/cameo/mesh-credential.json \
  --advertise https://node-a.example:9090 \
  --console-key '<node operator key of at least 16 bytes>'
```

After the first join, omit `--pairing-code` and keep the credential file. Cameo
Link reconnects with that identity. It refuses to overwrite an existing file;
intentional re-pairing requires removing the revoked credential first.

## Dispatch work

`POST /hub/dispatch` is operator-only. Use a stable `request_id` when executing:
retries with an identical body observe the existing admission instead of
starting the same work twice, while a conflicting reuse is rejected.

```bash
curl -X POST https://hub.example/hub/dispatch \
  -H "Authorization: Bearer $CAMEO_CONSOLE_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "request_id": "mission-42/model-start",
    "model": "qwen2.5-coder-7b",
    "params": 7,
    "execute": true,
    "privacy": "pool",
    "preference": "balanced",
    "deadline_ms": 30000,
    "expected_output_tokens": 2048,
    "priority": 50,
    "protocol_major": 1,
    "allow_legacy_token": false
  }'
```

Hard gates run before scoring: paired trust, current health, owner reclaim,
protocol compatibility, privacy, card constraints, capacity, and deadline.
Eligible nodes are then ranked by valid session affinity, a warm copy of the
model, predicted completion, load, and stable node ID. Active admissions count
against capacity immediately, closing the concurrent double-placement race.

`privacy: "local_only"` intentionally fails on the hub route: a remote pool can
never satisfy a local-only policy. Use the local Cameo engine directly instead.

## Capability truth

Read `GET /api/capabilities` or the `product_capabilities` field in
`GET /api/engines` before enabling optional behavior. The canonical machine-
readable contract is [`contracts/cameo-capabilities-v1.json`](../contracts/cameo-capabilities-v1.json).
Preview and planned features are labeled explicitly; consumers must not infer
availability from branding or documentation prose.
