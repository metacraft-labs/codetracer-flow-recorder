//! Integration tests for the Cadence tracer.
//!
//! Tests are split into two categories:
//!
//! 1. **NDJSON-based tests** (always run): These feed pre-built NDJSON data
//!    through the tracer's `trace_program_from_events` path and verify the
//!    resulting CodeTracer trace output.  They do NOT require the Go helper
//!    binary.
//!
//! 2. **Go-helper integration tests** (marked `#[ignore]`): These require the
//!    `cadence-trace-helper` Go binary to be built and available.  Run them
//!    with `cargo test -- --ignored`.  Set `CADENCE_HELPER_BIN` to point to
//!    the binary if it is not on `$PATH`.

use std::path::{Path, PathBuf};

use codetracer_flow_recorder::tracer::{CadenceTracer, parse_ndjson};
use codetracer_trace_writer::TraceEventsFileFormat;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Path to the test-programs directory.
fn test_programs_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-programs/cadence")
}

/// Build the NDJSON trace that the Go helper would produce for flow_test.cdc.
///
/// The program computes: a=10, b=32, sum_val=42, doubled=84, final_result=94.
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

/// Run the tracer on pre-built NDJSON events and return the output directory.
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

/// Run the tracer via the Go helper binary.
fn run_tracer_on_file(source_path: &Path, out_dir: &Path) {
    codetracer_flow_recorder::recorder::record(
        source_path,
        out_dir,
        TraceEventsFileFormat::Json,
    )
    .expect("trace_program should succeed");
}

/// Parse the trace events JSON from the output directory.
fn load_trace_events(out_dir: &Path) -> Vec<serde_json::Value> {
    let events_path = out_dir.join("trace.bin");
    let content = std::fs::read_to_string(&events_path).expect("failed to read trace events");
    let events: serde_json::Value =
        serde_json::from_str(&content).expect("trace events should be valid JSON");
    events
        .as_array()
        .expect("events should be an array")
        .clone()
}

/// Parse trace_metadata.json from the output directory.
fn load_trace_metadata(out_dir: &Path) -> serde_json::Value {
    let metadata_path = out_dir.join("trace_metadata.json");
    let content =
        std::fs::read_to_string(&metadata_path).expect("failed to read trace_metadata.json");
    serde_json::from_str(&content).expect("trace_metadata.json should be valid JSON")
}

/// Collect all Int values from Value events in the trace.
fn collect_int_values(events: &[serde_json::Value]) -> Vec<(i64, i64)> {
    events
        .iter()
        .filter_map(|e| {
            let val = e.get("Value")?;
            let variable_id = val.get("variable_id")?.as_i64()?;
            let value = val.get("value")?;
            if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                let i = value.get("i").and_then(|v| v.as_i64())?;
                Some((variable_id, i))
            } else {
                None
            }
        })
        .collect()
}

/// Collect all VariableName events and return the names in order.
fn collect_variable_names(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            e.get("VariableName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect()
}

/// Find all Int values for a given variable name across the trace.
fn find_variable_values(events: &[serde_json::Value], var_name: &str) -> Vec<i64> {
    let var_names = collect_variable_names(events);
    let var_id = var_names.iter().position(|name| name == var_name);

    match var_id {
        Some(id) => {
            let int_values = collect_int_values(events);
            int_values
                .iter()
                .filter(|(vid, _)| *vid == id as i64)
                .map(|(_, v)| *v)
                .collect()
        }
        None => vec![],
    }
}

// ===========================================================================
// NDJSON-based tests (always run, no Go helper needed)
// ===========================================================================

// ---------------------------------------------------------------------------
// Test 1: NDJSON parsing produces correct trace output files
// ---------------------------------------------------------------------------

#[test]
fn test_ndjson_trace_output_files() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    // Verify the three output files exist and are non-empty.
    for filename in &["trace.bin", "trace_metadata.json", "trace_paths.json"] {
        let path = out_dir.join(filename);
        assert!(path.exists(), "{} should exist", filename);
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 0, "{} should be non-empty", filename);
    }

    // trace.bin should be valid JSON containing an array of events.
    let events = load_trace_events(&out_dir);
    assert!(!events.is_empty(), "trace should have at least one event");

    // There should be Step events.
    let step_count = events.iter().filter(|e| e.get("Step").is_some()).count();
    assert!(
        step_count > 0,
        "trace should contain at least one Step event, got none"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Verify trace contains value 94 (final_result = doubled + a)
// ---------------------------------------------------------------------------

#[test]
fn test_ndjson_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let int_values = collect_int_values(&events);
    let all_values: Vec<i64> = int_values.iter().map(|(_, v)| *v).collect();

    assert!(
        all_values.contains(&94),
        "trace should contain value 94 (final_result = doubled + a = 84 + 10), got values: {:?}",
        {
            let mut unique: Vec<i64> = all_values.clone();
            unique.sort();
            unique.dedup();
            unique
        }
    );
}

