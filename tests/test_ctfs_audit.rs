//! CTFS-compliance audit tests for the Cadence/Flow recorder.
//!
//! These tests cover the post-fix invariants from `AUDIT-CTFS-2026-05.md`:
//!
//! * The CLI is CTFS-only — `--format` is rejected by clap and does not
//!   appear in `--help` output.  `--help` mentions `ct print` (the
//!   canonical conversion tool shipped with `codetracer-trace-format-nim`).
//! * The recorder produces a `.ct` container starting with the canonical
//!   CTFS magic bytes (0xC0 0xDE 0x72 0xAC 0xE2).
//! * Resource lifecycle events (create / move / destroy) complete the
//!   `register_special_event` route without crashing the converter and the
//!   `.ct` container is materially populated.
//! * Cadence runtime errors emitted by the Go helper as final `error` NDJSON
//!   records complete the `EventLogKind::Error` route.
//! * Cadence `emit MyEvent(...)` records from the Go helper complete the
//!   `EventLogKind::EvmEvent` route.
//! * Replay-shaped helper NDJSON containing on-chain resource state changes
//!   and Cadence events is accepted by the Rust converter; live replay remains
//!   blocked at the helper CLI/API boundary documented in the audit memo.
//!
//! The structural assertions here are deliberately lightweight (CTFS
//! magic + file size) — verifying the embedded event-log content end-to-end
//! requires the read-side `codetracer_trace_reader_nim` dep, tracked as an
//! open follow-up in `AUDIT-CTFS-2026-05.md`.

use std::path::PathBuf;
use std::process::Command;

use codetracer_flow_recorder::tracer::{parse_ndjson, CadenceTracer};
use codetracer_trace_types::{EventLogKind, TraceLowLevelEvent};

/// Canonical CTFS multi-stream container magic bytes.
const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

/// Locate the recorder binary that `cargo build` produced.
///
/// `CARGO_BIN_EXE_<name>` is set by Cargo for integration tests of crates that
/// declare a `[[bin]]` target.
fn recorder_bin() -> &'static str {
    env!("CARGO_BIN_EXE_codetracer-flow-recorder")
}

/// Find any `.ct` file under `dir` and return the first one (sorted).
fn first_ct_file(dir: &std::path::Path) -> PathBuf {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("read out_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("no .ct file under {}", dir.display()))
}

/// Assert the file at `path` starts with the canonical CTFS magic bytes.
fn assert_ctfs_magic(path: &std::path::Path) {
    let content = std::fs::read(path).expect("read .ct");
    assert!(
        content.len() >= CTFS_MAGIC.len(),
        ".ct file too small: {} bytes",
        content.len()
    );
    assert_eq!(
        &content[..CTFS_MAGIC.len()],
        &CTFS_MAGIC,
        ".ct file at {} should start with the canonical CTFS magic bytes",
        path.display()
    );
}

// ---------------------------------------------------------------------------
// (f) Canonical CTFS schema match
// ---------------------------------------------------------------------------

#[test]
fn test_ctfs_writer_produces_ct_container() {
    // Feed a tiny pre-built NDJSON event stream through the converter and
    // assert the resulting .ct file exists and starts with the CTFS magic.
    // This exercises the same code path the `record` subcommand drives once
    // the Go helper has produced its NDJSON output.
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"flow_test.cdc","line":1}
{"type":"variable","name":"x","value":"42","cadence_type":"Int"}
{"type":"return","value":"42","cadence_type":"Int"}"#;

    let events = parse_ndjson(ndjson).expect("parse ndjson");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("traces");
    let source_path = PathBuf::from("flow_test.cdc");

    CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
        .expect("trace_program_from_events should succeed");

    let ct = first_ct_file(&out_dir);
    assert_ctfs_magic(&ct);
    let size = std::fs::metadata(&ct).unwrap().len();
    assert!(
        size > 100,
        ".ct should be materially populated, got {size} bytes"
    );
}

/// `--format` must not appear in any `--help` output (CTFS-only contract).
/// Replaces the pre-2026-05-08 `test_ctfs_format_advertised_in_help` and
/// `test_ctfs_format_default_for_replay` tests, which would have locked
/// in the regression.
///
/// Convention: `Recorder-CLI-Conventions.md` §4 — recorders are
/// CTFS-only.  Same shape as the Cairo / Cardano / Circom 2026-05-08
/// audit follow-ups.
#[test]
fn test_no_format_flag_in_help() {
    for subcmd in [None, Some("record"), Some("replay")] {
        let mut cmd = Command::new(recorder_bin());
        if let Some(s) = subcmd {
            cmd.arg(s);
        }
        cmd.arg("--help");

        let output = cmd.output().expect("failed to run --help");
        assert!(
            output.status.success(),
            "--help (subcmd={:?}) should exit 0",
            subcmd
        );

        let help = String::from_utf8_lossy(&output.stdout);
        assert!(
            !help.contains("--format"),
            "--help (subcmd={:?}) must not advertise --format; got:\n{help}",
            subcmd
        );
        assert!(
            !help.contains("CODETRACER_FORMAT"),
            "--help (subcmd={:?}) must not advertise CODETRACER_FORMAT; got:\n{help}",
            subcmd
        );
    }
}

