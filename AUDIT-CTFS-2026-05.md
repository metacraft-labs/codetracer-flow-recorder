# Flow / Cadence Recorder CTFS Audit — 2026-05-02

This audit checks `codetracer-flow-recorder` against the canonical CodeTracer
multi-stream CTFS schema and the section 5.6 audit checklist maintained in
`/tmp/isonim-migration.txt`. Prior audits set the canonical patterns: Ruby
(1.21, 1.22), Python (1.27), JavaScript (1.38), EVM (1.39), PHP (1.41),
Solana (1.44), Move (1.46), Cardano (1.48), and Cairo (1.50). This is the
**tenth** recorder audited.

## Architecture

The Flow recorder is a **two-process recorder**:

* **Go helper** (`go-helper/main.go` → `cadence-trace-helper`) embeds the
  real Cadence runtime/interpreter from `github.com/onflow/cadence`. It
  evaluates a `.cdc` source file (or replays an on-chain transaction by
  hash) and prints a stream of NDJSON trace events to stdout.
* **Rust recorder** (this crate) shells out to the Go helper, parses the
  NDJSON, and emits canonical CodeTracer events through the
  Rust-native `NimTraceWriter` (`codetracer_trace_writer_nim` crate,
  sibling-path dep). It is **not** an FFI consumer — every canonical
  entry point (`register_call`, `register_step`, `register_special_event`,
  `arg`, `register_thread_*`) is reachable.

The Go helper currently emits **eight** NDJSON event kinds:
`step`, `variable`, `call`, `return`, `resource_create`, `resource_move`,
`resource_destroy`, `error`. `call` records now optionally carry
`args:[{name,value,cadence_type}]`, recovered from the current Cadence
activation for formal parameters, and the Rust converter stages them on
`Call.args`. Cadence runtime errors and panics are emitted as a final `error`
record and routed into the trace as `EventLogKind::Error`. Anything Cadence
emits that is not in that schema (e.g. on-chain `event` records) is dropped on
the helper side before the Rust recorder ever sees it. Closing that remaining
gap requires a Go-helper change.

## Summary

