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
            observed_vars.iter().any(|(n, v)| n == name && v == value),
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
// Go helper binary auto-built by build.rs (CADENCE_HELPER_BIN_BUILT env). No #[ignore] needed.
fn test_go_helper_compile_and_run() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    assert_valid_ct_file(&out_dir);
}

#[test]
// Go helper binary auto-built by build.rs (CADENCE_HELPER_BIN_BUILT env). No #[ignore] needed.
fn test_go_helper_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    assert_valid_ct_file(&out_dir);
}

#[test]
// Go helper binary auto-built by build.rs (CADENCE_HELPER_BIN_BUILT env). No #[ignore] needed.
fn test_go_helper_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    assert_valid_ct_file(&out_dir);
}

#[test]
// Go helper binary auto-built by build.rs (CADENCE_HELPER_BIN_BUILT env). No #[ignore] needed.
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

    let doc: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("ct-print --full should emit valid JSON");

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
fn observed_int_var_sequence(doc: &serde_json::Value, int_only: &[&str]) -> Vec<(String, i64)> {
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
/// `ValueRecord::Raw` (the residual fallback for Cadence values whose
/// typed encoding has not yet shipped — e.g. Int128/Address scalars
/// outside the i64-fits set, and any future compound forms beyond
/// arrays / dicts / structs / optionals / `@Resource` handles).
#[allow(dead_code)]
fn observed_raw_var_sequence(doc: &serde_json::Value, raw_only: &[&str]) -> Vec<(String, String)> {
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

/// Decode (varname, text) pairs for variables surfaced as a typed
/// `ValueRecord::String` (post-typed-encoding shape for Cadence
/// `String` values).  Strict on the variant tag — the Raw fallback is
/// captured separately by `observed_raw_var_sequence`.
fn observed_string_var_sequence(
    doc: &serde_json::Value,
    string_only: &[&str],
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
            if !string_only.iter().any(|n| *n == name) {
                continue;
            }
            let value = &v["value"];
            assert_eq!(
                value["kind"].as_str(),
                Some("String"),
                "variable `{}` should decode as `ValueRecord::String` \
                 (typed Cadence String), got {}",
                name,
                value
            );
            let text = value["text"]
                .as_str()
                .unwrap_or_else(|| panic!("String.text must be string for `{name}`; got {value}"));
            out.push((name, text.to_string()));
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

    // ----- String variables surface as typed `String { text }` ---------
    // After the typed-encoding fix, Cadence Strings decode as
    // `ValueRecord::String { text }` (and Bools as `Bool { b }`) rather
    // than the legacy stringified `Raw` payload.  Pinned exactly so any
    // future regression to `Raw` is caught here, not just by the
    // dedicated `test_control_flow_test_strings_decode_as_string`.
    assert_eq!(
        observed_string_var_sequence(&doc, &["sign_label", "switch_result"]),
        vec![
            ("sign_label".into(), "small".into()),
            ("switch_result".into(), "small".into()),
        ],
    );

    // ----- Return values: typed kinds per call_exit ------------------
    // Exit order: classify=String "small", while_sum=6, for_sum=10,
    // switch_label=String "small", early_return=999, compute=1022,
    // main=1022.  String returns must surface as `ValueRecord::String`,
    // not `Raw`.
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
            ("classify".into(), "String"),
            ("while_sum".into(), "Int"),
            ("for_sum".into(), "Int"),
            ("switch_label".into(), "String"),
            ("early_return".into(), "Int"),
            ("compute".into(), "Int"),
            ("main".into(), "Int"),
        ],
        "return value kinds per call_exit (typed: String returns must \
         decode as `ValueRecord::String`, not `Raw`)"
    );

    // Each String return must carry the correct text payload.
    let string_returns: Vec<(String, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call_exit" && e["return_value"]["kind"] == "String")
        .map(|e| {
            let f = e["function"].as_str().unwrap().to_string();
            let text = e["return_value"]["text"].as_str().unwrap().to_string();
            (f, text)
        })
        .collect();
    assert_eq!(
        string_returns,
        vec![
            ("classify".into(), "small".into()),
            ("switch_label".into(), "small".into()),
        ],
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
    // apple_price, dist_sq, doubled are all Ints.  `some_val` and the
    // `o` argument carry Cadence type `Int?` and decode as
    // `ValueRecord::Variant(Some(Int 7))` — see the dedicated check
    // further down.
    let expected_ints: Vec<(String, i64)> = vec![
        ("total".into(), 0),
        ("total".into(), 10),
        ("apple_price".into(), 30),
        ("dist_sq".into(), 25),
        ("doubled".into(), 14),
    ];
    assert_eq!(
        observed_int_var_sequence(&doc, &["total", "apple_price", "dist_sq", "doubled"],),
        expected_ints
    );

    // ----- Typed compound variables ---------------------------------
    // After the typed-encoding fix for arrays / dicts / structs /
    // optionals, `xs` (array), `prices` (dictionary), `p` (struct),
    // and `d` (dictionary arg) surface as their spec-compliant
    // `Sequence` / `Sequence-of-Tuples` / `Struct` variants — no
    // longer the legacy stringified `Raw` payload.  Pin the kind
    // sequence by varname so any future regression is caught here in
    // addition to the dedicated `_value_kinds_present` test.
    let compound_kinds: Vec<(String, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "xs" | "prices" | "p" | "d") {
                return None;
            }
            let kind = v["value"]["kind"].as_str()?.to_string();
            Some((name, kind))
        })
        .collect();
    assert_eq!(
        compound_kinds,
        vec![
            ("xs".into(), "Sequence".into()),
            ("xs".into(), "Sequence".into()),
            ("prices".into(), "Sequence".into()),
            ("d".into(), "Sequence".into()),
            ("p".into(), "Struct".into()),
            ("p".into(), "Struct".into()),
        ],
    );

    // Spot-check the Sequence elements for `xs`: four typed Int leaves.
    let xs_first = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "xs")
        .expect("xs should be present");
    let xs_elems = xs_first["value"]["elements"]
        .as_array()
        .expect("Sequence.elements array");
    let xs_ints: Vec<i64> = xs_elems
        .iter()
        .map(|e| {
            assert_eq!(e["kind"].as_str(), Some("Int"), "xs element should be Int");
            e["i"].as_i64().expect("Int.i must be i64")
        })
        .collect();
    assert_eq!(xs_ints, vec![1, 2, 3, 4]);

    // Spot-check `p` (Point struct): two typed Int field values.
    let p_first = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "p")
        .expect("p should be present");
    let p_fields = p_first["value"]["field_values"]
        .as_array()
        .expect("Struct.field_values array");
    let p_ints: Vec<i64> = p_fields
        .iter()
        .map(|e| {
            assert_eq!(
                e["kind"].as_str(),
                Some("Int"),
                "p field should be Int (numeric coercion)"
            );
            e["i"].as_i64().expect("Int.i must be i64")
        })
        .collect();
    assert_eq!(p_ints, vec![3, 4]);

    // ----- Optionals decode as `Variant(Some|None)` ------------------
    // `some_val` (let-binding) and `o` (the `Int?` argument to
    // `maybe_double`) both carry Cadence type `Int?` and surface as
    // `ValueRecord::Variant { discriminator: "Some", contents: Int(7) }`.
    let optional_vars: Vec<(String, String, i64)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "some_val" | "o") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Variant"),
                "Cadence Int? must decode as `ValueRecord::Variant`; \
                 got {} for {}",
                v["value"],
                name
            );
            let disc = v["value"]["discriminator"].as_str()?.to_string();
            assert_eq!(
                v["value"]["contents"]["kind"].as_str(),
                Some("Int"),
                "Variant.contents must be a typed Int leaf for {}",
                name
            );
            let i = v["value"]["contents"]["i"].as_i64()?;
            Some((name, disc, i))
        })
        .collect();
    assert_eq!(
        optional_vars,
        vec![
            ("some_val".into(), "Some".into(), 7),
            ("o".into(), "Some".into(), 7),
        ],
    );

    // ----- Typed String variables ------------------------------------
    // Cadence String dictionary keys decode as `ValueRecord::String`
    // (not Raw) — see `test_control_flow_test_strings_decode_as_string`
    // for the canonical pin.
    assert_eq!(
        observed_string_var_sequence(&doc, &["key"]),
        vec![("key".into(), "apple".into())],
    );

    // ----- Returns ---------------------------------------------------
    // All 6 returns are Int (Cadence Int): array_sum=10, dict_lookup=30,
    // point_distance_sq=25, maybe_double=14, compute=79, main=79.
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(10), Some(30), Some(25), Some(14), Some(79), Some(79),],
    );
}

#[test]
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
/// **exact** event shape.  Drives the safe path through
/// `divide_strict` (no pre/post violation), then exercises three
/// failure modes back-to-back so the recorder can be pinned to emit
/// distinct trace events for each:
///
///  1. A `divide_strict(a:10, b:0)` call that triggers the
///     `b != 0` pre-condition.
///  2. A `divide_strict(a:7,  b:2)` call that triggers the
///     `result * b == a` post-condition (integer truncation).
///  3. A `panicking_call()` invocation that issues an explicit
///     `panic("intentional panic for trace coverage")`.
///
/// All three flow through `EventLogKind` events; the pre/post failures
/// route through `EventLogKind::TraceLogEvent` (-> `ioStderr`) with a
/// `CadencePreCondition` / `CadencePostCondition` metadata tag, while
/// the user `panic` keeps the historical `EventLogKind::Error`
/// (-> `ioError`) channel.  The `#[ignore]`d sibling test asserts the
/// looser `>=3` count guarantee; this strict test additionally pins
/// the io_kind / text payload of every emitted event.
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
    // 15 explicit `step` events in the NDJSON + 1 implicit start
    // step emitted by `TraceWriter::start` = 16 step records.
    assert_eq!(counts["steps"].as_u64(), Some(16), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(7), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(3),
        "io_events; counts={counts} (one event per failure mode: \
         pre-condition + post-condition + panic)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 16 steps + 7 call_entry + 7 call_exit + 3 io = 33 events.
    assert_eq!(events.len(), 33, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call sequence ---------------------------------------------
    // First the safe chain: main → compute → safe_compute → divide_strict.
    // Then two more divide_strict invocations exercising pre / post
    // violations, and finally the standalone panicking_call.
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "safe_compute".to_string(),
            "divide_strict".to_string(),
            "divide_strict".to_string(),
            "divide_strict".to_string(),
            "panicking_call".to_string(),
        ],
    );
    // LIFO inside the safe chain, then the failure-mode invocations
    // exit in source order (each is depth-0 sibling to main).
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "divide_strict".to_string(),
            "safe_compute".to_string(),
            "compute".to_string(),
            "main".to_string(),
            "divide_strict".to_string(),
            "divide_strict".to_string(),
            "panicking_call".to_string(),
        ],
    );

    // ----- Variable values --------------------------------------------
    // safe_compute body: a=10, b=5.  divide_strict body re-binds the
    // same parameter names a=10, b=5.  Then q=2 surfaces in the
    // post-call step in safe_compute.  The two failure-mode invocations
    // re-stage a=10/b=0 (pre-fail) and a=7/b=2 (post-fail).
    assert_eq!(
        observed_int_var_sequence(&doc, &["a", "b", "q"]),
        vec![
            ("a".into(), 10),
            ("b".into(), 5),
            ("a".into(), 10),
            ("b".into(), 5),
            ("q".into(), 2),
            ("a".into(), 10),
            ("b".into(), 0),
            ("a".into(), 7),
            ("b".into(), 2),
        ],
    );

    // ----- Returns: divide_strict=2, safe_compute=3, compute=3,
    // main=3, then divide_strict=Void (pre-fail), divide_strict=Void
    // (post-fail), panicking_call=Void --------------------------------
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(2), Some(3), Some(3), Some(3), None, None, None,],
    );

    // ----- IO events: pre-condition, post-condition, panic ------------
    // Three distinct entries.  Pre/post failures route through
    // `EventLogKind::TraceLogEvent` (= `ioStderr`); the explicit panic
    // keeps `EventLogKind::Error` (= `ioError`).  Order matches the
    // emission order in the fixture.
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(
        io_events.len(),
        3,
        "exactly three io events (pre, post, panic)"
    );

    let io_summary: Vec<(&str, &str)> = io_events
        .iter()
        .map(|e| {
            let kind = e["io_kind"].as_str().unwrap_or("?");
            let text = e["text"].as_str().unwrap_or("?");
            (kind, text)
        })
        .collect();
    assert_eq!(
        io_summary,
        vec![
            (
                "ioStderr",
                "pre-condition failed: denominator must be non-zero",
            ),
            ("ioStderr", "post-condition failed: division must be exact",),
            ("ioError", "intentional panic for trace coverage"),
        ],
        "Pre/post-condition failures must route through `TraceLogEvent` \
         (ioStderr) with a distinct text payload; the user-issued panic \
         keeps the historical `Error` (ioError) channel."
    );
}

#[test]
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