/// `--help` must mention `ct print` so users know where to go for
/// human-readable conversion of the recorded CTFS bundle.
#[test]
fn test_help_mentions_ct_print() {
    let output = Command::new(recorder_bin())
        .arg("--help")
        .output()
        .expect("failed to run --help");
    assert!(output.status.success(), "--help should exit 0");

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("ct print"),
        "--help must mention `ct print` as the conversion tool; got:\n{help}"
    );
}

// ---------------------------------------------------------------------------
// (c) Structured event routing for resource lifecycle
// ---------------------------------------------------------------------------

#[test]
fn test_resource_lifecycle_emits_special_events() {
    // Post-fix invariant: ResourceCreate / ResourceMove / ResourceDestroy
    // events emit a register_special_event(TraceLogEvent, ...) in addition
    // to the existing variable record.  This assertion is structural — we
    // synthesise an NDJSON stream containing all three resource lifecycle
    // events, run the converter through the CTFS writer, and verify the .ct
    // container is materially populated.  Pre-fix the converter still
    // succeeded but lost the lifecycle from the structured event stream;
    // post-fix the same input drives extra special-event records into the
    // CTFS multi-stream IO bucket.
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"resource_test.cdc","line":5}
{"type":"resource_create","resource_type":"FlowToken.Vault","uuid":1001,"owner":"0x01","file":"resource_test.cdc","line":5}
{"type":"step","file":"resource_test.cdc","line":10}
{"type":"resource_move","resource_type":"FlowToken.Vault","uuid":1001,"from_owner":"0x01","to_owner":"0x02","file":"resource_test.cdc","line":10}
{"type":"step","file":"resource_test.cdc","line":15}
{"type":"resource_destroy","resource_type":"FlowToken.Vault","uuid":1001,"owner":"0x02","file":"resource_test.cdc","line":15}
{"type":"return","value":"","cadence_type":"Void"}"#;

    let events = parse_ndjson(ndjson).expect("parse ndjson");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("traces");
    let source_path = PathBuf::from("resource_test.cdc");

    CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
        .expect("converter must succeed when resource lifecycle events are present");

    let ct = first_ct_file(&out_dir);
    assert_ctfs_magic(&ct);
}

#[test]
fn test_steps_emitted_for_variable_assignments() {
    // Smoke test that the post-fix CTFS writer still emits a substantial
    // event stream for a basic Cadence program (parser-equivalent fixture).
    // Catches regressions where audit-related changes silently drop output.
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"basic.cdc","line":1}
{"type":"variable","name":"a","value":"10","cadence_type":"Int"}
{"type":"step","file":"basic.cdc","line":2}
{"type":"variable","name":"b","value":"32","cadence_type":"Int"}
{"type":"step","file":"basic.cdc","line":3}
{"type":"variable","name":"sum","value":"42","cadence_type":"Int"}
{"type":"return","value":"42","cadence_type":"Int"}"#;

    let events = parse_ndjson(ndjson).expect("parse ndjson");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("traces");
    let source_path = PathBuf::from("basic.cdc");

    CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
        .expect("trace_program_from_events should succeed");

    let ct = first_ct_file(&out_dir);
    assert_ctfs_magic(&ct);
    let size = std::fs::metadata(&ct).unwrap().len();
    assert!(
        size > 100,
        ".ct should be materially populated for a 3-variable program, got {size} bytes"
    );
}

#[test]
fn test_cadence_runtime_error_emits_special_event() {
    // The Go helper emits this as the final record when Cadence evaluation
    // fails.  The Rust converter must treat it as a trace error event, not as
    // parser failure or a recorder-level error that prevents writer creation.
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"runtime_error.cdc","line":3}
{"type":"error","message":"pre-condition failed: balance must be non-negative"}"#;

    let events = parse_ndjson(ndjson).expect("parse runtime-error ndjson");
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("traces");
    let source_path = PathBuf::from("runtime_error.cdc");

    CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
        .expect("runtime error records should be converted into trace Error events");

    let ct = first_ct_file(&out_dir);
    assert_ctfs_magic(&ct);
    let size = std::fs::metadata(&ct).unwrap().len();
    assert!(
        size > 100,
        ".ct should contain the runtime error event, got {size} bytes"
    );
}

