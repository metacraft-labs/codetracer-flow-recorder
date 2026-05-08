//! Integration tests for the Cadence tracer.
//!
//! Tests are split into the following categories:
//!
//! 1. **NDJSON-based tests** (always run): These feed pre-built NDJSON data
//!    through the tracer's `trace_program_from_events` path and verify the
//!    resulting CTFS trace output.  They do NOT require the Go helper binary.
//!
//! 2. **`ct-print` content test** (skipped if `ct-print` is not present):
//!    Records a fixture and pipes the resulting `.ct` container through
//!    `ct print` from `codetracer-trace-format-nim` for content-level
//!    assertions.  Skips gracefully when run outside the metacraft
//!    workspace.
//!
//! 3. **CLI env-var contract**: Exercises the post-2026-05-08
//!    `CODETRACER_FLOW_RECORDER_OUT_DIR` /
//!    `CODETRACER_FLOW_RECORDER_DISABLED` env vars and the no-`--format`
//!    invariant from `Recorder-CLI-Conventions.md` §4 / §5.  These tests
//!    invoke the recorder binary via `CARGO_BIN_EXE_*`.
//!
//! 4. **Go-helper integration tests** (marked `#[ignore]`): These require
//!    the `cadence-trace-helper` Go binary to be built and available.
//!
//! History note: pre-2026-05-08 the recorder shipped a `--format
//! ctfs|binary|json` flag and the CLI integration test
//! (`test_go_helper_cli_record`) drove it with `--format json`.  When
//! the convention switched to CTFS-only the flag was removed and the
//! test was rewritten to drop the `--format` argument.  See
//! `AUDIT-CTFS-2026-05.md` ("Convention compliance follow-up") for
//! the full record.

use std::path::{Path, PathBuf};
use std::process::Command;

use codetracer_flow_recorder::tracer::{parse_ndjson, CadenceTracer, TraceEvent};

/// CTFS magic bytes: C0 DE 72 AC E2
const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/cadence")
}

/// Path to the `ct-print` binary shipped with `codetracer-trace-format-nim`.
///
/// The Flow recorder is CTFS-only; tests that need to make
/// content-level assertions on a recorded trace pipe the `.ct`
/// container through `ct-print --json` and assert on the resulting
/// JSON.  This is the same workflow that `Recorder-CLI-Conventions.md`
/// §4 prescribes for downstream tools / golden snapshots.
fn ct_print_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("codetracer-trace-format-nim")
        .join("ct-print")
}

fn flow_test_ndjson() -> &'static str {
    r#"{"type":"call","name":"main"}
{"type":"step","file":"flow_test.cdc","line":11}
{"type":"call","name":"compute"}
{"type":"step","file":"flow_test.cdc","line":2}
{"type":"variable","name":"a","value":"10","cadence_type":"Int"}
{"type":"step","file":"flow_test.cdc","line":3}
{"type":"variable","name":"b","value":"32","cadence_type":"Int"}
{"type":"step","file":"flow_test.cdc","line":4}
{"type":"variable","name":"sum_val","value":"42","cadence_type":"Int"}
{"type":"step","file":"flow_test.cdc","line":5}
{"type":"variable","name":"doubled","value":"84","cadence_type":"Int"}
{"type":"step","file":"flow_test.cdc","line":6}
{"type":"variable","name":"final_result","value":"94","cadence_type":"Int"}
{"type":"step","file":"flow_test.cdc","line":7}
{"type":"return","value":"94","cadence_type":"Int"}
{"type":"return","value":"94","cadence_type":"Int"}"#
}

fn run_tracer_from_ndjson(ndjson: &str, source_path: &Path, out_dir: &Path) {
    let events = parse_ndjson(ndjson).expect("NDJSON should parse");
    CadenceTracer::trace_program_from_events(source_path, &events, out_dir)
        .expect("trace_program_from_events should succeed");
}

fn run_tracer_on_file(source_path: &Path, out_dir: &Path) {
    codetracer_flow_recorder::recorder::record(source_path, out_dir)
        .expect("trace_program should succeed");
}

