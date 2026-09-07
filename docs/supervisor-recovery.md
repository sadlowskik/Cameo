# Endpoint recovery and support reports

The daemon stores endpoint intent in `endpoints/` under its configured state
directory. Each successful mutation writes an immutable version-3 snapshot,
syncs it, and renames it into a numbered generation. Linux also syncs the parent
directory. The two most recent generations are retained. An OS advisory lock
prevents two daemons from controlling the same store; it is released on exit or
crash. Snapshots are limited to 4 MiB and 256 endpoints. Unix directories/files
are created with owner-only access. Windows power-loss durability is unqualified.

Start, stop and eviction commit desired state before changing child processes.
Persisted fields include high-level model settings, the observed model digest,
recovery attempt count and last observed health/restarts. Runtime commands, credentials and live PIDs are not replayed from these files.
Session lease ownership is persisted, but restored claims are recovery_required
until explicitly reclaimed against a currently ready model endpoint.

On restart, Cameo checks current model bytes, recomputes placement against current
hardware and refuses occupied ports. It never adopts or kills an unknown process.
A persisted endpoint which cannot be restarted appears as `recovery_required`;
last observed readiness is not live readiness. Recovery is limited to five daemon
restart attempts until manual intervention or five minutes of sustained health.
A successful explicit stop survives daemon restart, including the starter model.

This is partial H9 implementation. Verified process adoption,
device-memory reconciliation, migration beyond version 1, planned drain
and GPU/power-loss qualification remain open. Model hashing reads the entire file
on endpoint start, which adds I/O latency for large models. A custom model's first
observed hash establishes its byte identity, not its provenance or license.

## Failure handling

An incomplete pending write is ignored. A corrupt newest committed snapshot or
unsupported schema fails daemon startup; Cameo does not silently fall back to an
older generation that might resurrect an explicitly stopped endpoint. Preserve
the state directory and logs for diagnosis. Do not remove lock or snapshot files
while a daemon is running.

With the daemon stopped, use the endpoint directory (not its parent state directory):

```sh
cameod state check /var/lib/cameo/endpoints
cameod state backup /var/lib/cameo/endpoints /safe/location/endpoints.json
cameod state restore /safe/location/endpoints.json /safe/location/restored-endpoints
```

Check and backup acquire the daemon's exclusive ownership lock. Backup refuses an
existing destination file. Restore validates the checksum, schema and limits, stages
and verifies a new store, then publishes it to a new destination directory. It never
replaces the existing store or starts processes. A failed staging operation may leave
a hidden `.cameo-restore-*` directory for diagnosis; it is not an active endpoint store.
Move a verified restore into service only with the daemon stopped and after reviewing
the desired endpoints: old intent may restart a model previously stopped by an operator.

These backups contain endpoint configuration and model paths. They exclude model
weights, credentials or mesh identities. Persisted lease ownership is included; live readiness is not. They are not a full
appliance backup. Version 1 endpoint-only snapshots are verified and upgraded on the next commit.
Version 2 adds lease ownership; version 3 adds session recovery identity. Versions
1 and 2 are verified and upgraded on the next commit. Unsupported versions fail closed. Restore round-trip and refusal fixtures pass locally; whole-appliance and
power-loss acceptance remain outstanding.

An occupied model port is reported for intervention. Identify the owning process
locally before taking action; the persisted endpoint record is not proof that
the process belongs to Cameo. Missing or changed model files must be restored to
the intended digest, or explicitly admitted as a new model by the operator.

## Support report

Run `cameo doctor` to preview an allowlisted JSON report. Add
`--bundle <new-file.json>` to save that exact report. Existing files are refused.
The report includes platform, hardware assessments if available, configuration
parse status, credential-presence boolean, model cache totals and runtime binary
presence. It excludes credential values, prompts, raw logs and configuration paths.
Hardware and inference that were not checked are explicitly labeled as such.
This report is neither an inference certification nor a recoverable backup.

