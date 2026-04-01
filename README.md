## codetracer-flow-recorder

A recorder for Flow/Cadence smart contracts that produces [CodeTracer](https://github.com/metacraft-labs/CodeTracer) traces.

> **Note:** This project is in early development. APIs and trace formats may change.
> We welcome contributions and discussion!

### Overview

`codetracer-flow-recorder` executes Cadence programs via a Go helper
binary (`cadence-trace-helper`), captures step-level execution traces
with resource lifecycle tracking, and writes them in the CodeTracer trace
format. Resource events (`resource_create`, `resource_move`,
`resource_destroy`) are recorded with `@resource:Type#UUID` naming so
you can follow the full lifecycle of Flow resources through the program.

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
codetracer-flow-recorder record <file.cdc> --out-dir <dir> [--format binary|json]
```

Parses the `.cdc` source file, evaluates variable assignments through
the Go tracer helper, captures the execution trace including resource
lifecycle events, and writes CodeTracer trace files to `--out-dir`.

#### Replay a Flow on-chain transaction

```bash
codetracer-flow-recorder replay <tx-hash> --out-dir <dir>
```

Fetches the transaction from a Flow Access Node, extracts its Cadence
script and arguments, replays execution through the Go tracer helper,
and writes CodeTracer trace files to `--out-dir`.

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
```

Test programs live in:

- `test-programs/cadence/` -- Cadence smart contract examples

### Environment variables

| Variable | Description |
|---|---|
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