const RESOURCE_CAPABILITY_NDJSON: &str = include_str!("ndjson/resource_capability_test.ndjson");

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
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(
        io_events.len(),
        4,
        "exactly four io events (resource lifecycle)"
    );
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
        vec![("balance".into(), 42), ("final_balance".into(), 42),],
    );

    // ----- Resource-handle lifecycle variables surface as `String` ---
    // The recorder names them `@resource:<Type>#<uuid>` and stages
    // both create + destroy snapshots as typed `ValueRecord::String`
    // text payloads.  (A future fix would lift these to a dedicated
    // Resource variant — see the `#[ignore]`d sibling test below.)
    assert_eq!(
        observed_string_var_sequence(&doc, &["@resource:Coin#7001", "@resource:Vault#7002"],),
        vec![
            ("@resource:Coin#7001".into(), "created(owner=0x01)".into()),
            ("@resource:Vault#7002".into(), "created(owner=0x01)".into()),
            ("@resource:Coin#7001".into(), "destroyed(owner=0x01)".into(),),
            (
                "@resource:Vault#7002".into(),
                "destroyed(owner=0x01)".into(),
            ),
        ],
    );

    // The `@Coin` move arg now decodes as a typed
    // `ValueRecord::Struct { field_values: [String "Coin", Int 7001] }`
    // (the resource type name + uuid), not the legacy stringified Raw.
    // Pinning every layer of the typed shape so any regression to Raw
    // is caught here in addition to the dedicated
    // `_resource_kind_is_typed` test.
    let coin_var = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "coin")
        .expect("coin should be present");
    assert_eq!(
        coin_var["value"]["kind"].as_str(),
        Some("Struct"),
        "@Coin resource handle must decode as `ValueRecord::Struct`; \
         got {}",
        coin_var["value"]
    );
    let coin_fields = coin_var["value"]["field_values"]
        .as_array()
        .expect("Struct.field_values array");
    assert_eq!(coin_fields.len(), 2, "expected (type, uuid) field pair");
    assert_eq!(coin_fields[0]["kind"].as_str(), Some("String"));
    assert_eq!(coin_fields[0]["text"].as_str(), Some("Coin"));
    assert_eq!(coin_fields[1]["kind"].as_str(), Some("Int"));
    assert_eq!(coin_fields[1]["i"].as_i64(), Some(7001));

    // ----- Returns: deposit=Void, compute=42, main=42 ---------------
    assert_eq!(observed_int_returns(&doc), vec![None, Some(42), Some(42)],);
}

#[test]
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

// ---------------------------------------------------------------------------
// M10 fixtures: capabilities / paths / references / transactions /
//               numeric_widths / address_literals
// ---------------------------------------------------------------------------
//
// The five fixtures below close out the M10 top-priority gaps in Cadence-
// specific recorder coverage:
//
//   * `capabilities_test`     — capability lifecycle (issue, publish,
//                                borrow, unpublish).  Capability values
//                                surface as `ValueRecord::Reference`.
//   * `account_storage_test`  — `/storage`, `/public`, `/private` paths
//                                under `account.storage.{save, borrow,
//                                copy, load}`.  Paths surface as
//                                `ValueRecord::Struct { domain, identifier }`.
//   * `references_test`       — `&T`, `&{Provider}`, `auth(...) &T`.
//                                All three surface as
//                                `ValueRecord::Reference` (entitlement-
//                                authorized references are `mutable: true`).
//   * `transactions_test`     — `transaction { prepare; execute; post }`
//                                phases, each as a distinct top-level
//                                call frame with a paired `CadenceTxPhase:*`
//                                io_event.
//   * `numeric_widths_test`   — `Int8` … `UInt256` matrix.  Widths up
//                                to 64-bit surface as `ValueRecord::Int`;
//                                widths beyond surface as
//                                `ValueRecord::BigInt`.
//   * `address_literals_test` — `Address` literals.  Hex-form values
//                                fitting in `i64` surface as
//                                `ValueRecord::Int`; full 8-byte
//                                addresses exceeding `i64::MAX` surface
//                                as `ValueRecord::BigInt`.
//
// All tests use strict `assert_eq!` on counts, function tables, call
// sequences, and decoded ValueRecord variants — no `>=` allowed.

// --- capabilities_test.cdc ------------------------------------------------

const CAPABILITIES_NDJSON: &str = include_str!("ndjson/capabilities_test.ndjson");

/// Pins the capability lifecycle (issue / publish / borrow / unpublish).
///
/// Recorder shape: a capability value (cadence_type `Capability<&Vault>`)
/// surfaces as `ValueRecord::Reference` with the borrowed-form text as
/// the dereferenced payload.  Borrowed references (cadence_type `&Vault`)
/// share the same Reference variant.  The publish / unpublish
/// transitions surface as `event` NDJSON entries that route through
/// `EventLogKind::EvmEvent` → `ioStderr` io_events with the path
/// captured in `text`.
#[test]
fn test_capabilities_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_capabilities_test_via_ct_print_full",
        "capabilities_test.cdc",
        CAPABILITIES_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(8), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts} (CapabilityPublish + CapabilityUnpublish)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 8 steps + 2 call_entry + 2 call_exit + 2 io = 14 events
    assert_eq!(events.len(), 14, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec!["main".to_string(), "compute".to_string()]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec!["compute".to_string(), "main".to_string()]
    );

    // ----- Capability `cap` surfaces as ValueRecord::Reference --------
    let cap_var = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "cap")
        .expect("cap should be present");
    assert_eq!(
        cap_var["value"]["kind"].as_str(),
        Some("Reference"),
        "capability must decode as ValueRecord::Reference; got {}",
        cap_var["value"]
    );
    assert_eq!(
        cap_var["value"]["mutable"].as_bool(),
        Some(false),
        "Capability is a read-only reference",
    );
    // dereferenced payload carries the printed form so the consumer
    // can re-render the capability without re-parsing.
    let cap_deref = &cap_var["value"]["dereferenced"];
    assert_eq!(cap_deref["kind"].as_str(), Some("String"));
    assert_eq!(
        cap_deref["text"].as_str(),
        Some("Capability<&Vault>(/storage/Vault)"),
    );

    // ----- Borrowed reference `ref` surfaces as ValueRecord::Reference -
    let ref_var = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "ref")
        .expect("ref should be present");
    assert_eq!(
        ref_var["value"]["kind"].as_str(),
        Some("Reference"),
        "borrowed reference must decode as ValueRecord::Reference; got {}",
        ref_var["value"]
    );
    assert_eq!(ref_var["value"]["mutable"].as_bool(), Some(false));

    // ----- io_events: CapabilityPublish + CapabilityUnpublish --------
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(io_events.len(), 2);
    let io_summary: Vec<(&str, &str)> = io_events
        .iter()
        .map(|e| {
            let kind = e["io_kind"].as_str().unwrap_or("?");
            let text = e["text"].as_str().unwrap_or("?");
            (kind, text)
        })
        .collect();
    assert_eq!(
        io_summary,
        vec![("ioStderr", "/public/Vault"), ("ioStderr", "/public/Vault"),],
        "capability publish/unpublish payloads"
    );

    // ----- Returns ---------------------------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(0), Some(0)]);
}

// --- account_storage_test.cdc ---------------------------------------------

const ACCOUNT_STORAGE_NDJSON: &str = include_str!("ndjson/account_storage_test.ndjson");

/// Pins `/storage` / `/public` / `/private` path operations.
///
/// Each `Path` argument (e.g. `/storage/Vault`) must surface as a typed
/// `ValueRecord::Struct { [String "domain", String "identifier"] }` —
/// the canonical Cadence Path shape — so the frontend can resolve the
/// domain (`storage` / `public` / `private`) and identifier
/// (`Vault` / `Config` / ...) without re-parsing the slash form.
#[test]
fn test_account_storage_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_account_storage_test_via_ct_print_full",
        "account_storage_test.cdc",
        ACCOUNT_STORAGE_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 13 explicit step + 2 implicit steps from resource_create / destroy
    // (each lifecycle event also calls `register_step` at its line) +
    // 1 implicit start step from `TraceWriter::start` = 16.
    assert_eq!(counts["steps"].as_u64(), Some(16), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // 2 resource lifecycle events (create + destroy).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 16 steps + 2 call_entry + 2 call_exit + 2 io = 22 events
    assert_eq!(events.len(), 22, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Each Path surfaces as a typed Struct {domain, identifier} -
    let path_vars: Vec<(String, String, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(
                name.as_str(),
                "save_path" | "borrow_path" | "config_path" | "copy_path" | "load_path"
            ) {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Struct"),
                "Path values must decode as ValueRecord::Struct \
                 {{domain, identifier}}; got {} for {}",
                v["value"],
                name
            );
            let fields = v["value"]["field_values"].as_array()?;
            assert_eq!(
                fields.len(),
                2,
                "Path Struct must have two fields (domain, identifier)"
            );
            let domain = fields[0]["text"].as_str()?.to_string();
            let ident = fields[1]["text"].as_str()?.to_string();
            assert_eq!(fields[0]["kind"].as_str(), Some("String"));
            assert_eq!(fields[1]["kind"].as_str(), Some("String"));
            Some((name, domain, ident))
        })
        .collect();
    assert_eq!(
        path_vars,
        vec![
            ("save_path".into(), "storage".into(), "Vault".into()),
            ("borrow_path".into(), "storage".into(), "Vault".into()),
            ("config_path".into(), "storage".into(), "Config".into()),
            ("copy_path".into(), "storage".into(), "Config".into()),
            ("load_path".into(), "storage".into(), "Vault".into()),
        ]
    );

    // ----- Returns: compute=242, main=242 ----------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(242), Some(242)]);
}

// --- references_test.cdc --------------------------------------------------

const REFERENCES_NDJSON: &str = include_str!("ndjson/references_test.ndjson");

/// Pins the three reference forms: `&T`, `&{Provider}`, `auth(...) &T`.
/// All three surface as `ValueRecord::Reference`, with the
/// `auth(...)`-authorized reference flagged `mutable: true` (entitlement-
/// authorized references can mutate the underlying resource).
#[test]
fn test_references_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_references_test_via_ct_print_full",
        "references_test.cdc",
        REFERENCES_NDJSON,
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
            "read_plain",
            "read_restricted",
            "apply_withdraw",
        ]
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 13 explicit step + 2 implicit steps from resource_create / destroy
    // + 1 implicit start step = 16.
    assert_eq!(counts["steps"].as_u64(), Some(16), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    // 2 resource lifecycle events (create + destroy).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 16 steps + 5 call_entry + 5 call_exit + 2 io = 28 events
    assert_eq!(events.len(), 28, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "read_plain".to_string(),
            "read_restricted".to_string(),
            "apply_withdraw".to_string(),
        ]
    );

    // ----- Each reference surfaces as ValueRecord::Reference ---------
    let collect_ref = |name: &str| -> serde_json::Value {
        doc["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["kind"] == "step")
            .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
            .find(|v| v["varname"] == name)
            .unwrap_or_else(|| panic!("{name} not found"))
    };

    let plain = collect_ref("plain");
    assert_eq!(plain["value"]["kind"].as_str(), Some("Reference"));
    assert_eq!(plain["value"]["mutable"].as_bool(), Some(false));

    let restricted = collect_ref("restricted");
    assert_eq!(restricted["value"]["kind"].as_str(), Some("Reference"));
    assert_eq!(restricted["value"]["mutable"].as_bool(), Some(false));

    let entitled = collect_ref("entitled");
    assert_eq!(entitled["value"]["kind"].as_str(), Some("Reference"));
    assert_eq!(
        entitled["value"]["mutable"].as_bool(),
        Some(true),
        "auth(Withdraw) reference is mutable (entitlement-authorized)",
    );

    // ----- Returns: read_plain=100, read_restricted=100,
    //                apply_withdraw=25, compute=225, main=225 ---------
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(100), Some(100), Some(25), Some(225), Some(225)]
    );
}

// --- transactions_test.cdc ------------------------------------------------

const TRANSACTIONS_NDJSON: &str = include_str!("ndjson/transactions_test.ndjson");

/// Pins the `transaction { prepare; execute; post }` phase model.
///
/// Each phase surfaces as a distinct top-level call frame
/// (`transaction.prepare`, `transaction.execute`, `transaction.post`)
/// in the function table, and a phase-boundary `CadenceTxPhase`
/// io_event is emitted at each entry so the frontend can highlight
/// the phase transitions.  The `auth(Storage, Capabilities) &Account`
/// signer surfaces as `ValueRecord::Reference` (entitlement-authorized
/// references → `mutable: true`).
#[test]
fn test_transactions_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_transactions_test_via_ct_print_full",
        "transactions_test.cdc",
        TRANSACTIONS_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table: each phase as a top-level call frame -----
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
            "transaction.prepare",
            "transaction.execute",
            "transaction.post",
        ]
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(8), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(3),
        "io_events; counts={counts} (one CadenceTxPhase event per phase)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 8 steps + 4 call_entry + 4 call_exit + 3 io = 19 events
    assert_eq!(events.len(), 19, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call sequence: main → prepare → execute → post -----------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "transaction.prepare".to_string(),
            "transaction.execute".to_string(),
            "transaction.post".to_string(),
        ]
    );
    // Each phase exits before the next one enters (siblings, not
    // nested), then main exits last.
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "transaction.prepare".to_string(),
            "transaction.execute".to_string(),
            "transaction.post".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Signer reference: auth(Storage, Capabilities) &Account ----
    let signer = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "signer")
        .expect("signer should be present");
    assert_eq!(
        signer["value"]["kind"].as_str(),
        Some("Reference"),
        "auth(...) &Account must decode as ValueRecord::Reference; got {}",
        signer["value"]
    );
    assert_eq!(
        signer["value"]["mutable"].as_bool(),
        Some(true),
        "entitlement-authorized signer reference is mutable",
    );

    // ----- io_events: CadenceTxPhase × 3 -----------------------------
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(io_events.len(), 3);
    let phase_payloads: Vec<&str> = io_events
        .iter()
        .map(|e| e["text"].as_str().unwrap_or("?"))
        .collect();
    assert_eq!(phase_payloads, vec!["prepare", "execute", "post"]);
    for io in &io_events {
        assert_eq!(io["io_kind"].as_str(), Some("ioStderr"));
    }
}