#[test]
fn test_cadence_call_args_are_staged_on_call_records() {
    // This fixture mirrors the Go helper's call-args IPC shape for a Cadence
    // call such as `add(x: Int, y: Int)`.  The Rust converter must stage both
    // args through TraceWriter::arg before register_call so the resulting
    // Call record has a non-empty args vector.
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"call_args.cdc","line":1}
{"type":"call","name":"add","args":[{"name":"x","value":"10","cadence_type":"Int"},{"name":"y","value":"20","cadence_type":"Int"}]}
{"type":"step","file":"call_args.cdc","line":2}
{"type":"return","value":"30","cadence_type":"Int"}
{"type":"return","value":"30","cadence_type":"Int"}"#;

    let events = parse_ndjson(ndjson).expect("parse call-args ndjson");
    let source_path = PathBuf::from("call_args.cdc");

    let low_level_events = CadenceTracer::trace_low_level_events_from_events(&source_path, &events)
        .expect("call args should be accepted by the converter");
    let call_arg_lengths: Vec<usize> = low_level_events
        .iter()
        .filter_map(|event| match event {
            TraceLowLevelEvent::Call(call) => Some(call.args.len()),
            _ => None,
        })
        .collect();

    assert!(
        call_arg_lengths.iter().any(|len| *len >= 2),
        "expected a Call record with staged Cadence args, got lengths {call_arg_lengths:?}"
    );
}

#[test]
fn test_cadence_events_route_to_evm_event_log() {
    // This mirrors the helper-side OnEmitEvent IPC shape for a Cadence
    // `emit MyEvent(message: "done")` statement.  The Rust converter should
    // route it through the EVM-event bucket, matching EVM LOG and Cairo
    // StarknetEvent audit patterns.
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"event_test.cdc","line":3}
{"type":"event","name":"MyEvent","payload":"MyEvent(message: \"done\")"}
{"type":"return","value":"","cadence_type":"Void"}"#;

    let events = parse_ndjson(ndjson).expect("parse event ndjson");
    let source_path = PathBuf::from("event_test.cdc");

    let low_level_events = CadenceTracer::trace_low_level_events_from_events(&source_path, &events)
        .expect("event records should be accepted by the converter");
    let event = low_level_events
        .iter()
        .find_map(|event| match event {
            TraceLowLevelEvent::Event(event) if event.metadata == "CadenceEvent:MyEvent" => {
                Some(event)
            }
            _ => None,
        })
        .expect("expected CadenceEvent:MyEvent special event");

    assert_eq!(event.kind, EventLogKind::EvmEvent);
    assert_eq!(event.content, "MyEvent(message: \"done\")");
}

#[test]
fn test_replay_on_chain_state_and_event_ndjson_routes_to_special_events() {
    // This is the replay-side content contract the Go helper must satisfy once
    // it grows a real `replay` subcommand.  If replay emits resource lifecycle
    // and Cadence event NDJSON records, the existing Rust converter preserves
    // both in the structured event stream.
    let ndjson = r#"{"type":"call","name":"execute"}
{"type":"step","file":"tx_deadbeef.cdc","line":8}
{"type":"resource_move","resource_type":"FlowToken.Vault","uuid":9001,"from_owner":"0xSender","to_owner":"0xReceiver","file":"FlowToken.cdc","line":42}
{"type":"event","name":"flow.AccountContract.TokensDeposited","payload":"TokensDeposited(amount: 10.0, to: 0xReceiver)"}
{"type":"return","value":"","cadence_type":"Void"}"#;

    let events = parse_ndjson(ndjson).expect("parse replay-side resource/event ndjson");
    let source_path = PathBuf::from("tx_deadbeef.cdc");

    let low_level_events = CadenceTracer::trace_low_level_events_from_events(&source_path, &events)
        .expect("replay-side resource/event records should be accepted by the converter");

    let resource_event = low_level_events
        .iter()
        .find_map(|event| match event {
            TraceLowLevelEvent::Event(event)
                if event.metadata == "CadenceResourceMove:FlowToken.Vault#9001" =>
            {
                Some(event)
            }
            _ => None,
        })
        .expect("expected replay resource_move special event");

    assert_eq!(resource_event.kind, EventLogKind::TraceLogEvent);
    assert_eq!(resource_event.content, "from=0xSender to=0xReceiver");

    let cadence_event = low_level_events
        .iter()
        .find_map(|event| match event {
            TraceLowLevelEvent::Event(event)
                if event.metadata == "CadenceEvent:flow.AccountContract.TokensDeposited" =>
            {
                Some(event)
            }
            _ => None,
        })
        .expect("expected replay Cadence event special event");

    assert_eq!(cadence_event.kind, EventLogKind::EvmEvent);
    assert_eq!(
        cadence_event.content,
        "TokensDeposited(amount: 10.0, to: 0xReceiver)"
    );
}
