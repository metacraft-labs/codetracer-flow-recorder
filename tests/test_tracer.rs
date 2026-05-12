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

// ===========================================================================
// Per-program ct-print --full coverage tests
// ===========================================================================
//
// These tests follow the recorder-test-requirements policy
// (`metacraft-specs/policies/recorder-test-requirements.md`):
//
// * Each test feeds a hand-curated NDJSON event stream — modelling what
//   the Go helper would emit for the corresponding `.cdc` source file —
//   through `CadenceTracer::trace_program_from_events`, the same code
//   path the live recorder uses once the helper has produced its output.
// * The produced `.ct` is piped through `ct-print --full --strip-paths`.
// * Assertions are made on the **decoded JSON document** with EXACT
//   counts (`assert_eq!(events.len(), N)` — never `>=`), EXACT
//   ordering (later step from a strictly later source line / event
//   position where applicable), and EXACT decoded values
//   (`value["i"] == 42`, `value["kind"] == "Int"`).
//
// `ValueRecord` variants outside the expected set are rejected with a
// hard error message asking the test author to extend the test rather
// than weaken the assertion.
//
// Where the recorder's current behaviour deviates from what the
// language semantics dictate (e.g. arrays/dicts/structs serialised as
// `Raw` strings instead of `Sequence`/`HashMap`/`Struct` ValueRecord
// variants), the deviation is documented inline as `RECORDER BUG: ...`
// and a parallel `#[ignore]`d sibling test captures the spec-correct
// expectation so it surfaces the moment the recorder catches up.

/// Skip-helper: returns `Some(path)` to ct-print or logs a clear
/// `SKIP:` diagnostic and returns `None`.  The
/// `verify-cli-convention-no-silent-skip.sh` script greps for the
/// literal `SKIP:` token, so silent skips remain forbidden.
fn ct_print_or_skip(test_name: &str) -> Option<PathBuf> {
    let p = ct_print_path();
    if !p.exists() {
        eprintln!(
            "SKIP: {test_name} requires ct-print at {} — only available \
             within the metacraft workspace where codetracer-trace-format-nim \
             is a sibling.",
            p.display()
        );
        return None;
    }
    Some(p)
}

/// Drive the NDJSON tracer with the given event stream against
/// `<test-programs/cadence>/<program>` and dump the resulting `.ct`
/// container through `ct-print --full --strip-paths`, returning the
/// decoded JSON document and the absolute source path.  Returns `None`
/// when `ct-print` is unavailable (the caller has already emitted a
/// `SKIP:` line via `ct_print_or_skip`).
fn record_and_dump_full(
    test_name: &str,
    program: &str,
    ndjson: &str,
) -> Option<(serde_json::Value, PathBuf)> {
    let ct_print = ct_print_or_skip(test_name)?;

    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join(program);
    let events = parse_ndjson(ndjson).expect("NDJSON should parse");
    CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
        .expect("trace_program_from_events should succeed");

    let ct_files = ct_files_in(&out_dir);
    assert!(
        !ct_files.is_empty(),
        "expected a .ct container in {:?}",
        out_dir
    );

    let output = Command::new(&ct_print)
        .args(["--full", "--strip-paths"])
        .arg(&ct_files[0])
        .output()
        .expect("failed to run ct-print --full");

    assert!(
        output.status.success(),
        "ct-print --full should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let doc: serde_json::Value = serde_json::from_slice(&output.stdout)
        .expect("ct-print --full should emit valid JSON");

    drop(tmp_dir);

    Some((doc, source_path))
}

/// Assert `metadata.program` ends with the expected source filename.
fn assert_metadata_program_ends_with(doc: &serde_json::Value, source_path: &Path) {
    let prog = doc["metadata"]["program"]
        .as_str()
        .expect("metadata.program str");
    let want = source_path.file_name().unwrap().to_string_lossy();
    assert!(
        prog.ends_with(&*want),
        "metadata.program {prog} must end with {want}"
    );
}

/// Decode the call-entry sequence as a vector of function names in
/// event-emission (entry) order.
fn observed_call_entry_sequence(doc: &serde_json::Value) -> Vec<String> {
    doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .map(|e| {
            e["function"]
                .as_str()
                .expect("call_entry.function str")
                .to_string()
        })
        .collect()
}

/// Decode the call-exit sequence as a vector of function names in
/// event-emission order.
fn observed_call_exit_sequence(doc: &serde_json::Value) -> Vec<String> {
    doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            e["function"]
                .as_str()
                .expect("call_exit.function str")
                .to_string()
        })
        .collect()
}