// ---------------------------------------------------------------------------
// Test 3: Verify variable values a=10, b=32, sum_val=42, doubled=84,
//         final_result=94
// ---------------------------------------------------------------------------

#[test]
fn test_ndjson_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    // Check that VariableName events exist.
    let var_names = collect_variable_names(&events);
    assert!(
        !var_names.is_empty(),
        "trace should contain VariableName events"
    );

    // Verify specific variable names appear.
    for name in &["a", "b", "sum_val", "doubled", "final_result"] {
        assert!(
            var_names.contains(&name.to_string()),
            "variable '{}' should appear in trace, got names: {:?}",
            name,
            var_names
        );
    }

    // Verify variable values.
    let expected: &[(&str, i64)] = &[
        ("a", 10),
        ("b", 32),
        ("sum_val", 42),
        ("doubled", 84),
        ("final_result", 94),
    ];
    for (name, expected_val) in expected {
        let values = find_variable_values(&events, name);
        assert!(
            values.contains(expected_val),
            "variable '{}' should have value {}, got: {:?}",
            name,
            expected_val,
            values
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4: Verify Step events at correct source lines
// ---------------------------------------------------------------------------

#[test]
fn test_ndjson_step_events() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let step_events: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e.get("Step").is_some())
        .collect();

    assert!(
        step_events.len() >= 6,
        "should have at least 6 step events, got {}",
        step_events.len()
    );

    // Verify step events have valid structure.
    for event in &step_events {
        let step = event.get("Step").unwrap();
        assert!(step.get("path_id").is_some(), "Step should have path_id");
        let line = step["line"].as_i64().expect("Step line should be an integer");
        assert!(line > 0, "Step line should be positive, got {}", line);
    }

    let step_lines: Vec<i64> = step_events
        .iter()
        .map(|e| e.get("Step").unwrap()["line"].as_i64().unwrap())
        .collect();

    // Lines from compute() body: 2, 3, 4, 5, 6, 7.
    for line in &[2, 3, 4, 5, 6, 7] {
        assert!(
            step_lines.contains(line),
            "step events should include line {}, got lines: {:?}",
            line,
            step_lines
        );
    }
}

// ---------------------------------------------------------------------------
// Test 5: Verify Call/Return events
// ---------------------------------------------------------------------------

#[test]
fn test_ndjson_function_calls() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let call_count = events.iter().filter(|e| e.get("Call").is_some()).count();
    assert!(
        call_count >= 2,
        "trace should contain at least 2 Call events (main + compute), got {}",
        call_count
    );

    let return_count = events.iter().filter(|e| e.get("Return").is_some()).count();
    assert!(
        return_count >= 2,
        "trace should contain at least 2 Return events, got {}",
        return_count
    );

    // The last Return event should be after the last Step.
    let last_return_idx = events
        .iter()
        .rposition(|e| e.get("Return").is_some())
        .expect("should have a Return event");

    let steps_after_return = events[last_return_idx + 1..]
        .iter()
        .filter(|e| e.get("Step").is_some())
        .count();
    assert_eq!(
        steps_after_return, 0,
        "no Step events should appear after the final Return"
    );
}