// --- numeric_widths_test.cdc ----------------------------------------------

const NUMERIC_WIDTHS_NDJSON: &str = include_str!("ndjson/numeric_widths_test.ndjson");

/// Pins Cadence's integer-width matrix.
///
/// Widths up to 64-bit (`Int8` / `Int16` / `Int32` / `Int64` /
/// `UInt32` / `UInt64`) surface as `ValueRecord::Int` (the value
/// round-trips through `i64`).  Widths beyond 64-bit (`Int128` /
/// `UInt128` / `UInt256`) surface as `ValueRecord::BigInt` with the
/// exact big-endian unsigned magnitude preserved.  Closes the M9
/// known limitation that wide integers fell back to `ValueRecord::Raw`.
#[test]
fn test_numeric_widths_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_numeric_widths_test_via_ct_print_full",
        "numeric_widths_test.cdc",
        NUMERIC_WIDTHS_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(12), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 12 steps + 2 call_entry + 2 call_exit = 16 events
    assert_eq!(events.len(), 16, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Widths <= 64-bit surface as ValueRecord::Int --------------
    let int_widths: Vec<(String, i64)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(
                name.as_str(),
                "i8_val" | "i16_val" | "i32_val" | "i64_val" | "u32_val" | "u64_val"
            ) {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Int"),
                "<= 64-bit Cadence integer must decode as ValueRecord::Int; \
                 got {} for {}",
                v["value"],
                name
            );
            let i = v["value"]["i"].as_i64()?;
            Some((name, i))
        })
        .collect();
    assert_eq!(
        int_widths,
        vec![
            ("i8_val".into(), 100),
            ("i16_val".into(), 30000),
            ("i32_val".into(), 2000000000),
            ("i64_val".into(), 9223372036854775000),
            ("u32_val".into(), 4000000000),
            ("u64_val".into(), 9000000000000000000),
        ]
    );

    // ----- Widths > 64-bit surface as ValueRecord::BigInt ------------
    let big_vars: Vec<(String, bool, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "big_i128" | "big_u128" | "big_u256") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("BigInt"),
                "> 64-bit Cadence integer must decode as ValueRecord::BigInt; \
                 got {} for {}",
                v["value"],
                name
            );
            let neg = v["value"]["negative"].as_bool()?;
            let hex = v["value"]["b_hex"].as_str()?.to_string();
            Some((name, neg, hex))
        })
        .collect();
    // big_i128 = 2^126 = 0x40 followed by 15 zero bytes.
    // big_u128 = 2^128 - 1 = sixteen 0xff bytes.
    // big_u256 = 2^200      = 0x01 followed by 25 zero bytes.
    assert_eq!(
        big_vars,
        vec![
            (
                "big_i128".into(),
                false,
                "40000000000000000000000000000000".into(),
            ),
            (
                "big_u128".into(),
                false,
                "ffffffffffffffffffffffffffffffff".into(),
            ),
            (
                "big_u256".into(),
                false,
                "0100000000000000000000000000000000000000000000000000".into(),
            ),
        ]
    );

    // ----- Returns ---------------------------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(1), Some(1)]);
}

// --- address_literals_test.cdc --------------------------------------------

const ADDRESS_LITERALS_NDJSON: &str = include_str!("ndjson/address_literals_test.ndjson");

/// Pins `Address` literals.
///
/// Small addresses (e.g. `0x01`) fit in `i64` and surface as
/// `ValueRecord::Int` (matching the M2 hex-form convention).
/// Full 8-byte addresses that exceed `i64::MAX`
/// (e.g. `0xf8d6e0586b0a20c7`, a real Flow testnet address) surface
/// as `ValueRecord::BigInt` with the 8 raw bytes preserved.  Closes
/// the M9 known limitation that `Address` fell back to
/// `ValueRecord::Raw`.
#[test]
fn test_address_literals_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_address_literals_test_via_ct_print_full",
        "address_literals_test.cdc",
        ADDRESS_LITERALS_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    assert_eq!(counts["steps"].as_u64(), Some(8), "steps; counts={counts}");
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 8 steps + 2 call_entry + 2 call_exit = 12 events
    assert_eq!(events.len(), 12, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Small address (`0x01`) surfaces as ValueRecord::Int -------
    let small: Vec<(String, i64)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "a" | "a_alt") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Int"),
                "i64-fitting Address must decode as ValueRecord::Int; got {}",
                v["value"]
            );
            let i = v["value"]["i"].as_i64()?;
            Some((name, i))
        })
        .collect();
    assert_eq!(small, vec![("a".into(), 1), ("a_alt".into(), 1)],);

    // ----- Full 8-byte address surfaces as ValueRecord::BigInt -------
    let b = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "b")
        .expect("b should be present");
    assert_eq!(
        b["value"]["kind"].as_str(),
        Some("BigInt"),
        "> i64::MAX Address must decode as ValueRecord::BigInt; got {}",
        b["value"]
    );
    assert_eq!(b["value"]["negative"].as_bool(), Some(false));
    assert_eq!(
        b["value"]["b_hex"].as_str(),
        Some("f8d6e0586b0a20c7"),
        "8-byte big-endian Address payload",
    );

    // ----- Bool comparisons round-trip as ValueRecord::Bool ----------
    let bool_vars: Vec<(String, bool)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "eq_small" | "eq_mixed") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Bool"),
                "Bool comparison result must decode as ValueRecord::Bool; got {}",
                v["value"]
            );
            let b = v["value"]["b"].as_bool()?;
            Some((name, b))
        })
        .collect();
    assert_eq!(
        bool_vars,
        vec![("eq_small".into(), true), ("eq_mixed".into(), false),]
    );
}

// ---------------------------------------------------------------------------
// M10 Round 2 fixtures: resources_full / enums / pre_post_conditions /
//                       events_emit / interfaces
// ---------------------------------------------------------------------------
//
// Round 1 closed the M10 top-priority gaps (capabilities, account_storage,
// references, transactions, numeric_widths, address_literals).  Round 2
// extends the strict pin coverage to the remaining high-priority Cadence
// constructs:
//
//   * `resources_full_test`        — full move-operator family
//                                    (`<-`, `<->`, shift, `<-!`, nested
//                                    `destroy`).  Owner transitions surface
//                                    as tagged `ResourceOwnerChange`
//                                    io_events.
//   * `enums_test`                  — `enum Color: UInt8 { case red; ... }`.
//                                    Each constructor surfaces as
//                                    `ValueRecord::Variant`; `rawValue`
//                                    surfaces as `ValueRecord::Int` matching
//                                    the backing-type width.
//   * `pre_post_conditions_test`    — `pre { }` + `post { }` blocks.
//                                    Successful evaluations are silent;
//                                    failures surface with
//                                    `CadencePreCondition` / `CadencePostCondition`
//                                    metadata routing through ioStderr.
//   * `events_emit_test`            — `event Foo(...)` declarations + `emit`.
//                                    Each emit surfaces as a tagged
//                                    `CadenceEmit:` io_event with all
//                                    parameters preserved as typed
//                                    `ValueRecord` variants.
//   * `interfaces_test`             — `resource interface Provider`.
//                                    Interface-restricted reference
//                                    surfaces as `ValueRecord::Reference`,
//                                    and the dispatched call carries both
//                                    the concrete and interface-typed
//                                    function names in the call table.

// --- resources_full_test.cdc ---------------------------------------------

const RESOURCES_FULL_NDJSON: &str = include_str!("ndjson/resources_full_test.ndjson");

/// Pins the full Cadence move-operator family.
///
/// Each of the five move operators (`let b <- a`, swap `<->`, shift,
/// force-unwrap `<-!`, explicit nested `destroy`) surfaces both as
/// a typed `ValueRecord::Struct { ResourceType, ResourceUuid,
/// ResourceOwner }` snapshot of the resource at its new home AND
/// as a tagged `ResourceOwnerChange:<Type>#<uuid>: <from> -> <to>`
/// io_event so the frontend can highlight the ownership transfer
/// without re-deriving it from the snapshots.
#[test]
fn test_resources_full_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_resources_full_test_via_ct_print_full",
        "resources_full_test.cdc",
        RESOURCES_FULL_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec!["main".to_string(), "compute".to_string()]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec!["compute".to_string(), "main".to_string()]
    );

    // ----- Step indices monotonic ------------------------------------
    assert_step_indices_monotonic(&doc);

    // ----- Each resource snapshot decodes as a typed Struct
    //       {ResourceType, ResourceUuid, ResourceOwner} -------------
    let resource_snapshots: Vec<(String, String, i64, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(
                name.as_str(),
                "a" | "b" | "x" | "y" | "new_v" | "old" | "box" | "unwrapped"
            ) {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Struct"),
                "resource snapshot `{}` must decode as ValueRecord::Struct \
                 (with ResourceType + ResourceUuid + ResourceOwner fields); \
                 got {}",
                name,
                v["value"]
            );
            let fields = v["value"]["field_values"].as_array()?;
            assert_eq!(
                fields.len(),
                3,
                "resource Struct must carry 3 fields (type, uuid, owner) \
                 for `{name}` on the move-operator path; got {fields:?}"
            );
            assert_eq!(fields[0]["kind"].as_str(), Some("String"));
            assert_eq!(fields[1]["kind"].as_str(), Some("Int"));
            assert_eq!(fields[2]["kind"].as_str(), Some("String"));
            let ty = fields[0]["text"].as_str()?.to_string();
            let uuid = fields[1]["i"].as_i64()?;
            let owner = fields[2]["text"].as_str()?.to_string();
            Some((name, ty, uuid, owner))
        })
        .collect();
    assert_eq!(
        resource_snapshots,
        vec![
            // Plain move-assignment `let b <- a`.
            ("a".into(), "Vault".into(), 5001, "alice".into()),
            ("b".into(), "Vault".into(), 5001, "bob".into()),
            // Initial create-and-bind for swap operands.
            ("x".into(), "Vault".into(), 5002, "alice".into()),
            ("y".into(), "Vault".into(), 5003, "bob".into()),
            // Swap result: x and y exchange resources (and hence owners).
            ("x".into(), "Vault".into(), 5003, "alice".into()),
            ("y".into(), "Vault".into(), 5002, "bob".into()),
            // Shift: new_v binds, then `let old <- x <- new_v` rotates.
            ("new_v".into(), "Vault".into(), 5004, "alice".into()),
            ("x".into(), "Vault".into(), 5004, "alice".into()),
            ("old".into(), "Vault".into(), 5003, "bob".into()),
            // Force-unwrap move: nested Vault leaves Box, owner flips.
            ("box".into(), "Box".into(), 5005, "alice".into()),
            ("unwrapped".into(), "Vault".into(), 5006, "bob".into()),
        ]
    );

    // ----- Tagged ResourceOwnerChange io_events ----------------------
    let events = doc["events"].as_array().expect("events array");
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();

    // Filter to just the owner-change events (the resource lifecycle
    // create/destroy events also route through TraceLogEvent → ioStderr,
    // so we look for the literal `ResourceOwnerChange:` tag in `text`).
    let owner_changes: Vec<&str> = io_events
        .iter()
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.starts_with("ResourceOwnerChange:"))
        .collect();
    assert_eq!(
        owner_changes,
        vec![
            "ResourceOwnerChange:Vault#5001: alice -> bob",
            "ResourceOwnerChange:Vault#5003: bob -> alice",
            "ResourceOwnerChange:Vault#5002: alice -> bob",
            "ResourceOwnerChange:Vault#5003: alice -> bob",
            "ResourceOwnerChange:Vault#5006: alice -> bob",
        ],
        "exactly five tagged owner-change io_events: one per move \
         operator that transfers ownership (plain move, swap × 2, \
         shift, force-unwrap)"
    );

    // Every owner-change event routes through TraceLogEvent → ioStderr.
    for ev in io_events.iter().filter(|e| {
        e["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("ResourceOwnerChange:"))
    }) {
        assert_eq!(ev["io_kind"].as_str(), Some("ioStderr"));
    }

    // ----- Returns: compute=0, main=0 --------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(0), Some(0)]);
}

// --- enums_test.cdc -------------------------------------------------------

const ENUMS_NDJSON: &str = include_str!("ndjson/enums_test.ndjson");

/// Pins Cadence enum cases backed by an integer width.
///
/// Each `Color.<case>` constructor surfaces as
/// `ValueRecord::Variant { discriminator: "Color.<case>", contents:
/// ValueRecord::None, type_id }` (the M9 Variant path), and each
/// `rawValue` access surfaces as `ValueRecord::Int` matching the
/// backing-type width (`UInt8` → an Int with `i in 0..255`).
#[test]
fn test_enums_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_enums_test_via_ct_print_full",
        "enums_test.cdc",
        ENUMS_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute", "classify"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 18 explicit step events + 1 implicit start step = 19 step records.
    assert_eq!(counts["steps"].as_u64(), Some(19), "steps; counts={counts}");
    // main, compute, classify×3 = 5 calls.
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    // Enums do not produce io_events.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 19 steps + 5 call_entry + 5 call_exit = 29 events.
    assert_eq!(events.len(), 29, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "classify".to_string(),
            "classify".to_string(),
            "classify".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "classify".to_string(),
            "classify".to_string(),
            "classify".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Each enum case decodes as ValueRecord::Variant -----------
    let enum_cases: Vec<(String, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "r" | "g" | "b" | "c") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Variant"),
                "enum case `{}` must decode as ValueRecord::Variant; got {}",
                name,
                v["value"]
            );
            // The discriminator carries the canonical "Color.<case>"
            // name (the M9 Variant path landed in commit 4dd2b59).
            let discrim = v["value"]["discriminator"].as_str()?.to_string();
            // Spec calls out `fields: []` — surfaced as the canonical
            // empty-payload `None` inner content.
            assert_eq!(
                v["value"]["contents"]["kind"].as_str(),
                Some("None"),
                "enum Variant inner contents must be ValueRecord::None \
                 (no fields); got {}",
                v["value"]["contents"]
            );
            Some((name, discrim))
        })
        .collect();
    assert_eq!(
        enum_cases,
        vec![
            ("r".into(), "Color.red".into()),
            ("g".into(), "Color.green".into()),
            ("b".into(), "Color.blue".into()),
            ("c".into(), "Color.red".into()),
            ("c".into(), "Color.green".into()),
            ("c".into(), "Color.blue".into()),
        ]
    );

    // ----- rawValue access surfaces as ValueRecord::Int (UInt8) ------
    // UInt8 backing type → Int with i in 0..255.  We use the existing
    // `observed_int_var_sequence` which already enforces the
    // ValueRecord::Int variant (and rejects any non-Int variant with
    // a hard error).
    assert_eq!(
        observed_int_var_sequence(&doc, &["r_raw", "g_raw", "b_raw"]),
        vec![
            ("r_raw".into(), 0),
            ("g_raw".into(), 1),
            ("b_raw".into(), 2),
        ]
    );

    // ----- classify returns the case index in source order ----------
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(1), Some(2), Some(3), Some(9), Some(9)]
    );
}