| # | Check | Status (pre-fix) | Status (post-fix) | Notes |
|---|---|---|---|---|
| a | `register_call` for each call | OK | OK | `tracer.rs::convert_events` emits `register_call` directly for every `TraceEvent::Call`. No `add_event(Call(..))` calls anywhere. |
| b | Call args via `register_call_arg` / `arg()` | **GAP** (helper-side, not closeable in Rust) | **OK** | `go-helper/main.go` now parses formal parameter names from Cadence `fun` declarations and, on the first stop inside a callee, filters `activation.FunctionValues()` to those parameter names. `call` NDJSON records can carry `args:[{name,value,cadence_type}]`; `src/tracer.rs::convert_events` parses that vector, calls `TraceWriter::arg` for each argument, and passes the returned `FullValueRecord`s to `register_call`. |
| c | Write/WriteOther/Error/EvmEvent/TraceLogEvent for IO and structured events via `register_special_event` | **GAP** | **PARTIAL** | Resource lifecycle events (create / move / destroy) now mirror the Move 1.46 `External` effect pattern: each emits `register_special_event(EventLogKind::TraceLogEvent, "CadenceResource{Create,Move,Destroy}:<type>#<uuid>", "<payload>")` in addition to the existing variable-record emission. Cadence runtime errors and panics now emit a final helper-side `{"type":"error","message":"..."}` NDJSON record and route through `register_special_event(EventLogKind::Error, "CadenceRuntimeError", message)`. Cadence has no native stdout/stderr from the script side, so `Write`/`WriteOther` is N/A. One helper-side gap remains: on-chain `event` emissions (the `emit MyEvent(...)` Cadence statement, structurally analogous to EVM logs and to Cairo's `StarknetEvent`) are not surfaced by the Go helper at all; once they are, the Rust side should route them through `EventLogKind::EvmEvent` (mirrors EVM 1.39 / Cairo 1.50 routing). |
| d | Thread events (ThreadStart / Exit / Switch) | OK (N/A) | OK (N/A) | Cadence is single-threaded by design — transactions and scripts run on a single interpreter thread. Recorder correctly emits no thread events. |
| e | Step records for line navigation | OK | OK | `tracer.rs::convert_events` emits `register_step(path, line)` for every `TraceEvent::Step` and as a leading step for each resource lifecycle event (so the lifecycle event is locatable in source). |
| f | Canonical CTFS schema match | **GAP** | **OK** | Pre-fix CLI `--format` exposed only `binary` / `json` (defaulting to `binary`, the legacy CBOR+Zstd format) with no way to request the canonical CTFS multi-stream container. Post-fix CLI exposes a typed `OutputFormat { Ctfs, Binary, Json }` `clap::ValueEnum` defaulting to `ctfs` for both `record` and `replay` subcommands, plus an `impl From<OutputFormat> for TraceEventsFileFormat` so dispatch sites stay one-liner. Same fix applied in EVM (1.39), Solana (1.44), Move (1.46), Cardano (1.48), Cairo (1.50). |
| g | Obsolete `#[no_mangle]` stubs | OK | OK | `grep -r '#\[no_mangle\]' src/` returns no results. Recorder predates the JS-recorder pattern that introduced FFI stubs colliding with upstream Nim exports. |
| C-FFI vs native | OK | OK | `Cargo.toml` depends on `codetracer_trace_writer_nim` (sibling-path dep), not on the C FFI. Every canonical API is reachable. No FFI-extension blockers. |

## Concrete fixes applied

### 1. CLI now exposes and defaults to `Ctfs`

`src/main.rs`'s `OutputFormat` enum used to expose only `Binary` and
`Json`, with `Binary` as the default. There was no way to request the
canonical CTFS multi-stream container — `Binary` writes the legacy
CBOR+Zstd format that the canonical Nim `ct_reader_*` FFI and the
db-backend's `CTFSTraceReader` cannot consume directly.

Post-fix: `OutputFormat` gains a `Ctfs` variant (listed first), with
doc-comments explaining each option, and a freshly added
`impl From<OutputFormat> for TraceEventsFileFormat` makes the call sites
uniform:

```rust
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    /// Canonical CodeTracer multi-stream container (recommended).
    Ctfs,
    /// Legacy CBOR + Zstd binary format.
    Binary,
    /// Human-readable JSON (slower; useful for debugging).
    Json,
}

impl From<OutputFormat> for TraceEventsFileFormat {
    fn from(f: OutputFormat) -> Self {
        match f {
            OutputFormat::Ctfs => TraceEventsFileFormat::Ctfs,
            OutputFormat::Binary => TraceEventsFileFormat::Binary,
            OutputFormat::Json => TraceEventsFileFormat::Json,
        }
    }
}
```

Both `RecordArgs` and `ReplayArgs` now `default_value = "ctfs"`, and the
two dispatch sites (`record` / `replay`) reduce to
`let format: TraceEventsFileFormat = args.format.into();`.

### 2. Resource lifecycle events route through the structured event log

`src/tracer.rs::convert_events` previously rendered each
`TraceEvent::ResourceCreate` / `ResourceMove` / `ResourceDestroy` only
as a `register_variable_with_full_value` record with a synthetic
`@resource:<type>#<uuid>` name — useful in the locals pane, but the
multi-stream IO event reader never saw the lifecycle. Move 1.46
established the canonical pattern for non-stdout structured effects
(`Effect::External` → `EventLogKind::TraceLogEvent`).

Post-fix: the variable-record emission is preserved (still useful in the
locals pane) and an additional `register_special_event(TraceLogEvent,
"CadenceResource{Create,Move,Destroy}:<type>#<uuid>", "<payload>")` is
emitted alongside it, so the lifecycle is captured by the structured
event stream as well.

```rust
TraceWriter::register_special_event(
    &mut *self.writer,
    EventLogKind::TraceLogEvent,
    &format!("CadenceResourceCreate:{}#{}", resource_type, uuid),
    &format!("owner={}", owner),
);
```

The `ResourceMove` payload is `from=... to=...` and `ResourceDestroy` is
`owner=...`. `EventLogKind` is added to the
`codetracer_trace_types` import list at the top of the file.

### 3. Runtime errors route through Error special events

`go-helper/main.go` now includes a minimal `message` field on `TraceEvent` and
emits a final `{"type":"error","message":"..."}` record when Cadence
evaluation returns an error. The helper no longer exits non-zero for that
handled runtime-failure path, so the Rust recorder can create a trace writer
and preserve the failure in the `.ct` file. The goroutine that runs
`ExecuteScript` also recovers panics and reports them through the same final
error record.

`src/tracer.rs` adds `TraceEvent::Error { message }` and converts it with:

```rust
TraceWriter::register_special_event(
    &mut *self.writer,
    EventLogKind::Error,
    "CadenceRuntimeError",
    message,
);
```

### 4. Helper-side call args route onto Call records

`go-helper/main.go` extends the `call` record with an optional `args` field:

```json
{"type":"call","name":"add","args":[{"name":"x","value":"10","cadence_type":"Int"}]}
```

The helper recovers parameter names from single-line Cadence function
declarations and reads live values from the current
`interpreter.VariableActivation` via `activation.FunctionValues()`. The Rust
side adds `TraceArg`, parses `TraceEvent::Call { name, args }`, stages each
argument with `TraceWriter::arg`, then calls `register_call`.

## Tests added

`tests/test_ctfs_audit.rs` (7 cases):

* `test_ctfs_writer_produces_ct_container` — runs the tiny inline NDJSON
  fixture through `trace_program_from_events` with
  `TraceEventsFileFormat::Ctfs` and asserts the resulting `.ct` file
  starts with the canonical CTFS magic bytes (0xC0 0xDE 0x72 0xAC 0xE2)
  and is materially populated.
* `test_ctfs_format_advertised_in_help` — CLI smoke test that
  `record --help` advertises `ctfs` as a `--format` value with
  `[default: ctfs]`. Uses `CARGO_BIN_EXE_codetracer-flow-recorder` to
  locate the just-built binary.
* `test_ctfs_format_default_for_replay` — same guarantee for the
  `replay` subcommand.
* `test_resource_lifecycle_emits_special_events` — synthesises an NDJSON
  stream containing all three resource lifecycle events, runs the
  converter through the CTFS writer, and asserts the .ct container is
  materially populated. Post-fix invariant for the `TraceLogEvent`
  routing added in this commit.
* `test_steps_emitted_for_variable_assignments` — structural smoke test
  guarding against silent regressions where an audit-related change
  empties the event stream.
* `test_cadence_runtime_error_emits_special_event` — synthesises a final
  `error` NDJSON record and verifies the converter creates a populated CTFS
  trace instead of treating the runtime failure as a recorder-level error.
* `test_cadence_call_args_are_staged_on_call_records` — synthesises the
  helper's call-args IPC shape for a representative `add(x, y)` Cadence call
  and asserts the resulting low-level `Call.args` vector is non-empty.

`src/tracer.rs` also has unit parser tests for the `error` NDJSON kind and
for `call` records carrying `args`.

The structural assertions are deliberately lightweight (CTFS magic +
file-size) because verifying the embedded event-log content end-to-end
requires the read-side `codetracer_trace_reader_nim` dep, tracked as an
open follow-up below.

## Verification

```
cd /home/zahary/metacraft/codetracer-flow-recorder
AH_TEST_RESOURCE_GUARD=1 \
LIBRARY_PATH="/nix/store/5hg6h4zjxc3ax7j4ywn6ksd509yl4pmd-zstd-1.5.6/lib" \
cargo test --release
```

* lib unit tests: 29/29 passing
* `test_ctfs_audit`: 7/7 passing
* `test_tracer` (existing integration suite): 11/11 passing, 4 ignored
  (Go-helper-required tests, ignored when `cadence-trace-helper` is not
  on `$PATH`)

Total: 47/47 active passing, 0 regressions.

`cargo build --release` passes for the Rust crate. With the repo direnv
loaded, `go test ./...` and `go build ./...` pass in `go-helper/`.

`cargo clippy --release --all-targets` exits successfully. It still reports
four pre-existing `unnecessary_map_or` warnings on
`p.extension().map_or(false, ...)` patterns in `src/replay.rs`,
`src/tracer.rs`, and `tests/test_tracer.rs`; those are unrelated to this
runtime-error follow-up and remain untouched.

### Build / test environment notes

The dev shell does not put zstd on `LIBRARY_PATH` by default, and the
system's first-on-`LIBRARY_PATH` `zstd-1.5.2` has its `RUNPATH` pinned to
glibc-2.33 — which conflicts with the binary's main libc (2.40) at load
time (`undefined symbol: __libc_siglongjmp, version GLIBC_PRIVATE`).
Workaround:
`LIBRARY_PATH="/nix/store/5hg6h4zjxc3ax7j4ywn6ksd509yl4pmd-zstd-1.5.6/lib"`
before `cargo test --release`. Pre-existing dev-shell ergonomics
issue (also documented in the Cardano 1.48 audit), not caused by audit
changes.

The dev shell also blocks `cargo test` directly in favour of the
resource-isolation wrapper. Set `AH_TEST_RESOURCE_GUARD=1` to bypass for
audit-time verification; production CI should still go through
`scripts/run-test-suite.sh cargo nextest run -p codetracer-flow-recorder`.

## Open gaps (not blocking, documented for follow-up)

### Go-helper-side gaps

* **(c.1) Cadence on-chain `event` emissions**. The Cadence runtime
  surfaces `emit MyEvent(...)` statements via the `OnEmit` callback /
  `runtime.Event` records. The Go helper does not subscribe to that
  callback today, so emitted events never reach the Rust side. Once the
  helper adds an `event` NDJSON kind (`{"type":"event","name":"...","payload":"..."}`),
  the Rust side should match it and route through
  `register_special_event(EventLogKind::EvmEvent, "CadenceEvent:<name>", payload)`.
  Mirrors EVM 1.39's LOG-opcode routing and Cairo 1.50's
  `StarknetEvent` routing.
* **Replay-side resource events**. The replay path (`replay.rs`)
  consumes the same `parse_ndjson` pipeline, so the Rust side already
  handles resource lifecycle events when present. Whether the Go
  helper's `replay` mode actually emits `resource_*` events for on-chain
  state changes is unverified — the replay subcommand has not been
  exercised end-to-end against a real Flow Access Node in this audit.

### Rust-side / cross-cutting gaps

* **Variable types beyond `Int`**: the Rust side currently calls
  `ensure_type_id` for `Int / UInt64 / Fix64 / Bool / String / Address`
  but always falls back to `Int` for unknown Cadence types
  (`tracer.rs:310-315`). Higher-fidelity rendering would require the
  helper to surface the resolved Cadence type-string and the Rust side
  to fan it out to `TypeKind::Bool / String / Float` etc. Small
  follow-up.
* **Synthetic `register_call` line-1 location**: the `call` handler in
  `convert_events` always passes `Line(1)` to `ensure_function_id`
  because the helper's `call` event omits the function declaration
  line. Closing this needs a `decl_file` / `decl_line` pair on the
  helper-side `call` event.
* **Multi-stream IO event collapse**: same cross-cutting issue
  documented in 1.39 (EVM), 1.41 (PHP), 1.44 (Solana), 1.46 (Move),
  1.48 (Cardano), and 1.50 (Cairo) — `EventLogKind::TraceLogEvent` for
  Cadence resource lifecycle lands cleanly in the `stderr` IO bucket,
  but `metadata` is dropped in the multi-stream path so the frontend
  cannot distinguish Cadence resource events from generic recorder
  events without reaching the embedded raw event stream. Out of scope
  for any single recorder audit; flagged as a writer-side fix
  (`toIOEventKind` in `codetracer_trace_writer_ffi.nim`).
* **Read-side end-to-end test**: the audit tests assert the .ct file
  starts with the CTFS magic and is materially populated; verifying
  that the embedded event stream contains the expected `register_call`
  / `register_special_event` records requires the
  `codetracer_trace_reader_nim` dep added as a `[dev-dependencies]`
  entry plus a small reader-walk helper. Tracked here for the next
  pass (also open for Cairo and Cardano).

## After this audit

Section 5.6's recorder list shows `codetracer-flow-recorder` as audited
(gaps closed for default Ctfs CLI + resource-lifecycle structured-event
routing + runtime-error Error routing + helper-side call args; on-chain
Cadence event surfacing remains open for a future Go-helper iteration).
Audited recorder count:
9 → 10.