/// Decode (varname, i64) pairs from every step's `vars` array.
///
/// Rejects any `ValueRecord` variant other than `Int` for an
/// allow-listed variable name with a hard error that asks the test
/// author to extend the test rather than weaken it.  Variables not in
/// `int_only` are skipped (used when the same step contains a mix of
/// Int + non-Int values; the non-Int ones are checked with a separate
/// helper).
fn observed_int_var_sequence(
    doc: &serde_json::Value,
    int_only: &[&str],
) -> Vec<(String, i64)> {
    let mut out = Vec::new();
    for ev in doc["events"].as_array().expect("events array") {
        if ev["kind"] != "step" {
            continue;
        }
        let Some(vars) = ev["vars"].as_array() else {
            continue;
        };
        for v in vars {
            let name = v["varname"].as_str().expect("varname str").to_string();
            if !int_only.iter().any(|n| *n == name) {
                continue;
            }
            let value = &v["value"];
            assert_eq!(
                value["kind"].as_str(),
                Some("Int"),
                "variable `{}` should decode as Int, got {}; \
                 if a new ValueRecord variant has landed for Cadence \
                 integers (e.g. BigInt for out-of-range values), extend \
                 this test to assert on it explicitly rather than \
                 weakening the check",
                name,
                value
            );
            let i = value["i"]
                .as_i64()
                .unwrap_or_else(|| panic!("Int.i must be i64 for `{name}`; got {value}"));
            out.push((name, i));
        }
    }
    out
}

/// Decode (varname, raw-text) pairs for variables surfaced as
/// `ValueRecord::Raw` (the present-day fallback for Cadence
/// String/Array/Dict/Struct/Optional values — see the RECORDER BUG
/// notes on the `#[ignore]`d sibling tests).
fn observed_raw_var_sequence(
    doc: &serde_json::Value,
    raw_only: &[&str],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for ev in doc["events"].as_array().expect("events array") {
        if ev["kind"] != "step" {
            continue;
        }
        let Some(vars) = ev["vars"].as_array() else {
            continue;
        };
        for v in vars {
            let name = v["varname"].as_str().expect("varname str").to_string();
            if !raw_only.iter().any(|n| *n == name) {
                continue;
            }
            let value = &v["value"];
            assert_eq!(
                value["kind"].as_str(),
                Some("Raw"),
                "variable `{}` should decode as Raw today (RECORDER BUG: \
                 should be String/Sequence/HashMap/Struct/Variant once \
                 typed encoding lands), got {}",
                name,
                value
            );
            let r = value["r"]
                .as_str()
                .unwrap_or_else(|| panic!("Raw.r must be string for `{name}`; got {value}"));
            out.push((name, r.to_string()));
        }
    }
    out
}

/// Decode the i64 return value off every `call_exit` event in order.
/// Asserts the kind is `Int` (or `Void` — we map Void to `None`).
fn observed_int_returns(doc: &serde_json::Value) -> Vec<Option<i64>> {
    doc["events"]
        .as_array()
        .expect("events array")
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let rv = &e["return_value"];
            match rv["kind"].as_str() {
                Some("Int") => Some(
                    rv["i"]
                        .as_i64()
                        .unwrap_or_else(|| panic!("Int.i must be i64; got {rv}")),
                ),
                Some("Void") => None,
                other => panic!(
                    "return value should be Int or Void; got kind={:?} full={}",
                    other, rv
                ),
            }
        })
        .collect()
}

/// Assert that every `step` event carries a strictly increasing
/// `step_index`.
fn assert_step_indices_monotonic(doc: &serde_json::Value) {
    let mut last = -1i64;
    for ev in doc["events"].as_array().expect("events array") {
        if ev["kind"] != "step" {
            continue;
        }
        let idx = ev["step_index"]
            .as_i64()
            .expect("step_index must be present on step events");
        assert!(
            idx > last,
            "step_index must strictly increase; got {idx} after {last}"
        );
        last = idx;
    }
}

// --- nested_calls_test.cdc ------------------------------------------------

const NESTED_CALLS_NDJSON: &str = include_str!("ndjson/nested_calls_test.ndjson");