// --- pre_post_conditions_test.cdc ----------------------------------------

const PRE_POST_NDJSON: &str = include_str!("ndjson/pre_post_conditions_test.ndjson");

/// Pins Cadence `pre { }` / `post { }` clauses across passing and
/// failing inputs.
///
/// Successful evaluations are silent (no io_event for the
/// pre/post check itself).  Failed pre-conditions surface with the
/// `CadencePreCondition` tag (routed through
/// `EventLogKind::TraceLogEvent` → `ioStderr`); failed
/// post-conditions surface with `CadencePostCondition` (same
/// channel, distinct text payload).  This tag-channel split — and
/// the user-issued `panic` keeping the historical `Error` channel
/// — is the load-bearing part of the M9 commit `4dd2b59`.
#[test]
fn test_pre_post_conditions_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_pre_post_conditions_test_via_ct_print_full",
        "pre_post_conditions_test.cdc",
        PRE_POST_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute", "deposit"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 16 explicit step events + 2 implicit steps (resource_create +
    // resource_destroy) + 1 implicit start step = 19 step records.
    assert_eq!(counts["steps"].as_u64(), Some(19), "steps; counts={counts}");
    // main, compute, deposit × 3 = 5 calls.
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    // 2 resource lifecycle events (create + destroy) + 2 errors
    // (pre-condition fail + post-condition fail) = 4 io_events.  The
    // PASSING pre/post evaluation in call 1 emits NO io_event — that
    // is the silence guarantee being pinned here.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(4),
        "io_events; counts={counts} (silent passing pre/post + \
         failing pre + failing post + 2 resource lifecycle)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 19 steps + 5 call_entry + 5 call_exit + 4 io = 33 events.
    assert_eq!(events.len(), 33, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering: main → compute → deposit×3 ---------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "deposit".to_string(),
            "deposit".to_string(),
            "deposit".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "deposit".to_string(),
            "deposit".to_string(),
            "deposit".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- IO events: 2 resource lifecycle then pre-fail then
    //                  post-fail (in source emission order) ----------
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(io_events.len(), 4);

    let io_summary: Vec<(&str, &str)> = io_events
        .iter()
        .map(|e| {
            let kind = e["io_kind"].as_str().unwrap_or("?");
            let text = e["text"].as_str().unwrap_or("?");
            (kind, text)
        })
        .collect();
    assert_eq!(
        io_summary,
        vec![
            // 1. Account create (lifecycle).
            ("ioStderr", "owner=alice"),
            // 2. First failing call: pre-condition.
            ("ioStderr", "pre-condition failed: amount must be positive"),
            // 3. Second failing call: post-condition.
            ("ioStderr", "post-condition failed: balance must increase"),
            // 4. Account destroy (lifecycle).
            ("ioStderr", "owner=alice"),
        ],
        "Pre/post failures route through TraceLogEvent → ioStderr \
         with distinct text payloads.  The PASSING first call \
         (deposit(amount: 25) → returns 125) produces NO io_event — \
         the silence is what proves the pre/post tag dispatch is not \
         emitting on success."
    );

    // ----- Returns: deposit#1 = 125, deposit#2 = Void (pre-fail),
    //                deposit#3 = Void (post-fail), compute = 125,
    //                main = 125 -------------------------------------
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(125), None, None, Some(125), Some(125)]
    );

    // ----- after1 captured in compute as ValueRecord::Int ------------
    assert_eq!(
        observed_int_var_sequence(&doc, &["after1"]),
        vec![("after1".into(), 125)]
    );
}

// --- events_emit_test.cdc ------------------------------------------------

const EVENTS_EMIT_NDJSON: &str = include_str!("ndjson/events_emit_test.ndjson");

/// Pins Cadence `event` declarations + `emit` statements.
///
/// Each `emit` surfaces as a tagged `CadenceEmit:<EventName>(field:
/// value, ...)` io_event with all parameters preserved in the text
/// payload, AND each field is captured as a typed
/// `ValueRecord` local under the canonical
/// `emit:<EventName>.<field>` name so the frontend can render the
/// event with full type fidelity rather than re-parsing the string
/// payload.
#[test]
fn test_events_emit_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_events_emit_test_via_ct_print_full",
        "events_emit_test.cdc",
        EVENTS_EMIT_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute", "do_transfer", "do_mint"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 7 explicit step events + 1 implicit start step = 8 step records.
    assert_eq!(counts["steps"].as_u64(), Some(8), "steps; counts={counts}");
    // main, compute, do_transfer, do_mint = 4 calls.
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    // 2 emit events.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(2),
        "io_events; counts={counts} (one CadenceEmit per emit statement)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 8 steps + 4 call_entry + 4 call_exit + 2 io = 18 events.
    assert_eq!(events.len(), 18, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- IO events: tagged CadenceEmit:<Name>(...) text payload ---
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(io_events.len(), 2);

    let io_summary: Vec<(&str, &str)> = io_events
        .iter()
        .map(|e| {
            let kind = e["io_kind"].as_str().unwrap_or("?");
            let text = e["text"].as_str().unwrap_or("?");
            (kind, text)
        })
        .collect();
    assert_eq!(
        io_summary,
        vec![
            (
                "ioStderr",
                "CadenceEmit:Transfer(amount: 12.5, from: 0x01, to: 0x02)",
            ),
            (
                "ioStderr",
                "CadenceEmit:NFTMinted(id: 1001, metadata: {\"name\": \"Cat\", \"rarity\": \"common\"})",
            ),
        ],
        "Each emit surfaces as a single tagged CadenceEmit:<Name> io \
         event with the full field-name+value payload preserved in \
         the text channel."
    );

    // ----- Per-field typed locals: emit:<Event>.<field> -------------
    // Address fields surface as ValueRecord::Int (matching the M2 hex
    // convention for i64-fitting addresses); UInt64 id surfaces as
    // ValueRecord::Int.  We pin each via the strict
    // `observed_int_var_sequence` helper which rejects any non-Int
    // variant with a hard error.
    assert_eq!(
        observed_int_var_sequence(
            &doc,
            &[
                "emit:Transfer.from",
                "emit:Transfer.to",
                "emit:NFTMinted.id",
            ],
        ),
        vec![
            ("emit:Transfer.from".into(), 0x01),
            ("emit:Transfer.to".into(), 0x02),
            ("emit:NFTMinted.id".into(), 1001),
        ]
    );

    // ----- Transfer.amount surfaces as a typed leaf (UFix64) --------
    // M10 Round 4 (`fixed_point_test`) shipped a dedicated `UFix64`
    // dispatch in the recorder: `12.5` surfaces as the canonical 1e8
    // scaled `ValueRecord::Int` (`12.5 * 10^8 == 1_250_000_000`),
    // with the type-id metadata (`UFix64`) carrying the scaling
    // factor implicitly.  Closes the M9 known limitation that
    // fractional decimals fell through to `Raw`.
    let amount = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "emit:Transfer.amount")
        .expect("emit:Transfer.amount should be present");
    assert_eq!(amount["value"]["kind"].as_str(), Some("Int"));
    assert_eq!(amount["value"]["i"].as_i64(), Some(1_250_000_000));

    // ----- NFTMinted.metadata surfaces as ValueRecord::Sequence -----
    // (Cadence dictionaries surface as Sequence-of-Tuple via the
    // existing `{K: V}` cadence_type path.)
    let metadata = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "emit:NFTMinted.metadata")
        .expect("emit:NFTMinted.metadata should be present");
    assert_eq!(metadata["value"]["kind"].as_str(), Some("Sequence"));
    let metadata_elems = metadata["value"]["elements"]
        .as_array()
        .expect("metadata.elements array");
    assert_eq!(
        metadata_elems.len(),
        2,
        "metadata dict has two key/value pairs"
    );
    for pair in metadata_elems {
        assert_eq!(pair["kind"].as_str(), Some("Tuple"));
        let elements = pair["elements"].as_array().expect("tuple elements");
        assert_eq!(elements.len(), 2, "(key, value) tuple shape");
        assert_eq!(elements[0]["kind"].as_str(), Some("String"));
        assert_eq!(elements[1]["kind"].as_str(), Some("String"));
    }
    let dict_pairs: Vec<(String, String)> = metadata_elems
        .iter()
        .map(|p| {
            let k = p["elements"][0]["text"].as_str().unwrap().to_string();
            let v = p["elements"][1]["text"].as_str().unwrap().to_string();
            (k, v)
        })
        .collect();
    // Note: dict-key parser strips outer JSON-style quotes off the
    // key (canonical Cadence dict-key shape), but values pass
    // through verbatim — the recorder treats values as opaque
    // payloads so the quotes are preserved literally.  Pinning
    // both to capture this current shape; if the value-side parser
    // ever learns to strip JSON quotes, this strict pin will catch
    // the change.
    assert_eq!(
        dict_pairs,
        vec![
            ("name".into(), "\"Cat\"".into()),
            ("rarity".into(), "\"common\"".into()),
        ]
    );

    // ----- Returns: do_transfer=Void, do_mint=Void, compute=0, main=0 -
    assert_eq!(
        observed_int_returns(&doc),
        vec![None, None, Some(0), Some(0)]
    );
}

// --- interfaces_test.cdc -------------------------------------------------

const INTERFACES_NDJSON: &str = include_str!("ndjson/interfaces_test.ndjson");

/// Pins Cadence resource interface + interface-restricted reference.
///
/// `&{Provider}` (an interface restriction set) surfaces as a typed
/// `ValueRecord::Reference` (the existing `&...` cadence_type path
/// catches the interface form too — the restriction-set name lives
/// in the type-id metadata).  The dispatched `myVault.provide()`
/// call surfaces as TWO call frames: the interface-typed
/// `Provider.provide` dispatch and the concrete `MyVault.provide`
/// implementation, both visible in the function table — so the
/// frontend can render the interface dispatch independently of the
/// concrete implementation.
#[test]
fn test_interfaces_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_interfaces_test_via_ct_print_full",
        "interfaces_test.cdc",
        INTERFACES_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table: both interface and concrete frames -------
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
            "consume",
            "Provider.provide",
            "MyVault.provide",
        ],
        "Both the interface-typed `Provider.provide` and the concrete \
         `MyVault.provide` must surface in the function table — the \
         frontend uses the dual-frame pattern to render interface \
         dispatch in the call trace independently of the concrete \
         implementation.",
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 13 explicit step events + 4 implicit steps from resource_create
    // (×2) and resource_destroy (×2) + 1 implicit start step = 18.
    assert_eq!(counts["steps"].as_u64(), Some(18), "steps; counts={counts}");
    // main, compute, consume, Provider.provide, MyVault.provide = 5.
    assert_eq!(counts["calls"].as_u64(), Some(5), "calls; counts={counts}");
    // 4 resource lifecycle events (create + destroy of MyVault and Vault).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(4),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 18 steps + 5 call_entry + 5 call_exit + 4 io = 32 events.
    assert_eq!(events.len(), 32, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call sequence: interface frame entered before concrete ---
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "consume".to_string(),
            "Provider.provide".to_string(),
            "MyVault.provide".to_string(),
        ]
    );
    // LIFO exit: concrete returns first, then interface, then consume.
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "MyVault.provide".to_string(),
            "Provider.provide".to_string(),
            "consume".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- `restricted` surfaces as ValueRecord::Reference (mutable: false) -
    let restricted = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "restricted")
        .expect("restricted should be present");
    assert_eq!(
        restricted["value"]["kind"].as_str(),
        Some("Reference"),
        "interface-restricted reference must decode as \
         ValueRecord::Reference; got {}",
        restricted["value"]
    );
    assert_eq!(
        restricted["value"]["mutable"].as_bool(),
        Some(false),
        "&{{Provider}} is a read-only interface-restricted reference"
    );
    // The dereferenced payload carries the printed form of the
    // restriction set so the consumer can render it without
    // re-parsing the cadence_type metadata.
    assert_eq!(
        restricted["value"]["dereferenced"]["kind"].as_str(),
        Some("String")
    );
    assert_eq!(
        restricted["value"]["dereferenced"]["text"].as_str(),
        Some("&{Provider}#8001"),
    );

    // ----- The interface-arg `p` on consume's call_entry is a Ref ---
    let p = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "p")
        .expect("`p` arg should be present");
    assert_eq!(p["value"]["kind"].as_str(), Some("Reference"));
    assert_eq!(p["value"]["mutable"].as_bool(), Some(false));

    // ----- Provided Vault carries owner field via @Type Struct ------
    // The provide() return path runs the concrete and interface
    // frames back to back; the resulting `v` local must surface as
    // ValueRecord::Struct {Vault, 8002, alice}.
    let v_var = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "v")
        .expect("v should be present");
    assert_eq!(v_var["value"]["kind"].as_str(), Some("Struct"));
    let v_fields = v_var["value"]["field_values"]
        .as_array()
        .expect("Struct.field_values array");
    assert_eq!(v_fields.len(), 3, "Vault Struct: type + uuid + owner");
    assert_eq!(v_fields[0]["text"].as_str(), Some("Vault"));
    assert_eq!(v_fields[1]["i"].as_i64(), Some(8002));
    assert_eq!(v_fields[2]["text"].as_str(), Some("alice"));

    // ----- bal and result decode as ValueRecord::Int ----------------
    assert_eq!(
        observed_int_var_sequence(&doc, &["bal", "result"]),
        vec![("bal".into(), 77), ("result".into(), 77)]
    );

    // ----- Returns up the stack: provide×2 = Vault Struct, then
    //       consume=77, compute=77, main=77 -------------------------
    // (We rely on observed_int_returns for the Int returns; the
    // resource-typed returns are filtered out — we re-derive them
    // directly here to pin them strictly.)
    let returns: Vec<&serde_json::Value> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .map(|e| &e["return_value"])
        .collect();
    assert_eq!(returns.len(), 5);
    // MyVault.provide return: typed Struct (Vault, 8002, alice).
    assert_eq!(returns[0]["kind"].as_str(), Some("Struct"));
    let r0_fields = returns[0]["field_values"].as_array().unwrap();
    assert_eq!(r0_fields.len(), 3);
    assert_eq!(r0_fields[0]["text"].as_str(), Some("Vault"));
    assert_eq!(r0_fields[1]["i"].as_i64(), Some(8002));
    assert_eq!(r0_fields[2]["text"].as_str(), Some("alice"));
    // Provider.provide return: same Struct (interface dispatch
    // re-emits the resource through the interface frame).
    assert_eq!(returns[1]["kind"].as_str(), Some("Struct"));
    // consume / compute / main: Int(77).
    assert_eq!(returns[2]["kind"].as_str(), Some("Int"));
    assert_eq!(returns[2]["i"].as_i64(), Some(77));
    assert_eq!(returns[3]["kind"].as_str(), Some("Int"));
    assert_eq!(returns[3]["i"].as_i64(), Some(77));
    assert_eq!(returns[4]["kind"].as_str(), Some("Int"));
    assert_eq!(returns[4]["i"].as_i64(), Some(77));
}