Lease writes fail closed: claim/release storage failures return 503, and a failed
release keeps the existing ownership record. Recovered claims conservatively protect
a recovered endpoint from normal eviction; they do not assert live GPU allocation.
An operator can re-register the session and reclaim a ready endpoint or explicitly
release its lease. Claims expire after 90 seconds without renewal. Active session heartbeats renew
the deadline durably; heartbeats alone never reclaim recovered or expired claims.
The maintenance loop releases expired ownership without dashboard traffic. A
failed storage write retains ownership and is retried; status reports expired,
not active. Restart never extends a saved deadline. Legacy claims without a
deadline receive one persisted recovery window; future deadlines are capped to
90 seconds at startup, and a monotonic runtime limit bounds backward clock steps.

Session recovery retains id, name, role, mode, engine and model. Mission text,
workspace paths, changed files and claimed verification are not stored in these
snapshots. Recovered identities show recovery_required, have no live heartbeat or
VRAM authority, and cannot claim resources until a fresh heartbeat arrives. Session
deletion removes its durable identity and lease in one commit. This does not resume
the external agent process or reconstruct its mission; that remains the harness
journal's responsibility. Session identities are bounded to 4,096 records and can
be explicitly deleted through the operator API.

Stop and eviction confirm owned-process exit within a one-second deadline before
dropping the handle. Failure retains ownership, disables restart/routing and reports
an error. Eviction commits desired stops before termination and records replacement
intent only after all victims exit. A partial eviction failure can leave earlier
victims stopped; retry deliberately after inspecting the remaining failed endpoint.
Process-exit confirmation does not establish that a driver has released GPU memory.

## Gateway drain

Operator-authenticated POST /api/drain accepts a JSON object with optional
`deadline_seconds` (1-3600, default 60). GET /api/drain reports active requests
and accepting/draining/drained/deadline_exceeded. DELETE /api/drain resumes
admission. Repeating POST does not reset the current deadline. New gateway
inference receives 503 node_draining with Retry-After; health remains available
and readiness returns 503 during drain.

Accounting lasts through response delivery, including streaming, and releases on
client-write failure or an abandoned response. The gateway_idle flag describes
only requests through Cameo's /v1 gateway. It does not cover clients connected
directly to runtime ports, mesh dispatch or model-management operations. Drain is
currently process-local and resets on daemon restart. Deadline expiry cancels registered upstream sockets. Buffered work returns a
retryable 503; streams with no relayed bytes receive 503, while started streams
terminate with an error without a fabricated completion or second response. Read
polling bounds cancellation checks for silent upstreams. Cancelled generations
remain cancelled after admission resumes. This does not invoke OS shutdown.
DNS callers now have a two-second limit with at most four outstanding resolver
workers; abandoned OS resolution retains its slot until it finishes. Numeric hosts
bypass DNS. Downstream TCP sockets are registered for cancellation, with 250ms
write polling preserving the usual 30-second idle limit. Unix wiring is present
but still needs execution on Unix. In-progress TCP connection attempts can take
up to their five-second connect timeout.
real runtime cancellation/KV release and update/shutdown integration remain gates.

At a hard drain deadline, downstream connections may close without a final JSON
error, including while sending headers or a buffered body. Clients must treat a
truncated response as failed and must not infer successful generation. The retryable
503 remains the response for newly rejected requests while draining.

## Planned service stop

The daemon handles Ctrl-C and, on Unix, SIGTERM/SIGHUP using the pinned ctrlc
termination feature. A signal starts a sealed 30-second gateway drain. Admission
cannot be reopened during shutdown; operator mutations and new endpoint starts
are refused. Existing endpoints have automatic restart disabled. After gateway
completion or five seconds of cancellation grace, owned endpoints are stopped
with a 15-second aggregate stop window, preserving desired intent for the next
boot. Unconfirmed child termination causes a nonzero daemon exit.

The shipped systemd unit uses KillMode=mixed and TimeoutStopSec=75: the initial
termination signal goes to the daemon, while systemd retains a bounded whole-group
kill fallback. This integration has not yet been exercised on an installed Linux
appliance. Terminal Ctrl-C can also reach child processes in the foreground group;
service SIGTERM is the intended graceful production path. Direct runtime clients,
mesh dispatch and driver-level resource release still need qualification.
