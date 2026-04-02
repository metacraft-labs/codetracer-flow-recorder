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

use codetracer_flow_recorder::tracer::{parse_ndjson, CadenceTracer, TraceEvent};
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
    codetracer_flow_recorder::recorder::record(source_path, out_dir, TraceEventsFileFormat::Json)
        .expect("trace_program should succeed");
}

/// Parse the trace events JSON from the output directory.
fn load_trace_events(out_dir: &Path) -> Vec<serde_json::Value> {
    let events_path = out_dir.join("trace.json");
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
    for filename in &["trace.json", "trace_metadata.json", "trace_paths.json"] {
        let path = out_dir.join(filename);
        assert!(path.exists(), "{} should exist", filename);
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 0, "{} should be non-empty", filename);
    }

    // trace.json should be valid JSON containing an array of events.
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

    let step_events: Vec<&serde_json::Value> =
        events.iter().filter(|e| e.get("Step").is_some()).collect();

    assert!(
        step_events.len() >= 6,
        "should have at least 6 step events, got {}",
        step_events.len()
    );

    // Verify step events have valid structure.
    for event in &step_events {
        let step = event.get("Step").unwrap();
        assert!(step.get("path_id").is_some(), "Step should have path_id");
        let line = step["line"]
            .as_i64()
            .expect("Step line should be an integer");
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
// Resource lifecycle tests (M4 - always run, no Go helper needed)
// ===========================================================================

/// Build NDJSON trace with resource lifecycle events.
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

/// Build NDJSON trace with nested resource operations.
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

// ---------------------------------------------------------------------------
// Test 7: Parse resource lifecycle NDJSON events
// ---------------------------------------------------------------------------

#[test]
fn test_parse_resource_lifecycle_events() {
    let events = parse_ndjson(resource_lifecycle_ndjson()).expect("should parse resource NDJSON");

    // Count resource events.
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

// ---------------------------------------------------------------------------
// Test 8: Convert resource events to trace output
// ---------------------------------------------------------------------------

#[test]
fn test_convert_resource_events_to_trace() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("resource_test.cdc");

    run_tracer_from_ndjson(resource_lifecycle_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    // Should have variable names with the @resource: prefix.
    let var_names = collect_variable_names(&events);
    let resource_vars: Vec<&String> = var_names
        .iter()
        .filter(|n| n.starts_with("@resource:"))
        .collect();

    assert!(
        !resource_vars.is_empty(),
        "trace should contain @resource: variable names, got: {:?}",
        var_names
    );

    // Should contain FlowToken.Vault#1001.
    let has_vault = resource_vars
        .iter()
        .any(|n| n.contains("FlowToken.Vault#1001"));
    assert!(
        has_vault,
        "should have @resource:FlowToken.Vault#1001, got: {:?}",
        resource_vars
    );

    // Verify resource values contain lifecycle descriptions.
    let string_values: Vec<String> = events
        .iter()
        .filter_map(|e| {
            let val = e.get("Value")?;
            let value = val.get("value")?;
            if value.get("kind").and_then(|k| k.as_str()) == Some("String") {
                value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    assert!(
        string_values.iter().any(|v| v.contains("created")),
        "should have 'created' value, got: {:?}",
        string_values
    );
    assert!(
        string_values.iter().any(|v| v.contains("moved")),
        "should have 'moved' value, got: {:?}",
        string_values
    );
    assert!(
        string_values.iter().any(|v| v.contains("destroyed")),
        "should have 'destroyed' value, got: {:?}",
        string_values
    );
}

// ---------------------------------------------------------------------------
// Test 9: Resource events also produce Step events at the correct lines
// ---------------------------------------------------------------------------

#[test]
fn test_resource_events_produce_steps() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("resource_test.cdc");

    run_tracer_from_ndjson(resource_lifecycle_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let step_lines: Vec<i64> = events
        .iter()
        .filter_map(|e| {
            e.get("Step")
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_i64())
        })
        .collect();

    // Resource events at lines 5, 10, 15 should produce steps (in addition
    // to the explicit step events at those same lines from the NDJSON).
    for line in &[5, 10, 15] {
        assert!(
            step_lines.contains(line),
            "step events should include line {} (from resource event), got lines: {:?}",
            line,
            step_lines
        );
    }
}

// ---------------------------------------------------------------------------
// Test 10: Mixed resource and regular events don't break existing conversion
// ---------------------------------------------------------------------------

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

    let events = load_trace_events(&out_dir);

    // Regular variables should still work.
    let var_names = collect_variable_names(&events);
    assert!(
        var_names.contains(&"balance".to_string()),
        "should have 'balance' variable"
    );
    assert!(
        var_names.contains(&"amount".to_string()),
        "should have 'amount' variable"
    );

    // Resource variable should also appear.
    let resource_vars: Vec<&String> = var_names
        .iter()
        .filter(|n| n.starts_with("@resource:"))
        .collect();
    assert!(!resource_vars.is_empty(), "should have resource variables");

    // Regular int values should be present.
    let int_values = collect_int_values(&events);
    let all_ints: Vec<i64> = int_values.iter().map(|(_, v)| *v).collect();
    assert!(all_ints.contains(&100), "should have value 100");
    assert!(all_ints.contains(&50), "should have value 50");
}

// ---------------------------------------------------------------------------
// Test 11: Nested resource tracking
// ---------------------------------------------------------------------------

#[test]
fn test_nested_resource_tracking() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = PathBuf::from("nested_resource.cdc");

    run_tracer_from_ndjson(nested_resource_ndjson(), &source_path, &out_dir);

    let events = load_trace_events(&out_dir);

    let var_names = collect_variable_names(&events);
    let resource_vars: Vec<&String> = var_names
        .iter()
        .filter(|n| n.starts_with("@resource:"))
        .collect();

    // Should track multiple resource types and UUIDs.
    let has_collection = resource_vars
        .iter()
        .any(|n| n.contains("NFT.Collection#2001"));
    let has_token_2002 = resource_vars.iter().any(|n| n.contains("NFT.Token#2002"));
    let has_token_2003 = resource_vars.iter().any(|n| n.contains("NFT.Token#2003"));

    assert!(
        has_collection,
        "should track NFT.Collection#2001, got: {:?}",
        resource_vars
    );
    assert!(
        has_token_2002,
        "should track NFT.Token#2002, got: {:?}",
        resource_vars
    );
    assert!(
        has_token_2003,
        "should track NFT.Token#2003, got: {:?}",
        resource_vars
    );

    // Verify the lifecycle values appear in correct order for collection:
    // created, moved, destroyed.
    let string_values: Vec<String> = events
        .iter()
        .filter_map(|e| {
            let val = e.get("Value")?;
            let value = val.get("value")?;
            if value.get("kind").and_then(|k| k.as_str()) == Some("String") {
                value
                    .get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect();

    // Count lifecycle events.
    let created_count = string_values
        .iter()
        .filter(|v| v.contains("created"))
        .count();
    let moved_count = string_values.iter().filter(|v| v.contains("moved")).count();
    let destroyed_count = string_values
        .iter()
        .filter(|v| v.contains("destroyed"))
        .count();

    assert_eq!(
        created_count, 3,
        "should have 3 created events (collection + 2 tokens)"
    );
    assert_eq!(
        moved_count, 2,
        "should have 2 moved events (collection + token)"
    );
    assert_eq!(
        destroyed_count, 2,
        "should have 2 destroyed events (token + collection)"
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

    for filename in &["trace.json", "trace_metadata.json", "trace_paths.json"] {
        let path = out_dir.join(filename);
        assert!(path.exists(), "{} should exist", filename);
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 0, "{} should be non-empty", filename);
    }

    let events = load_trace_events(&out_dir);
    assert!(!events.is_empty(), "trace should have at least one event");

    let step_count = events.iter().filter(|e| e.get("Step").is_some()).count();
    assert!(
        step_count > 0,
        "trace should contain at least one Step event"
    );
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

    assert!(out_dir.join("trace.json").exists());
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