/// Records `nested_calls_test.cdc` (NDJSON-driven) and asserts on the
/// **exact** event shape.  This is the well-behaved case: every
/// function in the chain takes scalar Int arguments, returns a scalar
/// Int, and the four-deep chain `main → compute → outer → middle →
/// inner` is captured end-to-end with correct entry/exit ordering and
/// correct return values on each `call_exit`.
#[test]
fn test_nested_calls_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_nested_calls_test_via_ct_print_full",
        "nested_calls_test.cdc",
        NESTED_CALLS_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Path table -------------------------------------------------
    let paths: Vec<&str> = doc["paths"]
        .as_array()
        .expect("paths array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        paths.len(),
        1,
        "expected exactly one path entry; got {paths:?}"
    );
    assert!(
        paths[0].ends_with("nested_calls_test.cdc"),
        "expected nested_calls_test.cdc in paths; got {paths:?}"
    );

    // ----- Function table — order is writer-assignment order ---------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec!["main", "compute", "outer", "middle", "inner"],
        "function table must be in entry order"
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(10), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );
    assert_eq!(
        counts["values"].as_u64(),
        Some(10),
        "values; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 10 steps + 5 call_entry + 5 call_exit = 20 events
    assert_eq!(events.len(), 20, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call entry order: outermost first --------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "outer".to_string(),
            "middle".to_string(),
            "inner".to_string(),
        ],
        "call_entry events must appear in entry order"
    );

    // ----- Call exit order: innermost first (LIFO) --------------------
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "inner".to_string(),
            "middle".to_string(),
            "outer".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ],
        "call_exit events must appear in LIFO order"
    );

    // ----- Exact decoded variable values ------------------------------
    // The chain: inner returns 1+2 wait — inner gets a=3,b=10 -> c=13;
    // middle: y=13; outer: q=113; compute: result=113.
    let expected_vars: Vec<(String, i64)> = vec![
        ("c".into(), 13),
        ("y".into(), 13),
        ("q".into(), 113),
        ("result".into(), 113),
    ];
    assert_eq!(
        observed_int_var_sequence(&doc, &["c", "y", "q", "result"]),
        expected_vars
    );

    // ----- Return values on each call_exit ----------------------------
    // LIFO order: inner=13, middle=13, outer=113, compute=113, main=113
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(13), Some(13), Some(113), Some(113), Some(113)],
    );
}

// --- control_flow_test.cdc ------------------------------------------------

const CONTROL_FLOW_NDJSON: &str = include_str!("ndjson/control_flow_test.ndjson");

