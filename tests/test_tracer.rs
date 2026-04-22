//! Integration tests for the Cadence tracer.
//!
//! Tests are split into two categories:
//!
//! 1. **NDJSON-based tests** (always run): These feed pre-built NDJSON data
//!    through the tracer's `trace_program_from_events` path and verify the
//!    resulting CTFS trace output.  They do NOT require the Go helper binary.
//!
//! 2. **Go-helper integration tests** (marked `#[ignore]`): These require the
//!    `cadence-trace-helper` Go binary to be built and available.

use std::path::{Path, PathBuf};

use codetracer_flow_recorder::tracer::{parse_ndjson, CadenceTracer, TraceEvent};
use codetracer_trace_writer_nim::TraceEventsFileFormat;

/// CTFS magic bytes: C0 DE 72 AC E2
const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn test_programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/cadence")
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
    CadenceTracer::trace_program_from_events(
        source_path,
        &events,
        out_dir,
        TraceEventsFileFormat::Json,
    )
    .expect("trace_program_from_events should succeed");
}

fn run_tracer_on_file(source_path: &Path, out_dir: &Path) {
    codetracer_flow_recorder::recorder::record(source_path, out_dir, TraceEventsFileFormat::Json)
        .expect("trace_program should succeed");
}

fn assert_valid_ct_file(out_dir: &Path) -> PathBuf {
    let ct_files: Vec<_> = std::fs::read_dir(out_dir)
        .expect("failed to read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "ct"))
        .collect();

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
    assert!(size > 100, ".ct file should have substantial content, got {} bytes", size);
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

    let output = std::process::Command::new(env!("CARGO"))
        .args([
            "run",
            "--quiet",
            "--",
            "record",
            source_path.to_str().unwrap(),
            "--out-dir",
            out_dir.to_str().unwrap(),
            "--format",
            "json",
        ])
        .output()
        .expect("failed to run");

    assert!(
        output.status.success(),
        "record should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_valid_ct_file(&out_dir);
}
