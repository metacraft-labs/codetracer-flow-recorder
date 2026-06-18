# Flow recorder examples

A small collection of self-contained Cadence programs you can record
and replay end-to-end with CodeTracer. Each fixture is intentionally
tiny so the resulting trace is easy to step through in the GUI.

## Prerequisites

* `ct` on your `PATH`. `ct` is the CodeTracer CLI; it dispatches to
  the language-specific recorder (here, `codetracer-flow-recorder`)
  and opens the trace bundle in the CodeTracer GUI.
* The Flow recorder built locally. From the repository root:

  ```bash
  nix develop      # optional, but pulls in `cadence-trace-helper`
  cargo build --release
  ```

  Make sure the resulting binary (and `cadence-trace-helper`) is on
  the same `PATH` that `ct` searches. Inside `nix develop` this is
  already arranged.

## Programs in `cadence/`

| File | What it exercises |
|---|---|
| `flow_test.cdc` | A trivial top-level script: two locals, one arithmetic chain, one return. The simplest possible thing the recorder can capture. |
| `nested_calls_test.cdc` | A four-deep call chain (`main` -> `compute` -> `outer` -> `middle` -> `inner`) — useful for inspecting the call stack panel. |
| `control_flow_test.cdc` | `if` / `while` / `for-in` / `switch` / early return, all in one fixture. |
| `scripts_test.cdc` + `scripts_test_market.cdc` | A Cadence script that imports a sibling contract. Shows cross-file source mapping. |

All five fixtures are promoted from `test-programs/cadence/` and are
exercised by the recorder's integration tests, so they are guaranteed
to record cleanly.

## Two-step workflow: record, then replay

Record a trace into a `.ct` bundle:

```bash
ct record examples/cadence/flow_test.cdc
```

`ct record` invokes the Flow recorder, which runs the program through
the Go `cadence-trace-helper`, captures step-level events, and writes
a canonical CTFS multi-stream bundle. The bundle path is printed on
stdout — typically `./ct-traces/<program>-<timestamp>/`.

Open the recorded bundle in the GUI:

```bash
ct replay -t ./ct-traces/flow_test-<timestamp>
```

You can replay the same bundle as many times as you like; it is a
self-contained artefact and does not need to be re-recorded.

## One-step workflow: record + open

For a quick poke-around, do both in one shot:

```bash
ct run examples/cadence/flow_test.cdc
```

`ct run` records into a temporary bundle and immediately hands it to
`ct replay`. This is the fastest path from source to GUI.

## Walkthrough: `flow_test.cdc`

The simplest fixture is twelve lines:

```cadence
access(all) fun compute(): Int {
    let a: Int = 10
    let b: Int = 32
    let sum_val: Int = a + b
    let doubled: Int = sum_val * 2
    let final_result: Int = doubled + a
    return final_result
}

access(all) fun main(): Int {
    return compute()
}
```

Record and open it:

```bash
ct run examples/cadence/flow_test.cdc
```

What to look for in the GUI:

1. The first frame is `main` (tagged as the Cadence script entry).
   Step **In** to descend into `compute`.
2. Each `let` produces a `Variable` event with the concrete typed
   value (`ValueRecord::Int(10)`, `ValueRecord::Int(32)`, …). The
   right-hand pane shows them appearing as you step.
3. Step **Out** of `compute` and you land back on the `return
   compute()` line in `main`, with the captured return value
   (`94`) attached to the call exit.

### Column-aware step-over

Cadence's parser exposes a real `ast.Position.Column` for every
syntactic node, and the Flow recorder forwards it through to the CTFS
writer (see `FU-Column-Aware-Nav-Flow` in `src/tracer.rs`). The trace
therefore carries `(line, column)` per step rather than line alone.

Try `nested_calls_test.cdc` to see this in action:

```bash
ct run examples/cadence/nested_calls_test.cdc
```

Inside `middle`, the line

```cadence
    let y: Int = inner(a: x, b: 10) + 0
```

contains both a function call and the binding that consumes its
result. Because the column is real, **step-over** stops once per
column-distinct sub-expression on this line: once at the call to
`inner`, then once at the binding of `y`. A line-only recorder would
collapse both into a single step. This is most useful in dense
expressions and is what makes the GUI's "next" button feel precise
on Cadence code.

## Cross-file scripts

`scripts_test.cdc` imports a sibling contract:

```bash
ct run examples/cadence/scripts_test.cdc
```

The recorder resolves `MarketContract.floor_price` across the two
files and surfaces the qualified name in the function table. You can
step from the script's `main` into `MarketContract.floor_price` and
back without losing the source mapping.

## Where to go next

See [`../README.md`](../README.md) for the recorder's CLI, env vars,
and architecture overview. See `test-programs/cadence/` for a broader
catalogue of fixtures (resources, capabilities, interfaces, events,
crypto, …), all recordable with the same `ct record` / `ct run`.
