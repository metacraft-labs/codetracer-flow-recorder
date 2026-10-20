## codetracer-flow-recorder

A recorder for Flow/Cadence smart contracts that produces [CodeTracer](https://github.com/metacraft-labs/CodeTracer) traces.

> **Note:** This project is in early development. APIs and trace formats may change.
> We welcome contributions and discussion!

### Overview

`codetracer-flow-recorder` executes Cadence programs via a Go helper
binary (`cadence-trace-helper`), captures step-level execution traces
with resource lifecycle tracking, and writes them in the canonical
CodeTracer CTFS multi-stream format. Resource events
(`resource_create`, `resource_move`, `resource_destroy`) are recorded
with `@resource:Type#UUID` naming so you can follow the full lifecycle
of Flow resources through the program.

The recorder is **CTFS-only**: it always writes the canonical CodeTracer
CTFS multi-stream format. To convert a recorded `.ct` bundle to JSON
or other human-readable forms, use `ct print` from
[`codetracer-trace-format-nim`](https://github.com/metacraft-labs/codetracer-trace-format-nim).

### Building

```bash
cargo build
```

Or enter the Nix dev shell first:

```bash
nix develop
cargo build
```

### Usage

#### Record a Cadence program

```bash
codetracer-flow-recorder record <file.cdc> --out-dir <dir>
```

Parses the `.cdc` source file, evaluates variable assignments through
the Go tracer helper, captures the execution trace including resource
lifecycle events, and writes a CTFS bundle to `--out-dir`.

#### Replay a Flow on-chain transaction

```bash
just build-replay-helper   # once: target/replay-helper/bin/cadence-replay-helper
CADENCE_REPLAY_HELPER_BIN=target/replay-helper/bin/cadence-replay-helper \
  codetracer-flow-recorder replay --tx-hash <tx-id> --out-dir <dir>
```

Re-executes the transaction with the network's own FVM (flow-go pinned to
the release mainnet runs, in `go-replay-helper/`) against the register
state an access node serves for the parent of its block, after re-executing
the transactions that precede it in the block, with Cadence's statement
debugger attached. `--out-dir` receives:

- the CTFS container, named after the transaction's Cadence location;
- `sources/`, every Cadence location the execution entered, read from the
  same register state -- so the contract source is the code that ran, not
  the account's current code;
- `replay-verdict.json`: the block, index and prestate height, the proposer,
  payer and authorizers, and whether the replay **reproduced** the chain's
  result (status and every event, with values), plus an untraced control run
  so that observing the execution is shown not to have changed it.

The public mainnet access node serves register state from the current
spork's root block (137,390,146) to the sealed tip. Not reproduced yet:
Cadence's `getBlock(at:)` and the per-block entropy behind `revertibleRandom`.
`just replay-smoke` replays a mainnet transaction and fails unless it
reproduced.

#### Convert a recorded trace to JSON

```bash
ct print --json <out-dir>/trace.bin
```

`ct print` is shipped with `codetracer-trace-format-nim` and is the
canonical conversion tool for human-readable output (see
`Recorder-CLI-Conventions.md`
§4).

### Architecture

The recorder is structured around the following modules in `src/`:

| Module | Purpose |
|---|---|
| `main.rs` | CLI entry point (clap) |
| `recorder.rs` | Top-level recording orchestration |
| `tracer.rs` | Step-level trace capture, delegates execution to the Go helper |
| `source_map.rs` | Mapping between execution steps and Cadence source locations |
| `replay.rs` | On-chain transaction replay via Flow Access Node |
| `lib.rs` | Public library API |

### Testing

```bash
cargo test
# or, end-to-end with the CLI convention guard:
just test
```

Test programs live in:

- `test-programs/cadence/` -- Cadence smart contract examples

### Examples

See [`examples/`](examples/README.md) for a short, hands-on walkthrough
of recording and replaying Cadence programs with `ct record`, `ct
replay`, and `ct run`, including a note on Cadence's column-aware
step-over.

### Environment variables

| Variable | Description |
|---|---|
| `CODETRACER_FLOW_RECORDER_OUT_DIR` | Fallback for `--out-dir` when the CLI flag is omitted. The CLI flag always wins. |
| `CODETRACER_FLOW_RECORDER_DISABLED` | Set to `1` or `true` to skip recording entirely. The recorder still validates the input but does not shell out to the Go helper or write any trace artefacts. |
| `CODETRACER_FLOW_RECORDER_LOG_LEVEL` | Recorder log verbosity (advisory; the Flow recorder currently logs to stderr unconditionally). |
| `CADENCE_HELPER_BIN` | Path to the `cadence-trace-helper` Go binary. Defaults to `cadence-trace-helper` on `$PATH`. Use `nix develop` to get it automatically. |

### Contributing

We'd be very happy if the community finds this useful, and if anyone wants to:

* Use and test the Flow/Cadence support or CodeTracer.
* Provide feedback and discuss alternative implementation ideas: in the issue tracker, or in our [discord](https://discord.gg/qSDCAFMP).
* Contribute code to enhance the Flow/Cadence support of CodeTracer.
* Provide [sponsorship](https://opencollective.com/codetracer), so we can hire dedicated full-time maintainers for this project.

### Legal info

LICENSE: MIT

Copyright (c) 2025 Metacraft Labs Ltd