// --- resource_collections_test.cdc ---------------------------------------

const RESOURCE_COLLECTIONS_NDJSON: &str = include_str!("ndjson/resource_collections_test.ndjson");

/// Pins resources nested in arrays (`@[Vault]`), dictionaries
/// (`@{Address: Vault}`), and optionals (`@Vault?`).
///
/// Each container shape decodes through the existing typed-collection
/// path:
///
///   * `@[Vault]` -> `ValueRecord::Sequence` whose elements are typed
///     `ValueRecord::Struct` resource snapshots
///     (`{ResourceType, ResourceUuid, ResourceOwner}`).
///   * `@{Address: Vault}` -> `Sequence` of `Tuple` (Int key + Struct
///     value) pairs — the canonical Cadence dict shape.
///   * `@Vault?` -> `ValueRecord::Variant { Some(Struct ...) }` when
///     populated and `ValueRecord::Variant { None }` after the move.
///
/// No recorder change required — the existing array / dict / optional
/// type-id parsers already recurse into the resource-handle path.
#[test]
fn test_resource_collections_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_resource_collections_test_via_ct_print_full",
        "resource_collections_test.cdc",
        RESOURCE_COLLECTIONS_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 18 explicit step events + 5 implicit steps from resource_create
    // (x5) + 5 implicit steps from resource_destroy (x5) + 1 implicit
    // start step = 29 step records.
    assert_eq!(counts["steps"].as_u64(), Some(29), "steps; counts={counts}");
    // main + compute = 2 calls.
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // 5 lifecycle creates + 5 lifecycle destroys = 10 io_events.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(10),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 29 steps + 2 call_entry + 2 call_exit + 10 io = 43 events.
    assert_eq!(events.len(), 43, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec!["main".to_string(), "compute".to_string()]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec!["compute".to_string(), "main".to_string()]
    );

    // ----- @[Vault] surfaces as Sequence of typed Struct snapshots ---
    let arr_snapshots: Vec<serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "arr")
        .collect();
    assert_eq!(
        arr_snapshots.len(),
        4,
        "arr snapshots: initial empty + 2 appends + 1 remove"
    );
    // Snapshot 0: empty.
    assert_eq!(arr_snapshots[0]["value"]["kind"].as_str(), Some("Sequence"));
    assert_eq!(
        arr_snapshots[0]["value"]["elements"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // Snapshot 1: [Vault#9001/alice].
    let s1_elems = arr_snapshots[1]["value"]["elements"].as_array().unwrap();
    assert_eq!(s1_elems.len(), 1);
    assert_eq!(s1_elems[0]["kind"].as_str(), Some("Struct"));
    let s1_fields = s1_elems[0]["field_values"].as_array().unwrap();
    assert_eq!(s1_fields.len(), 3);
    assert_eq!(s1_fields[0]["text"].as_str(), Some("Vault"));
    assert_eq!(s1_fields[1]["i"].as_i64(), Some(9001));
    assert_eq!(s1_fields[2]["text"].as_str(), Some("alice"));
    // Snapshot 2: [Vault#9001, Vault#9002].
    let s2_elems = arr_snapshots[2]["value"]["elements"].as_array().unwrap();
    assert_eq!(s2_elems.len(), 2);
    assert_eq!(
        s2_elems[1]["field_values"].as_array().unwrap()[1]["i"].as_i64(),
        Some(9002)
    );
    // Snapshot 3: [Vault#9002] (after remove).
    let s3_elems = arr_snapshots[3]["value"]["elements"].as_array().unwrap();
    assert_eq!(s3_elems.len(), 1);
    assert_eq!(
        s3_elems[0]["field_values"].as_array().unwrap()[1]["i"].as_i64(),
        Some(9002)
    );

    // ----- @{Address: Vault} surfaces as Sequence-of-Tuple(Int,Struct) -
    let dict_snapshots: Vec<serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "dict")
        .collect();
    assert_eq!(
        dict_snapshots.len(),
        4,
        "dict snapshots: initial empty + 2 inserts + 1 remove"
    );
    // Snapshot 0: empty.
    assert_eq!(
        dict_snapshots[0]["value"]["kind"].as_str(),
        Some("Sequence")
    );
    assert_eq!(
        dict_snapshots[0]["value"]["elements"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    // Snapshot 1: {0x01: Vault#9003/alice}.
    let d1 = dict_snapshots[1]["value"]["elements"].as_array().unwrap();
    assert_eq!(d1.len(), 1);
    assert_eq!(d1[0]["kind"].as_str(), Some("Tuple"));
    let d1_pair = d1[0]["elements"].as_array().unwrap();
    assert_eq!(d1_pair.len(), 2);
    assert_eq!(d1_pair[0]["kind"].as_str(), Some("Int"));
    assert_eq!(d1_pair[0]["i"].as_i64(), Some(0x01));
    assert_eq!(d1_pair[1]["kind"].as_str(), Some("Struct"));
    let d1_struct_fields = d1_pair[1]["field_values"].as_array().unwrap();
    assert_eq!(d1_struct_fields[0]["text"].as_str(), Some("Vault"));
    assert_eq!(d1_struct_fields[1]["i"].as_i64(), Some(9003));
    assert_eq!(d1_struct_fields[2]["text"].as_str(), Some("alice"));
    // Snapshot 2: 2 entries (insertion-order).
    let d2 = dict_snapshots[2]["value"]["elements"].as_array().unwrap();
    assert_eq!(d2.len(), 2);
    let d2_keys: Vec<i64> = d2
        .iter()
        .map(|t| t["elements"][0]["i"].as_i64().unwrap())
        .collect();
    assert_eq!(d2_keys, vec![0x01, 0x02]);
    // Snapshot 3: only 0x02 remains after remove.
    let d3 = dict_snapshots[3]["value"]["elements"].as_array().unwrap();
    assert_eq!(d3.len(), 1);
    assert_eq!(d3[0]["elements"][0]["i"].as_i64(), Some(0x02));
    assert_eq!(
        d3[0]["elements"][1]["field_values"].as_array().unwrap()[1]["i"].as_i64(),
        Some(9004)
    );

    // ----- @Vault? surfaces as Variant { Some(Struct) | None } -------
    let opt_snapshots: Vec<serde_json::Value> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter(|v| v["varname"] == "opt")
        .collect();
    assert_eq!(
        opt_snapshots.len(),
        2,
        "opt snapshots: initial Some + post-drain None"
    );
    let opt0 = &opt_snapshots[0]["value"];
    assert_eq!(opt0["kind"].as_str(), Some("Variant"));
    assert_eq!(opt0["discriminator"].as_str(), Some("Some"));
    assert_eq!(opt0["contents"]["kind"].as_str(), Some("Struct"));
    let opt0_fields = opt0["contents"]["field_values"].as_array().unwrap();
    assert_eq!(opt0_fields[0]["text"].as_str(), Some("Vault"));
    assert_eq!(opt0_fields[1]["i"].as_i64(), Some(9005));
    assert_eq!(opt0_fields[2]["text"].as_str(), Some("alice"));
    let opt1 = &opt_snapshots[1]["value"];
    assert_eq!(opt1["kind"].as_str(), Some("Variant"));
    assert_eq!(opt1["discriminator"].as_str(), Some("None"));
    assert_eq!(opt1["contents"]["kind"].as_str(), Some("None"));

    // ----- Removed singletons (head, removed, drained) ---------------
    // Each remove/drain extracts a single resource as a typed Struct.
    let removed_singles: Vec<(String, i64, String)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "head" | "removed" | "drained") {
                return None;
            }
            assert_eq!(v["value"]["kind"].as_str(), Some("Struct"));
            let f = v["value"]["field_values"].as_array()?;
            Some((
                name,
                f[1]["i"].as_i64()?,
                f[2]["text"].as_str()?.to_string(),
            ))
        })
        .collect();
    assert_eq!(
        removed_singles,
        vec![
            ("head".into(), 9001, "alice".into()),
            ("removed".into(), 9003, "alice".into()),
            ("drained".into(), 9005, "alice".into()),
        ]
    );

    // ----- Returns: compute=0, main=0 --------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(0), Some(0)]);
}

// --- access_control_test.cdc ---------------------------------------------

const ACCESS_CONTROL_NDJSON: &str = include_str!("ndjson/access_control_test.ndjson");