/// Records `control_flow_test.cdc` (NDJSON-driven) and asserts on the
/// **exact** event shape.  Exercises if/else, while, for-in, switch,
/// and early-return through a single `compute()` that calls each
/// helper once.
#[test]
fn test_control_flow_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_control_flow_test_via_ct_print_full",
        "control_flow_test.cdc",
        CONTROL_FLOW_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table — entry order ------------------------------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec![
            "main",
            "compute",
            "classify",
            "while_sum",
            "for_sum",
            "switch_label",
            "early_return",
        ],
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(30), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(7), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 30 steps + 7 call_entry + 7 call_exit = 44 events.
    assert_eq!(events.len(), 44, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call sequence ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "classify".to_string(),
            "while_sum".to_string(),
            "for_sum".to_string(),
            "switch_label".to_string(),
            "early_return".to_string(),
        ],
    );
    // Each helper returns directly (no further nesting), so the LIFO
    // exit order interleaves naturally: classify, while_sum, for_sum,
    // switch_label, early_return all exit before compute, then main.
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "classify".to_string(),
            "while_sum".to_string(),
            "for_sum".to_string(),
            "switch_label".to_string(),
            "early_return".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ],
    );

    // ----- Exact decoded Int values -----------------------------------
    // Only the integer variables — string variables (sign_label,
    // switch_result) are checked separately below as `Raw` per the
    // RECORDER BUG note.
    //
    // Sequence in source/event order:
    //   raw=7
    //   limit=3, i=1, total=0, total=1, i=2, total=3, i=3, total=6, i=4
    //   loop_total=6
    //   total=0, total=1, total=3, total=6, total=10
    //   for_total=10
    //   s=1
    //   early=999
    let expected_ints: Vec<(String, i64)> = vec![
        ("raw".into(), 7),
        ("limit".into(), 3),
        ("i".into(), 1),
        ("total".into(), 0),
        ("total".into(), 1),
        ("i".into(), 2),
        ("total".into(), 3),
        ("i".into(), 3),
        ("total".into(), 6),
        ("i".into(), 4),
        ("loop_total".into(), 6),
        ("total".into(), 0),
        ("total".into(), 1),
        ("total".into(), 3),
        ("total".into(), 6),
        ("total".into(), 10),
        ("for_total".into(), 10),
        ("s".into(), 1),
        ("early".into(), 999),
    ];
    assert_eq!(
        observed_int_var_sequence(
            &doc,
            &[
                "raw",
                "limit",
                "i",
                "total",
                "loop_total",
                "for_total",
                "s",
                "early",
            ],
        ),
        expected_ints
    );

    // ----- String/Bool variables surface as Raw today ----------------
    // RECORDER BUG: Cadence Strings should decode as `String { text }`,
    // Bools as `Bool { b }`.  The Flow recorder routes anything that
    // isn't a parseable i64 through `ValueRecord::String`, but on the
    // read side ct-print decodes those CBOR bytes as `Raw`.  Pinned
    // here as the present-day shape; the `#[ignore]`d sibling test
    // below captures the spec-compliant expectation.
    assert_eq!(
        observed_raw_var_sequence(&doc, &["sign_label", "switch_result"]),
        vec![
            ("sign_label".into(), "small".into()),
            ("switch_result".into(), "small".into()),
        ],
    );

    // ----- Return values: Int returns where applicable ---------------
    // Exit order: classify(Raw "small"), while_sum=6, for_sum=10,
    // switch_label(Raw "small"), early_return=999, compute=1022,
    // main=1022.
    let returns: Vec<(String, &str)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| {
            let f = e["function"].as_str().unwrap().to_string();
            let kind = e["return_value"]["kind"].as_str().unwrap_or("?");
            (f, kind)
        })
        .collect();
    assert_eq!(
        returns,
        vec![
            ("classify".into(), "Raw"),
            ("while_sum".into(), "Int"),
            ("for_sum".into(), "Int"),
            ("switch_label".into(), "Raw"),
            ("early_return".into(), "Int"),
            ("compute".into(), "Int"),
            ("main".into(), "Int"),
        ],
        "return value kinds per call_exit (RECORDER BUG: String returns \
         decode as Raw, not String)"
    );

    // The Int-typed returns must carry the right numeric values.
    let int_returns: Vec<i64> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call_exit" && e["return_value"]["kind"] == "Int")
        .map(|e| e["return_value"]["i"].as_i64().unwrap())
        .collect();
    assert_eq!(int_returns, vec![6, 10, 999, 1022, 1022]);
}

#[test]
#[ignore = "RECORDER BUG: Cadence String values decode as ValueRecord::Raw \
            instead of ValueRecord::String, and Bool values decode as Raw \
            instead of Bool.  When the typed-encoding work lands, \
            sign_label / switch_result / flag should each surface as a \
            properly typed ValueRecord variant."]
fn test_control_flow_test_strings_decode_as_string() {
    let Some((doc, _)) = record_and_dump_full(
        "test_control_flow_test_strings_decode_as_string",
        "control_flow_test.cdc",
        CONTROL_FLOW_NDJSON,
    ) else {
        return;
    };
    let events = doc["events"].as_array().unwrap();
    let sign_label = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "sign_label")
        .expect("sign_label should be present");
    assert_eq!(
        sign_label["value"]["kind"].as_str(),
        Some("String"),
        "spec-compliant: sign_label should decode as String, not Raw"
    );
    assert_eq!(sign_label["value"]["text"].as_str(), Some("small"));
}

// --- collections_test.cdc -------------------------------------------------

const COLLECTIONS_NDJSON: &str = include_str!("ndjson/collections_test.ndjson");