// ---------------------------------------------------------------------------
// Test 6: Verify metadata JSON structure
// ---------------------------------------------------------------------------

#[test]
fn test_ndjson_metadata_structure() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = test_programs_dir().join("flow_test.cdc");

    run_tracer_from_ndjson(flow_test_ndjson(), &source_path, &out_dir);

    let metadata = load_trace_metadata(&out_dir);

    assert!(
        metadata.get("program").is_some(),
        "metadata should have 'program' field"
    );
    assert!(
        metadata["program"].is_string(),
        "metadata 'program' should be a string"
    );
    let program_str = metadata["program"].as_str().unwrap();
    assert!(
        program_str.contains("flow_test.cdc"),
        "metadata 'program' should reference flow_test.cdc, got: {}",
        program_str
    );

    assert!(
        metadata.get("args").is_some(),
        "metadata should have 'args' field"
    );
    assert!(
        metadata["args"].is_array(),
        "metadata 'args' should be an array"
    );

    assert!(
        metadata.get("workdir").is_some(),
        "metadata should have 'workdir' field"
    );
    assert!(
        metadata["workdir"].is_string(),
        "metadata 'workdir' should be a string"
    );
}

// ===========================================================================
// Go-helper integration tests (require the cadence-trace-helper binary)
//
// These tests are marked #[ignore] and can be run with:
//   cargo test -- --ignored
//
// Before running, build the Go helper:
//   cd go-helper && go build -o cadence-trace-helper .
//
// Then either put it on your $PATH or set CADENCE_HELPER_BIN:
//   CADENCE_HELPER_BIN=./go-helper/cadence-trace-helper cargo test -- --ignored
// ===========================================================================

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_compile_and_run() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    for filename in &["trace.bin", "trace_metadata.json", "trace_paths.json"] {
        let path = out_dir.join(filename);
        assert!(path.exists(), "{} should exist", filename);
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 0, "{} should be non-empty", filename);
    }

    let events = load_trace_events(&out_dir);
    assert!(!events.is_empty(), "trace should have at least one event");

    let step_count = events.iter().filter(|e| e.get("Step").is_some()).count();
    assert!(step_count > 0, "trace should contain at least one Step event");
}

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_compute_value() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let int_values = collect_int_values(&events);
    let all_values: Vec<i64> = int_values.iter().map(|(_, v)| *v).collect();

    assert!(
        all_values.contains(&94),
        "trace should contain value 94, got values: {:?}",
        all_values
    );
}

#[test]
#[ignore = "requires Go helper binary (cadence-trace-helper); run with --ignored"]
fn test_go_helper_variable_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = test_programs_dir().join("flow_test.cdc");
    run_tracer_on_file(&source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let var_names = collect_variable_names(&events);
    for name in &["a", "b", "sum_val", "doubled", "final_result"] {
        assert!(
            var_names.contains(&name.to_string()),
            "variable '{}' should appear in trace, got names: {:?}",
            name,
            var_names
        );
    }

    let expected: &[(&str, i64)] = &[
        ("a", 10),
        ("b", 32),
        ("sum_val", 42),
        ("doubled", 84),
        ("final_result", 94),
    ];
    for (name, expected_val) in expected {
        let values = find_variable_values(&events, name);
        assert!(
            values.contains(expected_val),
            "variable '{}' should have value {}, got: {:?}",
            name,
            expected_val,
            values
        );
    }
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

    assert!(out_dir.join("trace.bin").exists());
    assert!(out_dir.join("trace_metadata.json").exists());
    assert!(out_dir.join("trace_paths.json").exists());

    let events = load_trace_events(&out_dir);
    assert!(!events.is_empty(), "CLI trace should have events");

    let int_values = collect_int_values(&events);
    let all_values: Vec<i64> = int_values.iter().map(|(_, v)| *v).collect();
    assert!(
        all_values.contains(&94),
        "CLI trace should contain value 94, got values: {:?}",
        all_values
    );
}