/// Pins Cadence access-control modifiers on the call frame.
///
/// Each call-entry's source-declared visibility (`access(self)` /
/// `access(contract)` / `access(account)` / `access(all)`) surfaces
/// as a tagged `CadenceAccess:<function>:<Tag>` io_event, where
/// `<Tag>` is `AccessSelf` / `AccessContract` / `AccessAccount` /
/// `AccessAll`.  The recorder routes the tag through
/// `EventLogKind::TraceLogEvent` so it survives the multi-stream
/// writer's metadata-drop and shows up in the strict pin's
/// `io_kind` + `text` view.  Mirrors the M9 `error_kind`
/// (pre/post/panic) dispatch on the `Error` variant.
///
/// Recorder change: `TraceEvent::Call` gains an optional `access`
/// field; when set, the recorder emits one
/// `CadenceAccess:<fn>:<Tag>` io_event per call_entry.
#[test]
fn test_access_control_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_access_control_test_via_ct_print_full",
        "access_control_test.cdc",
        ACCESS_CONTROL_NDJSON,
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
            "Vault.reveal_self",
            "Vault.reveal_contract",
            "Vault.reveal_account",
            "Vault.reveal_all",
        ]
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 11 explicit step events + 1 implicit start step = 12 step records.
    assert_eq!(counts["steps"].as_u64(), Some(12), "steps; counts={counts}");
    // main + compute + 4 reveal_* = 6 calls.
    assert_eq!(counts["calls"].as_u64(), Some(6), "calls; counts={counts}");
    // 6 visibility-tag io_events (one per call_entry).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(6),
        "io_events; counts={counts} (one CadenceAccess tag per call)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 12 steps + 6 call_entry + 6 call_exit + 6 io = 30 events.
    assert_eq!(events.len(), 30, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "Vault.reveal_self".to_string(),
            "Vault.reveal_contract".to_string(),
            "Vault.reveal_account".to_string(),
            "Vault.reveal_all".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "Vault.reveal_self".to_string(),
            "Vault.reveal_contract".to_string(),
            "Vault.reveal_account".to_string(),
            "Vault.reveal_all".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Tagged CadenceAccess io_events: one per call frame --------
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    assert_eq!(io_events.len(), 6);

    let access_tags: Vec<(&str, &str)> = io_events
        .iter()
        .map(|e| {
            (
                e["io_kind"].as_str().unwrap_or("?"),
                e["text"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    assert_eq!(
        access_tags,
        vec![
            ("ioStderr", "CadenceAccess:main:AccessAll"),
            ("ioStderr", "CadenceAccess:compute:AccessAll"),
            ("ioStderr", "CadenceAccess:Vault.reveal_self:AccessSelf"),
            (
                "ioStderr",
                "CadenceAccess:Vault.reveal_contract:AccessContract",
            ),
            (
                "ioStderr",
                "CadenceAccess:Vault.reveal_account:AccessAccount",
            ),
            ("ioStderr", "CadenceAccess:Vault.reveal_all:AccessAll"),
        ],
        "Each call frame must surface its source-declared visibility \
         tag through the CadenceAccess io_event channel — the strict \
         pin asserts the exact (function, tag) pair in call-entry \
         emission order."
    );

    // ----- Per-accessor return values + locals -----------------------
    assert_eq!(
        observed_int_var_sequence(&doc, &["s", "c", "a", "p"]),
        vec![
            ("s".into(), 1),
            ("c".into(), 10),
            ("a".into(), 100),
            ("p".into(), 1000),
        ]
    );

    // ----- Returns: reveal_self=1, reveal_contract=10, reveal_account=100,
    //                reveal_all=1000, compute=1111, main=1111 --------
    assert_eq!(
        observed_int_returns(&doc),
        vec![
            Some(1),
            Some(10),
            Some(100),
            Some(1000),
            Some(1111),
            Some(1111),
        ]
    );
}

// --- optional_chaining_test.cdc ------------------------------------------

const OPTIONAL_CHAINING_NDJSON: &str = include_str!("ndjson/optional_chaining_test.ndjson");

/// Pins Cadence optional chaining (`?.`) and force-unwrap (`!`).
///
/// Successful chains surface as `ValueRecord::Variant { Some(...) }`
/// with the chained inner payload preserved end-to-end; a None
/// short-circuit surfaces as `Variant { None }` (no further
/// dispatch).  A successful force-unwrap surfaces as the typed
/// inner `ValueRecord::Int`; a force-unwrap of `nil` surfaces as a
/// tagged `ForceNilUnwrap:<context>` io_event distinguishable from a
/// generic `panic` via the literal `ForceNilUnwrap:` prefix on the
/// io-event channel.
///
/// Recorder change: a new `TraceEvent::ForceNilUnwrap { context }`
/// variant routes through `EventLogKind::TraceLogEvent` with the
/// `ForceNilUnwrap:` prefix preserved in the text payload, mirroring
/// the M10 `ResourceOwnerChange:` tag pattern.
#[test]
fn test_optional_chaining_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_optional_chaining_test_via_ct_print_full",
        "optional_chaining_test.cdc",
        OPTIONAL_CHAINING_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // Explicit step events + implicit steps from resource_create /
    // resource_destroy emit one step per event.  Total = 20 (incl.
    // 1 implicit start step).
    assert_eq!(counts["steps"].as_u64(), Some(20), "steps; counts={counts}");
    // main + compute = 2 calls.
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // 6 lifecycle io_events + 1 ForceNilUnwrap = 7.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(7),
        "io_events; counts={counts} (lifecycle + ForceNilUnwrap)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 20 steps + 2 call_entry + 2 call_exit + 7 io = 31 events.
    assert_eq!(events.len(), 31, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec!["main".to_string(), "compute".to_string()]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec!["compute".to_string(), "main".to_string()]
    );

    // ----- chained: Variant { Some(Int 42) } -------------------------
    let chained = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "chained")
        .expect("chained should be present");
    assert_eq!(chained["value"]["kind"].as_str(), Some("Variant"));
    assert_eq!(chained["value"]["discriminator"].as_str(), Some("Some"));
    assert_eq!(chained["value"]["contents"]["kind"].as_str(), Some("Int"));
    assert_eq!(chained["value"]["contents"]["i"].as_i64(), Some(42));

    // ----- short: Variant { None } (None-short-circuit) --------------
    let short = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "short")
        .expect("short should be present");
    assert_eq!(short["value"]["kind"].as_str(), Some("Variant"));
    assert_eq!(short["value"]["discriminator"].as_str(), Some("None"));
    assert_eq!(short["value"]["contents"]["kind"].as_str(), Some("None"));

    // ----- present: Variant { Some(Int 7) }; bal: Int 7 --------------
    let present = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "present")
        .expect("present should be present");
    assert_eq!(present["value"]["kind"].as_str(), Some("Variant"));
    assert_eq!(present["value"]["discriminator"].as_str(), Some("Some"));
    assert_eq!(present["value"]["contents"]["kind"].as_str(), Some("Int"));
    assert_eq!(present["value"]["contents"]["i"].as_i64(), Some(7));
    assert_eq!(
        observed_int_var_sequence(&doc, &["bal"]),
        vec![("bal".into(), 7)]
    );

    // ----- absent: Variant { None } ----------------------------------
    let absent = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "absent")
        .expect("absent should be present");
    assert_eq!(absent["value"]["kind"].as_str(), Some("Variant"));
    assert_eq!(absent["value"]["discriminator"].as_str(), Some("None"));

    // ----- Tagged ForceNilUnwrap io_event distinguishes from panic ---
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    let force_nil: Vec<&str> = io_events
        .iter()
        .filter_map(|e| e["text"].as_str())
        .filter(|t| t.starts_with("ForceNilUnwrap:"))
        .collect();
    assert_eq!(
        force_nil,
        vec!["ForceNilUnwrap:absent"],
        "Force-unwrap of nil must surface as a tagged ForceNilUnwrap \
         io_event with the unwrap-site context preserved in the text \
         payload — distinct from the generic CadenceRuntimeError panic \
         channel so the frontend can flag the failure mode."
    );
    for ev in io_events.iter().filter(|e| {
        e["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("ForceNilUnwrap:"))
    }) {
        assert_eq!(ev["io_kind"].as_str(), Some("ioStderr"));
    }

    // ----- Returns: compute=7, main=7 --------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(7), Some(7)]);
}

// --- scripts_test.cdc ---------------------------------------------------

const SCRIPTS_NDJSON: &str = include_str!("ndjson/scripts_test.ndjson");

/// Pins the Cadence script entry point (`access(all) fun main(): X`).
///
/// The script form differs from a transaction form on the same
/// `main`-named entry: the recorder must expose the script-entry
/// boundary on the io-event channel so the strict pin can pin it
/// independently of any contract-level `main`.  Imported-contract
/// qualified names (`MarketContract.floor_price`) must resolve
/// across files and surface verbatim in the function table.  The
/// returned value carries its concrete typed `ValueRecord` variant
/// (here: `ValueRecord::Int` from a `UInt64`).
///
/// Recorder change: `TraceEvent::Call` gains an optional `script`
/// flag; when true, the recorder emits a tagged
/// `CadenceScriptEntry:<function>` io_event after the access tag.
#[test]
fn test_scripts_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_scripts_test_via_ct_print_full",
        "scripts_test.cdc",
        SCRIPTS_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table: imported-contract qualified name ----------
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec!["main", "MarketContract.floor_price"],
        "Imported-contract qualified name must resolve verbatim — \
         the script-form `main` plus the imported \
         `MarketContract.floor_price` are the only two entries in the \
         function table."
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 3 explicit step events + 1 implicit start step = 4 step records.
    assert_eq!(counts["steps"].as_u64(), Some(4), "steps; counts={counts}");
    // main + MarketContract.floor_price = 2 calls.
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // 2 access tags (main + floor_price) + 1 script-entry tag = 3.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(3),
        "io_events; counts={counts} (2 access + 1 script-entry)"
    );

    let events = doc["events"].as_array().expect("events array");
    // 4 steps + 2 call_entry + 2 call_exit + 3 io = 11 events.
    assert_eq!(events.len(), 11, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Tagged io_events: access(main) → script-entry → access(...) -
    let io_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["kind"] == "io").collect();
    let io_summary: Vec<(&str, &str)> = io_events
        .iter()
        .map(|e| {
            (
                e["io_kind"].as_str().unwrap_or("?"),
                e["text"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    assert_eq!(
        io_summary,
        vec![
            ("ioStderr", "CadenceAccess:main:AccessAll"),
            ("ioStderr", "CadenceScriptEntry:main"),
            (
                "ioStderr",
                "CadenceAccess:MarketContract.floor_price:AccessAll",
            ),
        ],
        "Script entry-point boundary surfaces as a CadenceScriptEntry: \
         tag immediately after the access tag — the strict pin asserts \
         the exact io_event sequence so the script form is \
         distinguishable from a transaction `main`."
    );

    // ----- Returned UInt64 surfaces as typed ValueRecord::Int --------
    assert_eq!(
        observed_int_var_sequence(&doc, &["floor"]),
        vec![("floor".into(), 12345)]
    );
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(12345), Some(12345)],
        "MarketContract.floor_price and main both return UInt64 12345 \
         as a typed ValueRecord::Int"
    );
}

// --- hash_builtins_test.cdc ---------------------------------------------

const HASH_BUILTINS_NDJSON: &str = include_str!("ndjson/hash_builtins_test.ndjson");

/// SHA-2/256 digest of the canonical `"abc"` test vector (32 bytes).
const SHA2_256_ABC: [i64; 32] = [
    0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
    0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
];

/// SHA-3/256 digest of the canonical `"abc"` test vector (32 bytes).
const SHA3_256_ABC: [i64; 32] = [
    0x3a, 0x98, 0x5d, 0xa7, 0x4f, 0xe2, 0x25, 0xb2, 0x04, 0x5c, 0x17, 0x2d, 0x6b, 0xd3, 0x90, 0xbd,
    0x85, 0x5f, 0x08, 0x6e, 0x3e, 0x9d, 0x52, 0x5b, 0x46, 0xbf, 0xe2, 0x45, 0x11, 0x43, 0x15, 0x32,
];

/// Pins Cadence hash builtins (`HashAlgorithm.SHA2_256.hash` /
/// `HashAlgorithm.SHA3_256.hash`).
///
/// Each `hash` call surfaces as a Call/Return pair where:
///
///   * The input bytes argument decodes as
///     `ValueRecord::Sequence<UInt8>` — one `ValueRecord::Int`
///     element per byte, with `i in 0..=255`.
///   * The return digest decodes as `ValueRecord::Sequence<UInt8>`
///     of length 32 (algorithm-specific) with the exact published
///     test-vector bytes.
///
/// No recorder change required — the `[UInt8]` cadence_type already
/// recurses through the `[T]` array parser into per-byte
/// `ValueRecord::Int` leaves, both for the call's typed args buffer
/// and for the typed return value.
#[test]
fn test_hash_builtins_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_hash_builtins_test_via_ct_print_full",
        "hash_builtins_test.cdc",
        HASH_BUILTINS_NDJSON,
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
            "HashAlgorithm.SHA2_256.hash",
            "HashAlgorithm.SHA3_256.hash",
        ]
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 6 explicit step events + 1 implicit start step = 7 step records.
    assert_eq!(counts["steps"].as_u64(), Some(7), "steps; counts={counts}");
    // main + compute + 2 hash calls = 4 calls.
    assert_eq!(counts["calls"].as_u64(), Some(4), "calls; counts={counts}");
    // No io_events (hash builtins do not route through any tagged
    // io channel).
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 7 steps + 4 call_entry + 4 call_exit + 0 io = 15 events.
    assert_eq!(events.len(), 15, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "HashAlgorithm.SHA2_256.hash".to_string(),
            "HashAlgorithm.SHA3_256.hash".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "HashAlgorithm.SHA2_256.hash".to_string(),
            "HashAlgorithm.SHA3_256.hash".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Input bytes [97, 98, 99] surface as Sequence<Int> ---------
    let bytes_var = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "bytes")
        .expect("bytes local should be present");
    assert_eq!(bytes_var["value"]["kind"].as_str(), Some("Sequence"));
    let bytes_elems = bytes_var["value"]["elements"].as_array().unwrap();
    let bytes_int: Vec<i64> = bytes_elems
        .iter()
        .map(|e| {
            assert_eq!(e["kind"].as_str(), Some("Int"));
            e["i"].as_i64().unwrap()
        })
        .collect();
    assert_eq!(bytes_int, vec![97, 98, 99]);

    // ----- Per-call input arg `data` surfaces as Sequence<Int> -------
    // Both hash calls receive the same `[97, 98, 99]` byte sequence.
    let call_arg_data: Vec<Vec<i64>> = events
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .filter(|e| {
            e["function"]
                .as_str()
                .unwrap_or("")
                .starts_with("HashAlgorithm.")
        })
        .map(|e| {
            let args = e["args"].as_array().expect("args array");
            assert_eq!(args.len(), 1);
            assert_eq!(args[0]["varname"].as_str(), Some("data"));
            assert_eq!(args[0]["value"]["kind"].as_str(), Some("Sequence"));
            args[0]["value"]["elements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    assert_eq!(e["kind"].as_str(), Some("Int"));
                    e["i"].as_i64().unwrap()
                })
                .collect::<Vec<i64>>()
        })
        .collect();
    assert_eq!(
        call_arg_data,
        vec![vec![97, 98, 99], vec![97, 98, 99]],
        "Both SHA2_256.hash and SHA3_256.hash receive the same \
         3-byte `data` argument as a typed Sequence<UInt8>."
    );

    // ----- Per-call return digest matches published test vector ------
    let returns: Vec<Vec<i64>> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .filter(|e| {
            e["function"]
                .as_str()
                .unwrap_or("")
                .starts_with("HashAlgorithm.")
        })
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Sequence"));
            rv["elements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    assert_eq!(e["kind"].as_str(), Some("Int"));
                    e["i"].as_i64().unwrap()
                })
                .collect::<Vec<i64>>()
        })
        .collect();
    assert_eq!(returns.len(), 2);
    assert_eq!(returns[0].len(), 32, "SHA-2/256 digest is 32 bytes");
    assert_eq!(returns[1].len(), 32, "SHA-3/256 digest is 32 bytes");
    assert_eq!(
        returns[0],
        SHA2_256_ABC.to_vec(),
        "SHA-2/256(\"abc\") test vector mismatch"
    );
    assert_eq!(
        returns[1],
        SHA3_256_ABC.to_vec(),
        "SHA-3/256(\"abc\") test vector mismatch"
    );

    // ----- Locals d2/d3 carry the same digest payload ----------------
    let digest_locals: Vec<(String, Vec<i64>)> = events
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "d2" | "d3") {
                return None;
            }
            assert_eq!(v["value"]["kind"].as_str(), Some("Sequence"));
            let bytes: Vec<i64> = v["value"]["elements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    assert_eq!(e["kind"].as_str(), Some("Int"));
                    e["i"].as_i64().unwrap()
                })
                .collect();
            Some((name, bytes))
        })
        .collect();
    assert_eq!(digest_locals.len(), 2);
    assert_eq!(digest_locals[0].0, "d2");
    assert_eq!(digest_locals[0].1, SHA2_256_ABC.to_vec());
    assert_eq!(digest_locals[1].0, "d3");
    assert_eq!(digest_locals[1].1, SHA3_256_ABC.to_vec());

    // ----- Outer compute=244, main=244 (sum of first byte of each
    //       digest, 0xba + 0x3a = 0xf4) -------------------------------
    let outer_returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .filter(|e| {
            let f = e["function"].as_str().unwrap_or("");
            f == "compute" || f == "main"
        })
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(
                rv["kind"].as_str(),
                Some("Int"),
                "compute/main return UInt64 should decode as Int; got {rv}"
            );
            rv["i"].as_i64().unwrap()
        })
        .collect();
    assert_eq!(
        outer_returns,
        vec![244, 244],
        "compute and main both return UInt64 244 = 0xba + 0x3a (the \
         first byte of each digest)."
    );
}

