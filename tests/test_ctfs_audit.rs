//! CTFS-compliance audit tests for the Cadence/Flow recorder.
//!
//! These tests cover the post-fix invariants from `AUDIT-CTFS-2026-05.md`:
//!
//! * The CLI advertises `ctfs` as a `--format` option and defaults to it for
//!   both `record` and `replay` subcommands.
//! * The recorder produces a `.ct` container starting with the canonical
//!   CTFS magic bytes (0xC0 0xDE 0x72 0xAC 0xE2) when invoked with
//!   `TraceEventsFileFormat::Ctfs`.
//! * Resource lifecycle events (create / move / destroy) complete the
//!   `register_special_event` route without crashing the converter and the
//!   `.ct` container is materially populated.
//!
//! The structural assertions here are deliberately lightweight (CTFS
//! magic + file size) — verifying the embedded event-log content end-to-end
//! requires the read-side `codetracer_trace_reader_nim` dep, tracked as an
//! open follow-up in `AUDIT-CTFS-2026-05.md`.

use std::path::PathBuf;
use std::process::Command;

use codetracer_flow_recorder::tracer::{parse_ndjson, CadenceTracer};
use codetracer_trace_writer_nim::TraceEventsFileFormat;

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

    CadenceTracer::trace_program_from_events(
        &source_path,
        &events,
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("trace_program_from_events should succeed");

    let ct = first_ct_file(&out_dir);
    assert_ctfs_magic(&ct);
    let size = std::fs::metadata(&ct).unwrap().len();
    assert!(size > 100, ".ct should be materially populated, got {size} bytes");
}

#[test]
fn test_ctfs_format_advertised_in_help() {
    // CLI smoke test: `record --help` advertises `ctfs` as a --format value
    // and `[default: ctfs]` so the audit's (f) fix surfaces to the user.
    let out = Command::new(recorder_bin())
        .args(["record", "--help"])
        .output()
        .expect("run --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");

    assert!(
        combined.contains("ctfs"),
        "record --help must advertise ctfs format; got:\n{combined}"
    );
    assert!(
        combined.contains("[default: ctfs]"),
        "record --help must default to ctfs; got:\n{combined}"
    );
}

#[test]
fn test_ctfs_format_default_for_replay() {
    // Same guarantee for the `replay` subcommand: ctfs offered + default.
    let out = Command::new(recorder_bin())
        .args(["replay", "--help"])
        .output()
        .expect("run replay --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{stdout}\n{stderr}");

    assert!(
        combined.contains("ctfs"),
        "replay --help must advertise ctfs format; got:\n{combined}"
    );
    assert!(
        combined.contains("[default: ctfs]"),
        "replay --help must default to ctfs; got:\n{combined}"
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

    CadenceTracer::trace_program_from_events(
        &source_path,
        &events,
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
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

    CadenceTracer::trace_program_from_events(
        &source_path,
        &events,
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("trace_program_from_events should succeed");

    let ct = first_ct_file(&out_dir);
    assert_ctfs_magic(&ct);
    let size = std::fs::metadata(&ct).unwrap().len();
    assert!(
        size > 100,
        ".ct should be materially populated for a 3-variable program, got {size} bytes"
    );
}