/// Helper: collect every `.ct` file in `out_dir`.
fn ct_files_in(out_dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(out_dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect()
}

fn assert_valid_ct_file(out_dir: &Path) -> PathBuf {
    let ct_files = ct_files_in(out_dir);

    assert!(
        !ct_files.is_empty(),
        "expected at least one .ct file in {}, found: {:?}",
        out_dir.display(),
        std::fs::read_dir(out_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect::<Vec<_>>()
    );

    let ct_path = &ct_files[0];
    let content = std::fs::read(ct_path).expect("failed to read .ct file");
    assert!(content.len() >= CTFS_MAGIC.len(), ".ct file too small");
    assert_eq!(
        &content[..CTFS_MAGIC.len()],
        &CTFS_MAGIC,
        ".ct file should start with CTFS magic bytes"
    );

    ct_path.clone()
}

fn resource_lifecycle_ndjson() -> &'static str {
    r#"{"type":"call","name":"main"}
{"type":"step","file":"resource_test.cdc","line":5}
{"type":"resource_create","resource_type":"FlowToken.Vault","uuid":1001,"owner":"0x01","file":"resource_test.cdc","line":5}
{"type":"step","file":"resource_test.cdc","line":10}
{"type":"resource_move","resource_type":"FlowToken.Vault","uuid":1001,"from_owner":"0x01","to_owner":"0x02","file":"resource_test.cdc","line":10}
{"type":"step","file":"resource_test.cdc","line":15}
{"type":"resource_destroy","resource_type":"FlowToken.Vault","uuid":1001,"owner":"0x02","file":"resource_test.cdc","line":15}
{"type":"return","value":"","cadence_type":"Void"}"#
}

fn nested_resource_ndjson() -> &'static str {
    r#"{"type":"call","name":"main"}
{"type":"step","file":"nested_resource.cdc","line":3}
{"type":"resource_create","resource_type":"NFT.Collection","uuid":2001,"owner":"0x10","file":"nested_resource.cdc","line":3}
{"type":"step","file":"nested_resource.cdc","line":4}
{"type":"resource_create","resource_type":"NFT.Token","uuid":2002,"owner":"0x10","file":"nested_resource.cdc","line":4}
{"type":"step","file":"nested_resource.cdc","line":5}
{"type":"resource_create","resource_type":"NFT.Token","uuid":2003,"owner":"0x10","file":"nested_resource.cdc","line":5}
{"type":"step","file":"nested_resource.cdc","line":8}
{"type":"resource_move","resource_type":"NFT.Collection","uuid":2001,"from_owner":"0x10","to_owner":"0x20","file":"nested_resource.cdc","line":8}
{"type":"step","file":"nested_resource.cdc","line":9}
{"type":"resource_move","resource_type":"NFT.Token","uuid":2002,"from_owner":"0x10","to_owner":"0x20","file":"nested_resource.cdc","line":9}
{"type":"step","file":"nested_resource.cdc","line":12}
{"type":"resource_destroy","resource_type":"NFT.Token","uuid":2003,"owner":"0x10","file":"nested_resource.cdc","line":12}
{"type":"step","file":"nested_resource.cdc","line":13}
{"type":"resource_destroy","resource_type":"NFT.Collection","uuid":2001,"owner":"0x20","file":"nested_resource.cdc","line":13}
{"type":"return","value":"","cadence_type":"Void"}"#
}

// ===========================================================================
// NDJSON-based tests (always run, no Go helper needed)
// ===========================================================================

#[test]
fn test_ndjson_trace_output_files() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let ct_path = assert_valid_ct_file(&out_dir);
    let size = std::fs::metadata(&ct_path).unwrap().len();
    assert!(
        size > 100,
        ".ct file should have substantial content, got {} bytes",
        size
    );
}

#[test]
fn test_ndjson_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ndjson_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ndjson_step_events() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ndjson_metadata_structure() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_ndjson_function_calls() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_parse_resource_lifecycle_events() {
    let events = parse_ndjson(resource_lifecycle_ndjson()).expect("should parse resource NDJSON");

    let create_count = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::ResourceCreate { .. }))
        .count();
    let move_count = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::ResourceMove { .. }))
        .count();
    let destroy_count = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::ResourceDestroy { .. }))
        .count();

    assert_eq!(create_count, 1, "should have 1 resource_create event");
    assert_eq!(move_count, 1, "should have 1 resource_move event");
    assert_eq!(destroy_count, 1, "should have 1 resource_destroy event");
}