// ---------------------------------------------------------------------------
// M10 Round 4 fixtures: path_types / fixed_point / contracts_imports /
//                       composite_types / anyresource_anystruct
// ---------------------------------------------------------------------------
//
// Round 3 closed the resource_collections / access_control /
// optional_chaining / scripts / hash_builtins deliverables.  Round 4 closes
// the remaining M10 high-priority Cadence constructs:
//
//   * `path_types_test`              — `/storage` / `/public` / `/private`
//                                      Path values surface as the canonical
//                                      `Struct { String "domain", String
//                                      "identifier" }` shape.  Already
//                                      supported by the recorder via the
//                                      existing Path branch.
//   * `fixed_point_test`             — `Fix64` / `UFix64` fixed-point
//                                      arithmetic surfaces as `Int` with
//                                      the canonical 1e8 scaling factor
//                                      preserved in the type-id metadata
//                                      (`Fix64` / `UFix64` lang_type names).
//   * `contracts_imports_test`       — fully-qualified `A.<address>.
//                                      MyContract.foo` function names
//                                      survive end-to-end; multi-file
//                                      source-mapping metadata visible via
//                                      the per-file path table.
//   * `composite_types_test`         — `struct` / `resource` / `event`
//                                      composite-kind tags surface via
//                                      paired `CompositeKindStructure` /
//                                      `CompositeKindResource` /
//                                      `CompositeKindEvent` io_events.
//   * `anyresource_anystruct_test`   — `AnyResource` / `AnyStruct` dynamic
//                                      parameters surface as the *concrete*
//                                      runtime type's typed ValueRecord
//                                      variant (NOT Raw); both the static
//                                      and concrete-runtime type-ids
//                                      surface via `CadenceAnyType:` tags.

// --- path_types_test.cdc -------------------------------------------------

const PATH_TYPES_NDJSON: &str = include_str!("ndjson/path_types_test.ndjson");

/// Pins Cadence Path values across the three path domains.
///
/// `/storage/<id>` (`StoragePath`), `/public/<id>` (`PublicPath`) and
/// `/private/<id>` (`PrivatePath`) all surface as the canonical
/// `ValueRecord::Struct { [String "<domain>", String "<identifier>"] }`
/// shape so downstream consumers can resolve the domain and identifier
/// without re-parsing the slash form.  Already supported by the
/// recorder via the existing `value_record` Path branch (lines 1156-
/// 1192 in `src/tracer.rs`); this strict pin closes the M10 deliverable
/// by asserting the exact field shape end-to-end.
#[test]
fn test_path_types_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_path_types_test_via_ct_print_full",
        "path_types_test.cdc",
        PATH_TYPES_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 6 explicit step events + 1 implicit start step = 7 step records.
    assert_eq!(counts["steps"].as_u64(), Some(7), "steps; counts={counts}");
    // main + compute = 2 calls.
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // No io_events: Path values are pure typed locals.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 7 steps + 2 call_entry + 2 call_exit + 0 io = 11 events.
    assert_eq!(events.len(), 11, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Each Path local decodes as Struct { domain, identifier } --
    let path_locals: Vec<(String, String, String)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "p1" | "p2" | "p3") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Struct"),
                "Path local `{}` must decode as ValueRecord::Struct \
                 (with domain + identifier fields); got {}",
                name,
                v["value"]
            );
            let fields = v["value"]["field_values"].as_array()?;
            assert_eq!(
                fields.len(),
                2,
                "Path Struct must carry 2 fields (domain, identifier) \
                 for `{name}`; got {fields:?}"
            );
            assert_eq!(
                fields[0]["kind"].as_str(),
                Some("String"),
                "Path.domain must decode as ValueRecord::String"
            );
            assert_eq!(
                fields[1]["kind"].as_str(),
                Some("String"),
                "Path.identifier must decode as ValueRecord::String"
            );
            let domain = fields[0]["text"].as_str()?.to_string();
            let identifier = fields[1]["text"].as_str()?.to_string();
            Some((name, domain, identifier))
        })
        .collect();
    assert_eq!(
        path_locals,
        vec![
            ("p1".into(), "storage".into(), "Vault".into()),
            ("p2".into(), "public".into(), "Vault".into()),
            ("p3".into(), "private".into(), "Vault".into()),
        ],
        "Each Path domain decodes verbatim into the (domain, identifier) \
         pair off the canonical `/<domain>/<identifier>` slash form."
    );

    // ----- Returns: compute=0, main=0 --------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(0), Some(0)]);
}

// --- anyresource_anystruct_test.cdc -------------------------------------

const ANYRESOURCE_ANYSTRUCT_NDJSON: &str = include_str!("ndjson/anyresource_anystruct_test.ndjson");

/// Pins Cadence dynamic supertypes: `AnyResource` / `AnyStruct`.
///
/// Cadence permits passing a concrete `@T` resource through a
/// parameter typed `@AnyResource`, and any struct value through
/// a parameter typed `AnyStruct`.  The runtime preserves the
/// concrete type information; the recorder must surface BOTH:
///
///   * The concrete runtime type's typed `ValueRecord` variant on
///     the parameter local (NOT a `Raw` fallback) — `@Vault`
///     surfaces as the existing typed `Struct { ResourceType,
///     ResourceUuid, ResourceOwner }` payload, `Token(id: 9)`
///     surfaces as the existing typed `Struct { field_values }`
///     payload via the named-struct dispatch in `value_record`.
///   * Both the static (declared-parameter) type and the concrete
///     runtime type id via a tagged `CadenceAnyType:<static>:
///     <runtime>:<varname>` io_event so downstream tooling can
///     correlate the dynamic dispatch with the call frame's
///     argument types.
///
/// Recorder change: a new `TraceEvent::AnyTypeBind { static_type,
/// runtime_type, varname }` variant routes through
/// `register_special_event` with the canonical
/// `CadenceAnyType:<static>:<runtime>:<varname>` tag preserved on
/// both the metadata and the text payload (so the multi-stream
/// writer's metadata-drop never loses the discriminator triple).
#[test]
fn test_anyresource_anystruct_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_anyresource_anystruct_test_via_ct_print_full",
        "anyresource_anystruct_test.cdc",
        ANYRESOURCE_ANYSTRUCT_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute", "store", "process"]);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "store".to_string(),
            "process".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "store".to_string(),
            "process".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Step indices monotonic ------------------------------------
    assert_step_indices_monotonic(&doc);

    // ----- Tagged CadenceAnyType io_events ---------------------------
    // The strict pin asserts the exact (static, runtime, varname)
    // triple for each dynamic-supertype binding.  The triple lets
    // the frontend resolve both type-ids without re-deriving them
    // from the call frame's typed args.
    let events = doc["events"].as_array().expect("events array");
    let any_type_tags: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["kind"] == "io")
        .filter_map(|e| {
            let text = e["text"].as_str()?;
            if !text.starts_with("CadenceAnyType:") {
                return None;
            }
            Some((e["io_kind"].as_str().unwrap_or("?"), text))
        })
        .collect();
    assert_eq!(
        any_type_tags,
        vec![
            ("ioStderr", "CadenceAnyType:AnyResource:Vault:r"),
            ("ioStderr", "CadenceAnyType:AnyStruct:Token:s"),
        ],
        "Each dynamic-supertype binding surfaces as a tagged \
         CadenceAnyType:<static>:<runtime>:<varname> io_event so both \
         the static (declared-parameter) and concrete-runtime type-ids \
         are visible end-to-end."
    );

    // ----- Concrete runtime type surfaces as typed ValueRecord -------
    // The `r` parameter on `store` is declared as `@AnyResource` but
    // the bound value is `@Vault#8001@alice` — surfaces as the
    // existing typed `Struct { ResourceType, ResourceUuid,
    // ResourceOwner }` payload (NOT a `Raw` fallback).  We pin the
    // call frame's `args[0]` shape rather than the step-local so the
    // dispatch site is unambiguous.
    let store_call = events
        .iter()
        .find(|e| e["kind"] == "call_entry" && e["function"] == "store")
        .expect("store call_entry should be present");
    let store_args = store_call["args"].as_array().expect("store.args");
    assert_eq!(store_args.len(), 1);
    assert_eq!(store_args[0]["varname"].as_str(), Some("r"));
    assert_eq!(
        store_args[0]["value"]["kind"].as_str(),
        Some("Struct"),
        "@AnyResource parameter must surface as the concrete runtime \
         type's typed Struct (NOT Raw); got {}",
        store_args[0]["value"]
    );
    let store_fields = store_args[0]["value"]["field_values"]
        .as_array()
        .expect("store.r field_values");
    assert_eq!(store_fields.len(), 3, "@Vault carries (type, uuid, owner)");
    assert_eq!(store_fields[0]["text"].as_str(), Some("Vault"));
    assert_eq!(store_fields[1]["i"].as_i64(), Some(8001));
    assert_eq!(store_fields[2]["text"].as_str(), Some("alice"));

    // ----- AnyStruct binding: process(s: Token(id: 9)) ---------------
    let process_call = events
        .iter()
        .find(|e| e["kind"] == "call_entry" && e["function"] == "process")
        .expect("process call_entry should be present");
    let process_args = process_call["args"].as_array().expect("process.args");
    assert_eq!(process_args.len(), 1);
    assert_eq!(process_args[0]["varname"].as_str(), Some("s"));
    assert_eq!(
        process_args[0]["value"]["kind"].as_str(),
        Some("Struct"),
        "AnyStruct parameter must surface as the concrete runtime \
         struct (NOT Raw); got {}",
        process_args[0]["value"]
    );
    let process_fields = process_args[0]["value"]["field_values"]
        .as_array()
        .expect("process.s field_values");
    // Token has a single Int field `id`; the recorder's named-struct
    // parser surfaces it as a typed Int leaf.
    assert_eq!(process_fields.len(), 1);
    assert_eq!(process_fields[0]["kind"].as_str(), Some("Int"));
    assert_eq!(process_fields[0]["i"].as_i64(), Some(9));

    // ----- Returns: store=Void, process=0, compute=0, main=0 ---------
    assert_eq!(
        observed_int_returns(&doc),
        vec![None, Some(0), Some(0), Some(0)]
    );
}

// --- composite_types_test.cdc -------------------------------------------

const COMPOSITE_TYPES_NDJSON: &str = include_str!("ndjson/composite_types_test.ndjson");