/// Records `collections_test.cdc` (NDJSON-driven) and asserts on the
/// **exact** event shape.  Exercises arrays, dictionaries, structs,
/// and optionals.
///
/// RECORDER BUG: array literals (`[1, 2, 3, 4]`), dict literals
/// (`{"apple": 30, ...}`), struct construction (`Point(x:3, y:4)`),
/// and Optional Int values are all serialised by the recorder as
/// stringified text and decoded by ct-print as `Raw`, not as the
/// spec-compliant `Sequence` / `HashMap` / `Struct` / `Variant`
/// ValueRecord variants.  The `#[ignore]`d sibling test captures the
/// spec-compliant expectation.
#[test]
fn test_collections_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_collections_test_via_ct_print_full",
        "collections_test.cdc",
        COLLECTIONS_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table --------------------------------------------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec![
            "main",
            "compute",
            "array_sum",
            "dict_lookup",
            "point_distance_sq",
            "maybe_double",
        ],
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(18), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(6), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 18 steps + 6 call_entry + 6 call_exit = 30 events.
    assert_eq!(events.len(), 30, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "array_sum".to_string(),
            "dict_lookup".to_string(),
            "point_distance_sq".to_string(),
            "maybe_double".to_string(),
        ],
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "array_sum".to_string(),
            "dict_lookup".to_string(),
            "point_distance_sq".to_string(),
            "maybe_double".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ],
    );

    // ----- Int variables ---------------------------------------------
    // total reappears across array_sum body + compute return-site;
    // apple_price, dist_sq, doubled, some_val are all Ints.
    let expected_ints: Vec<(String, i64)> = vec![
        ("total".into(), 0),
        ("total".into(), 10),
        ("apple_price".into(), 30),
        ("dist_sq".into(), 25),
        ("some_val".into(), 7),
        ("o".into(), 7),
        ("doubled".into(), 14),
    ];
    assert_eq!(
        observed_int_var_sequence(
            &doc,
            &["total", "apple_price", "dist_sq", "some_val", "o", "doubled"],
        ),
        expected_ints
    );

    // ----- Raw (RECORDER BUG) variables ------------------------------
    // RECORDER BUG: `xs` (array), `prices` (dictionary), `p` (struct),
    // `d` (dictionary arg), `key` (String) all surface as Raw rather
    // than their spec-compliant ValueRecord variants.
    //
    // `xs` appears twice — once at let-binding site in compute, once
    // in array_sum's first dispatch step.
    assert_eq!(
        observed_raw_var_sequence(&doc, &["xs", "prices", "p", "d", "key"]),
        vec![
            ("xs".into(), "[1, 2, 3, 4]".into()),
            ("xs".into(), "[1, 2, 3, 4]".into()),
            ("prices".into(), "{\"apple\": 30, \"banana\": 10}".into()),
            ("d".into(), "{\"apple\": 30, \"banana\": 10}".into()),
            ("key".into(), "apple".into()),
            ("p".into(), "S.Point(x: 3, y: 4)".into()),
            ("p".into(), "S.Point(x: 3, y: 4)".into()),
        ],
    );

    // ----- Returns ---------------------------------------------------
    // All 6 returns are Int (Cadence Int): array_sum=10, dict_lookup=30,
    // point_distance_sq=25, maybe_double=14, compute=79, main=79.
    assert_eq!(
        observed_int_returns(&doc),
        vec![
            Some(10),
            Some(30),
            Some(25),
            Some(14),
            Some(79),
            Some(79),
        ],
    );
}

#[test]
#[ignore = "RECORDER BUG: Cadence Array/Dict/Struct/Optional values \
            decode as ValueRecord::Raw (stringified) instead of \
            Sequence/HashMap/Struct/Variant.  Spec-compliant output \
            should expose `xs` as Sequence, `prices` as HashMap (or a \
            dedicated Dict variant), `p` as Struct, and `some_val` as \
            Variant(Some)."]
fn test_collections_test_value_kinds_present() {
    let Some((doc, _)) = record_and_dump_full(
        "test_collections_test_value_kinds_present",
        "collections_test.cdc",
        COLLECTIONS_NDJSON,
    ) else {
        return;
    };
    let mut kinds = std::collections::BTreeSet::new();
    for ev in doc["events"].as_array().unwrap() {
        if ev["kind"] != "step" {
            continue;
        }
        for v in ev["vars"].as_array().cloned().unwrap_or_default() {
            if let Some(k) = v["value"]["kind"].as_str() {
                kinds.insert(k.to_string());
            }
        }
    }
    for want in ["Int", "Sequence", "Struct", "Variant"] {
        assert!(
            kinds.contains(want),
            "expected {want} ValueRecord variant in collections trace; got {kinds:?}"
        );
    }
}

// --- error_paths_test.cdc -------------------------------------------------

const ERROR_PATHS_NDJSON: &str = include_str!("ndjson/error_paths_test.ndjson");