#[test]
fn test_convert_resource_events_to_trace() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("resource_test.cdc");

    run_tracer_from_ndjson(resource_lifecycle_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_resource_events_produce_steps() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("resource_test.cdc");

    run_tracer_from_ndjson(resource_lifecycle_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_mixed_resource_and_regular_events() {
    let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"mixed.cdc","line":2}
{"type":"variable","name":"balance","value":"100","cadence_type":"Int"}
{"type":"step","file":"mixed.cdc","line":3}
{"type":"resource_create","resource_type":"FlowToken.Vault","uuid":5001,"owner":"0x01","file":"mixed.cdc","line":3}
{"type":"step","file":"mixed.cdc","line":4}
{"type":"variable","name":"amount","value":"50","cadence_type":"Int"}
{"type":"step","file":"mixed.cdc","line":5}
{"type":"resource_move","resource_type":"FlowToken.Vault","uuid":5001,"from_owner":"0x01","to_owner":"0x02","file":"mixed.cdc","line":5}
{"type":"return","value":"50","cadence_type":"Int"}"#;

    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("mixed.cdc");

    run_tracer_from_ndjson(ndjson, &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

#[test]
fn test_nested_resource_tracking() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("nested_resource.cdc");

    run_tracer_from_ndjson(nested_resource_ndjson(), &source_path, &out_dir);
    assert_valid_ct_file(&out_dir);
}

// ===========================================================================
// CTFS content via `ct-print` — replaces the legacy `--format json` test
// ===========================================================================

/// Drive the NDJSON-based tracer with the canonical `flow_test.cdc`
/// fixture, then convert the produced `.ct` container to JSON via
/// `ct-print` and assert on:
///
/// 1. **Structural anchors** (legacy layer): `ct-print --json` output
///    contains the source filename / variable names / canonical Cadence
///    `Int` values somewhere in the textual rendering.
/// 2. **Exact decoded values** (the layer enabled by `ct-print --full`):
///    the `flow_test.cdc` program executes `(10 + 32) * 2 + 10 = 94`
///    via the `compute()` function, with intermediate let-bindings
///    `a=10`, `b=32`, `sum_val=42`, `doubled=84`, `final_result=94`.
///    Each binding must surface in the trace as a step event with a
///    decoded `Int` ValueRecord whose `i` field matches the literal
///    value from the source program.
///
/// Pre-2026-05-08 a similar assertion was made directly on a recorder-
/// emitted `trace.json` file (via `--format json`).  The convention now
/// mandates CTFS-only output; `ct print` is the canonical conversion
/// tool.  See `Recorder-CLI-Conventions.md` §4.  `ct-print --full`
/// (added 2026-05 in `codetracer-trace-format-nim`) is what enables the
/// exact-value layer — its output is a deterministic JSON document with
/// every CBOR `ValueRecord` decoded to a structured form like
/// `{"kind":"Int","i":42,"type_id":7}`.
///
/// Skips gracefully (with a printed `SKIP:` line) when `ct-print` is
/// not present (e.g. when the crate is built outside the metacraft
/// workspace).
#[test]
fn test_recorded_trace_via_ct_print_json() {
    let ct_print = ct_print_path();
    if !ct_print.exists() {
        eprintln!(
            "SKIP: ct-print not found at {} — only available within the \
             metacraft workspace where codetracer-trace-format-nim is a sibling.",
            ct_print.display()
        );
        return;
    }

    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    // Drive the recorder through the NDJSON path so this test does not
    // require the Go helper.  The CTFS output is identical to what the
    // `record` subcommand would have written.
    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let ct_files = ct_files_in(&out_dir);
    assert!(
        !ct_files.is_empty(),
        "expected a .ct container in {:?}",
        out_dir
    );

    // -----------------------------------------------------------------
    // Layer 1 (legacy): ct-print --json — substring presence checks.
    // Kept as a safety net so a regression in the textual rendering
    // is caught even if --full's JSON shape evolves.
    // -----------------------------------------------------------------
    let output = Command::new(&ct_print)
        .args(["--json"])
        .arg(&ct_files[0])
        .output()
        .expect("failed to run ct-print");

    assert!(
        output.status.success(),
        "ct-print --json should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout_json = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout_json.is_empty(),
        "ct-print --json produced empty output"
    );

    // Source path, function name, every let-binding name must surface.
    assert!(
        stdout_json.contains("flow_test.cdc"),
        "ct-print --json output should mention the source file; got:\n{stdout_json}"
    );
    assert!(
        stdout_json.contains("\"compute\""),
        "ct-print --json output should mention the `compute` function; got:\n{stdout_json}"
    );
    for varname in ["a", "b", "sum_val", "doubled", "final_result"] {
        assert!(
            stdout_json.contains(&format!("\"{varname}\"")),
            "ct-print --json output should mention the `{varname}` variable; got:\n{stdout_json}"
        );
    }

    // -----------------------------------------------------------------
    // Layer 2 (the upgrade): ct-print --full — exact decoded values.
    // -----------------------------------------------------------------
    let full_output = Command::new(&ct_print)
        .args(["--full", "--strip-paths"])
        .arg(&ct_files[0])
        .output()
        .expect("failed to run ct-print --full");

    assert!(
        full_output.status.success(),
        "ct-print --full should succeed; stderr: {}",
        String::from_utf8_lossy(&full_output.stderr)
    );

    let doc: serde_json::Value = serde_json::from_slice(&full_output.stdout)
        .expect("ct-print --full should emit valid JSON");

    // ----- Function table: compute() and main() must both appear ------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        functions.iter().any(|f| f.ends_with("compute")),
        "expected `compute` in functions table; got {:?}",
        functions
    );
    assert!(
        functions.iter().any(|f| f.ends_with("main")),
        "expected `main` in functions table; got {:?}",
        functions
    );

    // ----- Path table: the canonical fixture path must appear ---------
    let paths: Vec<&str> = doc["paths"]
        .as_array()
        .expect("paths array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("flow_test.cdc")),
        "expected flow_test.cdc in paths table; got {:?}",
        paths
    );

    // ----- Step / call counts ----------------------------------------
    // The canonical NDJSON drives 8 step events (one per `step` line in
    // `flow_test_ndjson`, including the entry-point step on line 11 of
    // main and the post-return step on line 7) and 2 call_entry events
    // (main entered first at depth 0, then compute at depth 1).  These
    // are stable properties of the canonical fixture — if they change,
    // that's a real regression to investigate, not a flake.
    let counts = &doc["counts"];
    assert_eq!(
        counts["steps"].as_u64(),
        Some(8),
        "expected 8 step events for flow_test.cdc; counts={counts}",
    );
    assert_eq!(
        counts["calls"].as_u64(),
        Some(2),
        "expected 2 call events (main + compute); counts={counts}",
    );

    let events = doc["events"].as_array().expect("events array");

    // ----- Call sequence: main first, then compute --------------------
    // The recorder emits `call:main` then `call:compute` in the NDJSON
    // event stream (main calls compute), so call_entry events appear in
    // that order in the trace.
    let call_sequence: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .filter_map(|e| e["function"].as_str())
        .collect();
    assert_eq!(
        call_sequence.len(),
        2,
        "expected exactly 2 call_entry events; got {:?}",
        call_sequence
    );
    assert!(
        call_sequence[0].ends_with("main"),
        "expected first call to be `main`; got {:?}",
        call_sequence
    );
    assert!(
        call_sequence[1].ends_with("compute"),
        "expected second call to be `compute`; got {:?}",
        call_sequence
    );

    // ----- Exact decoded variable values ------------------------------
    // Collect every (varname, i64) pair surfaced by step events.  These
    // come from the recorder writing `ValueRecord::Int` CBOR blobs, then
    // ct-print --full decoding them back to `{"kind":"Int","i":<n>,...}`.
    let observed_vars: Vec<(String, i64)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| {
            e["vars"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
        })
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            let value = &v["value"];
            // The flow recorder encodes Cadence `Int` values as
            // ValueRecord::Int.  If something else surfaces (e.g.
            // BigInt for out-of-range Cadence integers, Sequence for
            // arrays, etc.), fail loudly so the test author can decide
            // whether to extend the assertions or accept the new
            // variant — never silently weaken the check.
            assert_eq!(
                value["kind"].as_str(),
                Some("Int"),
                "variable `{}` should decode as Int, got {}; \
                 if a new ValueRecord variant has landed for Cadence \
                 values, extend this test to assert on it explicitly \
                 rather than weakening the check",
                name,
                value
            );
            let i = value["i"]
                .as_i64()
                .unwrap_or_else(|| panic!("Int.i must be i64 for `{name}`; got {value}"));
            Some((name, i))
        })
        .collect();

    // The canonical flow: a=10, b=32, sum_val=a+b=42, doubled=sum_val*2=84,
    // final_result=doubled+a=94.  These mirror the literal values and
    // arithmetic in `test-programs/cadence/flow_test.cdc` lines 2-7.
    let expected: &[(&str, i64)] = &[
        ("a", 10),
        ("b", 32),
        ("sum_val", 42),
        ("doubled", 84),
        ("final_result", 94),
    ];
    for (name, value) in expected {
        assert!(
            observed_vars
                .iter()
                .any(|(n, v)| n == name && v == value),
            "expected step variable `{name}` = {value} in --full output; \
             observed = {observed_vars:?}"
        );
    }
}