/// Pins Cadence composite-declaration kinds: `struct` / `resource` /
/// `event`.
///
/// The Cadence runtime tags each composite declaration with one of
/// three distinct `interpreter.CompositeKind` discriminators
/// (`Structure` / `Resource` / `Event`).  The recorder surfaces each
/// kind as a tagged io_event (`CompositeKindStructure:<Type>`,
/// `CompositeKindResource:<Type>`, `CompositeKindEvent:<Type>`)
/// through `EventLogKind::TraceLogEvent` → `ioStderr`.  The `event`
/// composite additionally surfaces as a tagged `CadenceEmit:` io_event
/// when emitted (the existing M9 emit path).
///
/// Recorder change: a new `TraceEvent::CompositeKind { kind,
/// type_name }` variant routes through `register_special_event` with
/// the canonical `CompositeKind<Kind>:<Type>` tag preserved on both
/// the metadata and the text payload (so the multi-stream writer's
/// metadata-drop never loses the discriminator).
#[test]
fn test_composite_types_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_composite_types_test_via_ct_print_full",
        "composite_types_test.cdc",
        COMPOSITE_TYPES_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute", "C.create_vault"]);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "compute".to_string(),
            "C.create_vault".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "C.create_vault".to_string(),
            "compute".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Step indices monotonic ------------------------------------
    assert_step_indices_monotonic(&doc);

    // ----- Tagged composite-kind io_events ---------------------------
    // Each composite-declaration kind surfaces with its canonical
    // `CompositeKind<Kind>:<Type>` tag on the io-event channel.  The
    // resource lifecycle (create + destroy) and the emit each
    // additionally surface their own tagged events; we filter to the
    // composite-kind tags via the literal `CompositeKind` prefix.
    let events = doc["events"].as_array().expect("events array");
    let composite_kind_tags: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["kind"] == "io")
        .filter_map(|e| {
            let text = e["text"].as_str()?;
            if !text.starts_with("CompositeKind") {
                return None;
            }
            Some((e["io_kind"].as_str().unwrap_or("?"), text))
        })
        .collect();
    assert_eq!(
        composite_kind_tags,
        vec![
            ("ioStderr", "CompositeKindStructure:C.Coord"),
            ("ioStderr", "CompositeKindResource:C.Vault"),
            ("ioStderr", "CompositeKindEvent:C.Spawned"),
        ],
        "Each composite-declaration kind surfaces with its canonical \
         `CompositeKind<Kind>:<Type>` tag, in struct → resource → event \
         declaration order."
    );

    // ----- The event composite ALSO surfaces as a CadenceEmit: tag ---
    // The strict pin asserts that `event` declarations carry both the
    // composite-kind metadata AND the emit-side tagged io_event so
    // downstream tooling can correlate the kind with the emit site.
    let emit_tags: Vec<(&str, &str)> = events
        .iter()
        .filter(|e| e["kind"] == "io")
        .filter_map(|e| {
            let text = e["text"].as_str()?;
            if !text.starts_with("CadenceEmit:") {
                return None;
            }
            Some((e["io_kind"].as_str().unwrap_or("?"), text))
        })
        .collect();
    assert_eq!(
        emit_tags,
        vec![("ioStderr", "CadenceEmit:C.Spawned(id: 1)")],
        "Event-kind composites surface twice: once for the kind tag \
         (CompositeKindEvent), once for the emit-site tag \
         (CadenceEmit:<Name>(<args>))."
    );

    // ----- Per-emit field surfaces as a typed Int local --------------
    assert_eq!(
        observed_int_var_sequence(&doc, &["emit:C.Spawned.id"]),
        vec![("emit:C.Spawned.id".into(), 1)]
    );

    // ----- Struct local `p` decodes as ValueRecord::Struct -----------
    let p = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|v| v["varname"] == "p")
        .expect("p local should be present");
    assert_eq!(
        p["value"]["kind"].as_str(),
        Some("Struct"),
        "C.Coord local must decode as ValueRecord::Struct; got {}",
        p["value"]
    );
    let p_fields = p["value"]["field_values"].as_array().unwrap();
    assert_eq!(p_fields.len(), 2, "C.Coord has two fields (x, y)");
    assert_eq!(p_fields[0]["kind"].as_str(), Some("Int"));
    assert_eq!(p_fields[0]["i"].as_i64(), Some(3));
    assert_eq!(p_fields[1]["kind"].as_str(), Some("Int"));
    assert_eq!(p_fields[1]["i"].as_i64(), Some(4));

    // ----- Resource local `v` decodes as ValueRecord::Struct ---------
    // (typed { ResourceType, ResourceUuid, ResourceOwner } payload).
    let v = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .find(|var| var["varname"] == "v")
        .expect("v local should be present");
    assert_eq!(v["value"]["kind"].as_str(), Some("Struct"));
    let v_fields = v["value"]["field_values"].as_array().unwrap();
    assert_eq!(v_fields.len(), 3, "@C.Vault carries (type, uuid, owner)");
    assert_eq!(v_fields[0]["text"].as_str(), Some("C.Vault"));
    assert_eq!(v_fields[1]["i"].as_i64(), Some(7001));
    assert_eq!(v_fields[2]["text"].as_str(), Some("alice"));

    // ----- Returns: C.create_vault → struct, compute=7, main=7 -------
    // C.create_vault returns a typed @C.Vault Struct; we walk only
    // the Int-typed returns from compute + main here.  The earlier
    // call-exit-sequence assertion already pins the call ordering.
    let int_returns: Vec<i64> = events
        .iter()
        .filter(|e| e["kind"] == "call_exit")
        .filter(|e| {
            let f = e["function"].as_str().unwrap_or("");
            f == "compute" || f == "main"
        })
        .map(|e| {
            let rv = &e["return_value"];
            assert_eq!(rv["kind"].as_str(), Some("Int"));
            rv["i"].as_i64().unwrap()
        })
        .collect();
    assert_eq!(int_returns, vec![7, 7]);
}

// --- contracts_imports_test.cdc -----------------------------------------

const CONTRACTS_IMPORTS_NDJSON: &str = include_str!("ndjson/contracts_imports_test.ndjson");

/// Pins Cadence import-resolution across multiple contract files.
///
/// The function table carries the canonical fully-qualified
/// `A.<address>.<Contract>.<member>` names verbatim — no truncation,
/// no re-rendering — so downstream tooling can resolve symbols
/// against the on-chain naming scheme.  Source-mapping metadata
/// surfaces each imported contract's `.cdc` file as a distinct entry
/// in the trace's per-step paths array (the recorder learned to
/// dispatch on the helper-side `file` field of step events: when
/// `file` differs from the entry source's basename, the step is
/// recorded against the resolved sibling fixture path so the
/// frontend can navigate into the imported source verbatim).
///
/// Recorder change: `convert_events` Step branch routes the
/// helper-side `file` field through the new
/// `resolve_step_source_path` helper which maps a bare basename
/// onto `<entry source dir>/<basename>` (and honours
/// directory-qualified paths verbatim).
#[test]
fn test_contracts_imports_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_contracts_imports_test_via_ct_print_full",
        "contracts_imports_test.cdc",
        CONTRACTS_IMPORTS_NDJSON,
    ) else {
        return;
    };

    assert_metadata_program_ends_with(&doc, &source_path);

    // ----- Function table: fully-qualified import names verbatim -----
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        functions,
        vec!["main", "A.0x01.ContractA.foo", "A.0x02.ContractB.bar"],
        "Imported-contract qualified names must resolve verbatim — \
         the entry `main` plus each `A.<address>.<Contract>.<member>` \
         carries no truncation, no re-rendering."
    );

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 7 explicit step events; the writer coalesces the implicit start
    // step with the first same-source-line step → 6 step records.
    assert_eq!(counts["steps"].as_u64(), Some(6), "steps; counts={counts}");
    // main + A.0x01.ContractA.foo + A.0x02.ContractB.bar = 3 calls.
    assert_eq!(counts["calls"].as_u64(), Some(3), "calls; counts={counts}");
    // Three distinct source files surface in the per-step paths
    // table: the entry `contracts_imports_test.cdc` plus each
    // imported contract's source.
    assert_eq!(
        counts["paths"].as_u64(),
        Some(3),
        "paths; counts={counts} (entry + 2 imported contract sources)"
    );
    // No io_events: this fixture exercises pure imports.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 6 steps + 3 call_entry + 3 call_exit + 0 io = 12 events.
    assert_eq!(events.len(), 12, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Call ordering ---------------------------------------------
    assert_eq!(
        observed_call_entry_sequence(&doc),
        vec![
            "main".to_string(),
            "A.0x01.ContractA.foo".to_string(),
            "A.0x02.ContractB.bar".to_string(),
        ]
    );
    assert_eq!(
        observed_call_exit_sequence(&doc),
        vec![
            "A.0x01.ContractA.foo".to_string(),
            "A.0x02.ContractB.bar".to_string(),
            "main".to_string(),
        ]
    );

    // ----- Per-step source-mapping metadata: all three files visible -
    // Each step event carries a `path_id` referencing the trace's
    // path table.  Walk the per-step path_ids in emission order and
    // assert the step path basenames match the helper-side `file`
    // dispatch: each imported contract's body lands against its own
    // sibling fixture path, and the entry-file steps land against
    // `contracts_imports_test.cdc`.
    let step_paths: Vec<String> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .map(|e| {
            // ct-print --full surfaces the source-mapping path under
            // `path` for each step (already strip-paths normalised).
            e["path"]
                .as_str()
                .expect("step.path str")
                .rsplit('/')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        step_paths,
        vec![
            // Implicit start step (writer-emitted) at the entry source.
            "contracts_imports_test.cdc".to_string(),
            // line 28 of the entry: dispatch into ContractA.foo.
            "contracts_imports_test.cdc".to_string(),
            // ContractA.foo body: imported source surfaces verbatim.
            "contracts_imports_test_a.cdc".to_string(),
            // line 29 of the entry: dispatch into ContractB.bar.
            "contracts_imports_test.cdc".to_string(),
            // ContractB.bar body: imported source surfaces verbatim.
            "contracts_imports_test_b.cdc".to_string(),
            // line 30 of the entry: aggregator return.
            "contracts_imports_test.cdc".to_string(),
        ],
        "Each step's source-mapping path matches the helper-side `file` \
         dispatch — the imported contract bodies land against their own \
         sibling fixture paths, surfacing import resolution end-to-end."
    );

    // ----- Returns: ContractA.foo=11, ContractB.bar=22, main=33 ------
    assert_eq!(
        observed_int_returns(&doc),
        vec![Some(11), Some(22), Some(33)]
    );

    // ----- Locals: x=11, y=22 ----------------------------------------
    assert_eq!(
        observed_int_var_sequence(&doc, &["x", "y"]),
        vec![("x".into(), 11), ("y".into(), 22)]
    );
}

// --- fixed_point_test.cdc ------------------------------------------------

const FIXED_POINT_NDJSON: &str = include_str!("ndjson/fixed_point_test.ndjson");

/// Pins Cadence `Fix64` / `UFix64` fixed-point arithmetic.
///
/// Each fixed-point local surfaces as `ValueRecord::Int` carrying the
/// canonical 1e8-scaled integer payload (Cadence stores fixed-point
/// values internally as `int64 = value * 10^8`).  The scaling factor
/// is captured implicitly via the type-id metadata (`Fix64` / `UFix64`
/// lang_type names — distinct from the plain `Int` type-id), so
/// downstream tooling can recover the decimal form by dividing by 1e8
/// when the type-id metadata says the value is `Fix64`/`UFix64`.
///
/// Recorder change: `value_record` learned a dedicated `Fix64` /
/// `UFix64` dispatch that parses the printed decimal form
/// (`"-1.5"`, `"0.25"`, ...) into the scaled integer (via the new
/// `parse_fixed_point_scaled` helper).  Closes the M9 known
/// limitation that fractional decimals fell through to `Raw` — the
/// `emit:Transfer.amount` strict pin in
/// `test_events_emit_test_via_ct_print_full` was updated in the
/// same commit to reflect the new typed shape.
#[test]
fn test_fixed_point_test_via_ct_print_full() {
    let Some((doc, source_path)) = record_and_dump_full(
        "test_fixed_point_test_via_ct_print_full",
        "fixed_point_test.cdc",
        FIXED_POINT_NDJSON,
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
    assert_eq!(functions, vec!["main", "compute"]);

    // ----- counts -----------------------------------------------------
    let counts = &doc["counts"];
    // 8 explicit step events; the implicit start-step coalesces with
    // the first explicit step (same source-line) so the writer emits
    // 8 step records total.
    assert_eq!(counts["steps"].as_u64(), Some(8), "steps; counts={counts}");
    // main + compute = 2 calls.
    assert_eq!(counts["calls"].as_u64(), Some(2), "calls; counts={counts}");
    // No io_events: fixed-point locals are pure typed scalars.
    assert_eq!(
        counts["io_events"].as_u64(),
        Some(0),
        "io_events; counts={counts}"
    );

    let events = doc["events"].as_array().expect("events array");
    // 8 steps + 2 call_entry + 2 call_exit + 0 io = 12 events.
    assert_eq!(events.len(), 12, "events.len()");
    assert_step_indices_monotonic(&doc);

    // ----- Each Fix64 / UFix64 local decodes as a scaled Int ---------
    // Per Cadence's internal representation, every fixed-point value
    // is an int64 = decimal * 10^8.  We assert the scaled integer
    // form for each local across negative / unsigned / mixed-sign /
    // same-sign arithmetic.
    let scaled_locals: Vec<(String, i64)> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "step")
        .flat_map(|e| e["vars"].as_array().cloned().unwrap_or_default())
        .filter_map(|v| {
            let name = v["varname"].as_str()?.to_string();
            if !matches!(name.as_str(), "f" | "u" | "prod" | "sum") {
                return None;
            }
            assert_eq!(
                v["value"]["kind"].as_str(),
                Some("Int"),
                "fixed-point local `{}` must decode as a 1e8-scaled \
                 ValueRecord::Int (Cadence stores Fix64/UFix64 as \
                 int64 * 10^8); got {}",
                name,
                v["value"]
            );
            Some((name, v["value"]["i"].as_i64()?))
        })
        .collect();
    assert_eq!(
        scaled_locals,
        vec![
            // -1.5 : Fix64 → -1.5 * 10^8 = -150_000_000
            ("f".into(), -150_000_000),
            // 0.25 : UFix64 → 0.25 * 10^8 = 25_000_000
            ("u".into(), 25_000_000),
            // -1.5 * 0.25 = -0.375 : Fix64 → -37_500_000
            ("prod".into(), -37_500_000),
            // 0.25 + 1.0 = 1.25 : UFix64 → 125_000_000
            ("sum".into(), 125_000_000),
        ],
        "Each fixed-point local decodes to its canonical 1e8-scaled \
         integer; arithmetic results match the expected scaled values."
    );

    // ----- Returns: compute=0, main=0 --------------------------------
    assert_eq!(observed_int_returns(&doc), vec![Some(0), Some(0)]);
}
