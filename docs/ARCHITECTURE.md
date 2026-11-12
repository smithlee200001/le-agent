# Architecture

## Process and ownership

```text
GPUI main thread
  AgentView + AppState + input entity
       | Command (bounded Tokio MPSC)
       v
background OS thread / current-thread Tokio runtime
  Session: pending client requests, thread/turn IDs, approval map
       | stdin JSONL         ^ stdout JSONL
       v                     |
  independent `codex app-server` child
       stderr -> bounded local diagnostics
```

The UI drains at most 512 events per 50 ms tick and only calls `notify()` when data changes.
The implementation intentionally uses a simple timer bridge rather than attaching a Tokio runtime to GPUI.
`AppState` is mutated only by the UI thread. Process protocol state belongs to the backend thread.

The client sends numeric request IDs. Server IDs are a distinct direction and retain their original
numeric/string type. `pending` and `approvals` are separate maps, so a server request with ID 3 cannot
be confused with client request 3. Unknown notifications are ignored; unknown requests receive an
RPC method-not-found error. Any future protocol integration should preserve this distinction.

## Connection lifecycle

1. Canonicalize an existing workspace; resolve an explicit relative executable path before changing cwd.
2. Spawn `codex app-server` with piped stdin/stdout/stderr. No shell interpolation.
3. Send `initialize`, wait for its response, then send `initialized`.
4. Start or resume a thread with explicit sandbox, approval policy and human reviewer.
5. Allow one in-flight turn. A late or failed start cannot silently trigger retries.
6. Consume `item/started`, text/output deltas, `item/completed`, diff and turn notifications.
7. Queue early cancellation until the turn ID exists; interrupt response is not turn completion.
8. On EOF, malformed frame, pending RPC timeout or process death, clear the UI's transient state.
9. On shutdown, close stdin, allow two seconds for normal exit, then kill/reap the direct child.

Transport frames are read with `fill_buf` and a size bound before allocation grows. Every frame is
newline-terminated UTF-8 JSON. stdout must contain protocol only. stderr is independent and never
fed to the JSON parser. The RPC response timeout does not put a time limit on an entire model turn
or on a human's approval decision.

## State reconciliation

Items are keyed by `item.id`. Deltas can create provisional items, and final snapshots replace them
rather than appending a second answer. Completion status is taken from `turn/completed`, including
`interrupted` and `failed`; retryable error notifications do not end the turn.

Selected-thread notifications are filtered by thread ID. Approval requests may belong to delegated
threads. Their `serverRequest/resolved` notifications are handled connection-wide by request ID,
so delegated approvals do not remain stale. Approvals are never accepted merely because an item
is shown as completed. Repeated UI clicks and stale IDs do not create duplicate wire responses.

Display limits are intentional but crude; a production UI should virtualize transcript rows and keep
full text outside render clones. Full-history resume is also deliberately not claimed: versioned
pagination needs `thread/turns/list` / `thread/items/list` integration and live-event reconciliation.

## Protocol basis

Validated against the **installed Codex CLI 0.167.0-alpha.4** generated stable JSON schema on
2026-11-12. Actual payload builders are validated by `scripts/check_protocol.py`.
The CLI protocol can evolve independently of this project; rerun that script when upgrading.

- [Official App Server documentation](https://developers.openai.com/codex/app-server/)
- [Official App Server README](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
- [Protocol source/schema](https://github.com/openai/codex/tree/main/codex-rs/app-server-protocol)
- [GPUI 0.2.2 crate](https://docs.rs/crate/gpui/0.2.2)

The wire omits `jsonrpc`. `thread/start.sandbox` is a string such as `workspace-write`, not the
separate object used for a per-turn `sandboxPolicy` override. User text input includes
`text_elements: []`. Approval responses are `{id, result: {decision: "accept" | "decline"}}`.

`availableDecisions` is version/experimental-feature dependent. This client tolerates its absence
and checks it when supplied. It never sends a session/persistent grant. File approval requests may
not contain their diff; inspect the corresponding file-change item and unified diff panel too.

## Next useful increments

1. Replace single-line composer with a proper multiline editor; retain draft on request rejection.
2. Add protocol-version-specific generated types and recorded fixtures for multiple CLI versions.
3. Implement paging/resume hydration and a thread picker with explicit reconnect semantics.
4. Add typed structured questions and permission requests, always with visible user decisions.
5. Add process-group/job-object supervision if strict descendant cleanup is required.
6. Validate macOS/Linux UI flows on real desktops; package and sign binaries only after that.