// ===========================================================================
// CLI env-var contract
// ===========================================================================

/// `CODETRACER_FLOW_RECORDER_OUT_DIR` must be honoured as a fallback
/// for `--out-dir`.  Convention: `Recorder-CLI-Conventions.md` §5.
///
/// The flow recorder shells out to the Go helper for live recording;
/// to keep this test independent of the helper binary, we set
/// `CODETRACER_FLOW_RECORDER_DISABLED=1` so the recorder validates
/// inputs and resolves the output directory but skips emission.  The
/// post-condition is that the recorder exits 0 — proving it accepted
/// the env-var-supplied output directory without erroring on the
/// missing `--out-dir` flag.
#[test]
fn test_env_out_dir_used_when_flag_omitted() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let env_out_dir = tmp_dir.path().join("via-env");
    let source_path = test_programs_dir().join("flow_test.cdc");

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-flow-recorder"))
        .args(["record"])
        .arg(&source_path)
        .env("CODETRACER_FLOW_RECORDER_OUT_DIR", &env_out_dir)
        // Disable recording so we don't depend on the Go helper.  We're
        // verifying that the env-var fallback is honoured all the way
        // through to a successful exit code; the actual writer
        // invocation is exercised by the NDJSON tests above.
        .env("CODETRACER_FLOW_RECORDER_DISABLED", "1")
        .output()
        .expect("failed to run recorder");

    assert!(
        output.status.success(),
        "recorder should succeed when CODETRACER_FLOW_RECORDER_OUT_DIR is set; \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `CODETRACER_FLOW_RECORDER_DISABLED=1` must skip recording entirely.
/// The recorder process should still exit 0; no `.ct` file should be
/// written.
#[test]
fn test_env_disabled_skips_recording() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("should-stay-empty");
    let source_path = test_programs_dir().join("flow_test.cdc");

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-flow-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .env("CODETRACER_FLOW_RECORDER_DISABLED", "1")
        .output()
        .expect("failed to run recorder");

    assert!(
        output.status.success(),
        "recorder should succeed in disabled mode; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // No .ct file should have been written.
    assert!(
        !out_dir.exists() || ct_files_in(&out_dir).is_empty(),
        "no .ct container should be written when CODETRACER_FLOW_RECORDER_DISABLED=1; \
         got files in {:?}",
        out_dir
    );
}

/// `--format` is no longer accepted at any level — clap must reject it.
/// Convention: §4 (CTFS-only).
#[test]
fn test_format_flag_rejected_by_clap() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-flow-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .args(["--format", "json"])
        .output()
        .expect("failed to run recorder");

    assert!(
        !output.status.success(),
        "--format should be rejected by clap; stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format")
            || stderr.contains("unexpected argument")
            || stderr.contains("unrecognized")
            || stderr.contains("found argument"),
        "clap error should mention the unknown --format flag; got stderr:\n{stderr}"
    );
}

// ===========================================================================
// Go-helper integration tests (require the cadence-trace-helper binary)
// ===========================================================================

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_compile_and_run() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    assert_valid_ct_file(&out_dir);
}

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    assert_valid_ct_file(&out_dir);
}

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    assert_valid_ct_file(&out_dir);
}

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_cli_record() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("cli-traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-flow-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .output()
        .expect("failed to run");

    assert!(
        output.status.success(),
        "record should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_valid_ct_file(&out_dir);
}