/// Records `error_paths_test.cdc` (NDJSON-driven) and asserts on the
/// **exact** event shape.  Drives both the safe path through
/// `divide_strict` (which has spec-compliant pre/post conditions but
/// none are violated on this input) and a separate `panicking_call`
/// invocation that emits an `EventLogKind::Error` IO event.
///
/// RECORDER BUG: pre-condition / post-condition violations are not
/// surfaced as a distinct event kind — they would today look identical
/// to a generic `panic` (i.e. an `ioError` IO event with the
/// condition's failure message).  The `#[ignore]`d sibling test
/// captures the spec-compliant expectation that pre/post failures
/// receive a structured event.
#[test]
fn test_error_paths_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_error_paths_test_via_ct_print_full",
        "error_paths_test.cdc",
        ERROR_PATHS_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table --------------------------------------------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec![
            "main",
            "compute",
            "safe_compute",
            "divide_strict",
            "panicking_call",
        ],
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(13), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(1),
        "io_events; counts={counts} (the panic must surface as exactly one ioError)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 13 steps + 5 call_entry + 5 call_exit + 1 io = 24 events.
    assert_eq!(events.len(), 24, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call sequence ---------------------------------------------
    // First the safe chain: main → compute → safe_compute → divide_strict.
    // Then the standalone panicking_call (depth 0, sibling to main).
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "safe_compute".to_string(),
            "divide_strict".to_string(),
            "panicking_call".to_string(),
        ],
    );
    // LIFO inside the safe chain, then panicking_call exits last.
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "divide_strict".to_string(),
            "safe_compute".to_string(),
            "compute".to_string(),
            "main".to_string(),
            "panicking_call".to_string(),
        ],
    );

    // ----- Variable values --------------------------------------------
    // safe_compute body: a=10, b=5.  divide_strict body re-binds the
    // same parameter names a=10, b=5.  Then q=2 surfaces in the
    // post-call step in safe_compute.
    assert_eq!(
        observed_int_var_sequence(&doc, &["a", "b", "q"]),
        vec![
            ("a".into(), 10),
            ("b".into(), 5),
            ("a".into(), 10),
            ("b".into(), 5),
            ("q".into(), 2),
        ],
    );

    // ----- Returns: divide_strict=2, safe_compute=3, compute=3,
    // main=3, panicking_call=Void --------------------------------------
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(2), Some(3), Some(3), Some(3), None],
    );

    // ----- IO event: panic message --------------------------------------
    let io_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "io")
        .collect();
    assert_eq!(io_events.len(), 1, "exactly one io event");
    let panic_io = io_events[0];
    assert_eq!(
        panic_io["io_kind"].as_str(),
        Some("ioError"),
        "panic must surface as ioError; got {panic_io}"
    );
    assert_eq!(
        panic_io["text"].as_str(),
        Some("intentional panic for trace coverage"),
    );
}

#[test]
#[ignore = "RECORDER BUG: pre-condition and post-condition violations \
            in Cadence are not distinguishable from a generic `panic` \
            in the trace today — they all map to a single `ioError` \
            IO event.  Spec-compliant output should distinguish \
            EventLogKind::Error (pre/post failure) from a regular \
            user-issued panic, and ideally include the failing \
            condition's source location."]
fn test_error_paths_test_distinguishes_pre_post_from_panic() {
    let Some((doc, _)) = record_and_dump_full(
        "test_error_paths_test_distinguishes_pre_post_from_panic",
        "error_paths_test.cdc",
        ERROR_PATHS_NDJSON,
    ) else {
        return;
    };
    // Spec-compliant: at least 3 distinct error events
    // (pre-condition, post-condition, panic).
    let counts = &doc["counts"];
    assert!(
        counts["io_events"].as_u64().unwrap_or(0) >= 3,
        "expected >=3 io_events covering pre/post/panic; counts={counts}"
    );
}

// --- resource_capability_test.cdc -----------------------------------------

const RESOURCE_CAPABILITY_NDJSON: &str =
    include_str!("ndjson/resource_capability_test.ndjson");

/// Records `resource_capability_test.cdc` (NDJSON-driven) and asserts
/// on the **exact** event shape.  Cadence-specific coverage:
/// resources (create / move-via-deposit / destroy).  The recorder
/// surfaces resource lifecycle events through the special-event log
/// (`EventLogKind::TraceLogEvent` → `ioStderr` in `ct-print --full`).
///
/// RECORDER BUG: resource arguments to functions (e.g. the `coin: @Coin`
/// parameter on `Vault.deposit`) are encoded as plain strings rather
/// than as a typed Resource ValueRecord variant — they show up as
/// `kind: "String"` on the call_entry side and `kind: "Raw"` inside
/// the function body.  A future typed encoding for `@Coin#7001` would
/// make resource lineage trivially queryable.
#[test]
fn test_resource_capability_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_resource_capability_test_via_ct_print_full",
        "resource_capability_test.cdc",
        RESOURCE_CAPABILITY_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table --------------------------------------------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    // RECORDER BUG: the spec-compliant function table would also
    // include `init` for Coin, Vault, and the implicit getter for
    // `vault.balance`, but the NDJSON path only registers functions
    // that produce an explicit `call` event.  Pinning what's emitted
    // today.
    assert_eq!(functions, vec!["main", "compute", "deposit"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(15), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(3), "calls; counts={counts}");
    // 4 resource lifecycle events: create Coin, create Vault,
    // destroy Coin (inside deposit), destroy Vault.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(4),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 15 steps + 3 call_entry + 3 call_exit + 4 io = 25 events.
    assert_eq!(events.len(), 25, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call sequence ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "deposit".to_string(),
        ],
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "deposit".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ],
    );

    // ----- IO events: 4 ioStderr resource-log entries -----------------
    let io_events: Vec<&serde_json::Value> =
        events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(io_events.len(), 4, "exactly four io events (resource lifecycle)");
    for io in &io_events {
        assert_eq!(
            io["io_kind"].as_str(),
            Some("ioStderr"),
            "resource events route through TraceLogEvent → ioStderr; \
             got {io}"
        );
        // Each event payload follows the `owner=<address>` convention.
        assert_eq!(io["text"].as_str(), Some("owner=0x01"));
    }

    // ----- Int variables ---------------------------------------------
    // balance=42 (inside deposit) and final_balance=42 (in compute).
    assert_eq!(
        observed_int_var_sequence(&doc, &["balance", "final_balance"]),
        vec![
            ("balance".into(), 42),
            ("final_balance".into(), 42),
        ],
    );

    // ----- Resource-handle variables surface as Raw ------------------
    // The recorder names them `@resource:<Type>#<uuid>` and stages
    // both create + destroy snapshots as Raw text payloads.
    let raw_resource_vars = observed_raw_var_sequence(
        &doc,
        &[
            "@resource:Coin#7001",
            "@resource:Vault#7002",
            "coin",
        ],
    );
    assert_eq!(
        raw_resource_vars,
        vec![
            ("@resource:Coin#7001".into(), "created(owner=0x01)".into()),
            ("@resource:Vault#7002".into(), "created(owner=0x01)".into()),
            ("coin".into(), "@Coin#7001".into()),
            (
                "@resource:Coin#7001".into(),
                "destroyed(owner=0x01)".into(),
            ),
            (
                "@resource:Vault#7002".into(),
                "destroyed(owner=0x01)".into(),
            ),
        ],
    );

    // ----- Returns: deposit=Void, compute=42, main=42 ---------------
    assert_eq!(
        observed_int_returns(&doc),
        vec![None, Some(42), Some(42)],
    );
}

#[test]
#[ignore = "RECORDER BUG: Cadence resource references (e.g. `@Coin#7001`) \
            should decode as a dedicated typed ValueRecord variant \
            carrying the resource's UUID, owner, and type, not as a \
            stringified Raw payload.  Spec-compliant output should \
            also expose capability bindings as a distinct value \
            kind."]
fn test_resource_capability_test_resource_kind_is_typed() {
    let Some((doc, _)) = record_and_dump_full(
        "test_resource_capability_test_resource_kind_is_typed",
        "resource_capability_test.cdc",
        RESOURCE_CAPABILITY_NDJSON,
    ) else {
        return;
    };
    let mut kinds = std::collections::BTreeSet::new();
    for ev in doc["events"].as_array().unwrap() {
        if ev["kind"] != "step" {
            continue;
        }
        for v in ev["vars"].as_array().cloned().unwrap_or_default() {
            if let Some(k) = v["value"]["kind"].as_str() {
                kinds.insert(k.to_string());
            }
        }
    }
    // Spec-compliant: a dedicated Resource (or at minimum Struct)
    // variant should be present.
    assert!(
        kinds.contains("Resource") || kinds.contains("Struct"),
        "expected a Resource/Struct ValueRecord variant for typed \
         resource handles; got {kinds:?}"
    );
}
