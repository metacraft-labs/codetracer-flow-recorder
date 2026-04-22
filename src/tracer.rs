//! Tracer implementation for Cadence programs.
//!
//! Shells out to a Go helper binary that uses the real Cadence runtime to
//! execute programs, then parses the NDJSON trace output and emits CodeTracer
//! trace events (steps, calls, returns, variables).
//!
//! The Go helper binary path is controlled by the `CADENCE_HELPER_BIN`
//! environment variable.  When unset it defaults to `cadence-trace-helper`
//! (looked up on `$PATH`).

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use codetracer_trace_types::{Line, TypeKind, ValueRecord, NONE_VALUE};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{create_trace_writer, TraceEventsFileFormat};
use eyre::{eyre, Context, Result};
use serde::Deserialize;

// source_map is available for future use with Go helper position mapping

// ---------------------------------------------------------------------------
// NDJSON trace event types (emitted by the Go helper)
// ---------------------------------------------------------------------------

/// A single trace event from the Go helper's NDJSON output.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum TraceEvent {
    #[serde(rename = "step")]
    Step { file: String, line: u32 },
    #[serde(rename = "variable")]
    Variable {
        name: String,
        value: String,
        #[serde(default)]
        cadence_type: Option<String>,
    },
    #[serde(rename = "call")]
    Call { name: String },
    #[serde(rename = "return")]
    Return {
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        cadence_type: Option<String>,
    },

    // ----- Resource lifecycle events (M4) -----
    #[serde(rename = "resource_create")]
    ResourceCreate {
        resource_type: String,
        uuid: u64,
        owner: String,
        file: String,
        line: u32,
    },

    #[serde(rename = "resource_move")]
    ResourceMove {
        resource_type: String,
        uuid: u64,
        from_owner: String,
        to_owner: String,
        file: String,
        line: u32,
    },

    #[serde(rename = "resource_destroy")]
    ResourceDestroy {
        resource_type: String,
        uuid: u64,
        owner: String,
        file: String,
        line: u32,
    },
}

// ---------------------------------------------------------------------------
// NDJSON parsing
// ---------------------------------------------------------------------------

/// Parse NDJSON text (one JSON object per line) into a vector of trace events.
///
/// Blank lines are silently skipped.  Parsing errors are reported with the
/// 1-based line number that failed.
pub fn parse_ndjson(text: &str) -> Result<Vec<TraceEvent>> {
    let mut events = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let event: TraceEvent = serde_json::from_str(line).with_context(|| {
            format!(
                "failed to parse NDJSON trace event at line {}: {}",
                i + 1,
                line
            )
        })?;
        events.push(event);
    }
    Ok(events)
}

// ---------------------------------------------------------------------------
// Go helper invocation
// ---------------------------------------------------------------------------

/// Default binary name for the Go helper.
const DEFAULT_HELPER_BIN: &str = "cadence-trace-helper";

/// Environment variable to override the Go helper binary path.
const HELPER_BIN_ENV: &str = "CADENCE_HELPER_BIN";

/// Run the Go helper binary on a Cadence source file and return the parsed
/// NDJSON trace events.
pub fn run_go_helper(source_path: &Path) -> Result<Vec<TraceEvent>> {
    let helper_bin = std::env::var(HELPER_BIN_ENV).unwrap_or_else(|_| {
        // Fall back to the binary built by build.rs (if available).
        option_env!("CADENCE_HELPER_BIN_BUILT")
            .unwrap_or(DEFAULT_HELPER_BIN)
            .to_string()
    });

    let output = Command::new(&helper_bin)
        .arg(source_path)
        .output()
        .with_context(|| {
            format!(
                "failed to run Cadence helper binary '{}'. \
                 Make sure the Go helper is built and either:\n  \
                 - it is on your $PATH as '{}', or\n  \
                 - the {} environment variable points to the binary",
                helper_bin, DEFAULT_HELPER_BIN, HELPER_BIN_ENV
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "Cadence helper '{}' exited with status {}: {}",
            helper_bin,
            output.status,
            stderr.trim()
        ));
    }

    let stdout =
        String::from_utf8(output.stdout).with_context(|| "Go helper produced non-UTF-8 output")?;

    parse_ndjson(&stdout)
}

// ---------------------------------------------------------------------------
// The main tracer
// ---------------------------------------------------------------------------

/// The main tracer struct that captures Cadence execution traces.
pub struct CadenceTracer {
    writer: Box<dyn TraceWriter + Send>,
    /// Registered type IDs for Cadence types.
    type_ids: HashMap<String, codetracer_trace_types::TypeId>,
}

impl CadenceTracer {
    /// Trace a Cadence program and write CodeTracer output files.
    ///
    /// 1. Shells out to the Go helper to execute the program and capture
    ///    NDJSON trace events.
    /// 2. Converts the trace events into CodeTracer format.
    /// 3. Writes trace.json/trace.bin (depending on format), trace_metadata.json, trace_paths.json.
    pub fn trace_program(
        source_path: &Path,
        _source_code: &str,
        out_dir: &Path,
        format: TraceEventsFileFormat,
    ) -> Result<()> {
        // -- 1. Run the Go helper --
        let events = run_go_helper(source_path)?;

        eprintln!("Got {} trace events from Go helper", events.len());

        // -- 2. Create the trace writer --
        let program_str = source_path.to_string_lossy();
        let mut tracer = CadenceTracer {
            writer: create_trace_writer(&program_str, &[], format),
            type_ids: HashMap::new(),
        };

        // -- 3. Initialise output files --
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        let events_filename = match format {
            TraceEventsFileFormat::Json => "trace.json",
            TraceEventsFileFormat::Binary
            | TraceEventsFileFormat::BinaryV0
            | TraceEventsFileFormat::Ctfs => "trace.bin",
        };
        let events_path = out_dir.join(events_filename);
        let metadata_path = out_dir.join("trace_metadata.json");
        let paths_path = out_dir.join("trace_paths.json");

        TraceWriter::begin_writing_trace_events(&mut *tracer.writer, &events_path)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::begin_writing_trace_metadata(&mut *tracer.writer, &metadata_path)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::begin_writing_trace_paths(&mut *tracer.writer, &paths_path)
            .map_err(|e| eyre!("{e}"))?;

        // -- 4. Start the trace --
        TraceWriter::start(&mut *tracer.writer, source_path, Line(1));

        // Register common Cadence types.
        for type_name in &["Int", "UInt64", "Fix64", "Bool", "String", "Address"] {
            let type_id =
                TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Int, type_name);
            tracer.type_ids.insert(type_name.to_string(), type_id);
        }

        // -- 5. Convert NDJSON events to CodeTracer events --
        tracer.convert_events(source_path, &events)?;

        // -- 6. Finish writing --
        TraceWriter::finish_writing_trace_events(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_metadata(&mut *tracer.writer)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_paths(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;

        Ok(())
    }

    /// Trace a Cadence program from pre-parsed NDJSON events.
    ///
    /// This is used for testing without the Go helper binary.
    pub fn trace_program_from_events(
        source_path: &Path,
        events: &[TraceEvent],
        out_dir: &Path,
        format: TraceEventsFileFormat,
    ) -> Result<()> {
        eprintln!("Processing {} trace events", events.len());

        // Create the trace writer.
        let program_str = source_path.to_string_lossy();
        let mut tracer = CadenceTracer {
            writer: create_trace_writer(&program_str, &[], format),
            type_ids: HashMap::new(),
        };

        // Initialise output files.
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        let events_filename = match format {
            TraceEventsFileFormat::Json => "trace.json",
            TraceEventsFileFormat::Binary
            | TraceEventsFileFormat::BinaryV0
            | TraceEventsFileFormat::Ctfs => "trace.bin",
        };
        let events_path = out_dir.join(events_filename);
        let metadata_path = out_dir.join("trace_metadata.json");
        let paths_path = out_dir.join("trace_paths.json");

        TraceWriter::begin_writing_trace_events(&mut *tracer.writer, &events_path)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::begin_writing_trace_metadata(&mut *tracer.writer, &metadata_path)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::begin_writing_trace_paths(&mut *tracer.writer, &paths_path)
            .map_err(|e| eyre!("{e}"))?;

        // Start the trace.
        TraceWriter::start(&mut *tracer.writer, source_path, Line(1));

        // Register common Cadence types.
        for type_name in &["Int", "UInt64", "Fix64", "Bool", "String", "Address"] {
            let type_id =
                TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Int, type_name);
            tracer.type_ids.insert(type_name.to_string(), type_id);
        }

        // Convert events.
        tracer.convert_events(source_path, events)?;

        // Finish writing.
        TraceWriter::finish_writing_trace_events(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_metadata(&mut *tracer.writer)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_paths(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;

        Ok(())
    }

    /// Convert NDJSON trace events into CodeTracer trace writer calls.
    fn convert_events(&mut self, source_path: &Path, events: &[TraceEvent]) -> Result<()> {
        for event in events {
            match event {
                TraceEvent::Step { file: _, line } => {
                    TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));
                }
                TraceEvent::Variable {
                    name,
                    value,
                    cadence_type,
                } => {
                    let type_name = cadence_type.as_deref().unwrap_or("Int");
                    let type_id = self
                        .type_ids
                        .get(type_name)
                        .copied()
                        .unwrap_or_else(|| self.type_ids.get("Int").copied().unwrap());

                    // Try to parse the value as an integer.
                    let val_record = if let Ok(i) = value.parse::<i64>() {
                        ValueRecord::Int { i, type_id }
                    } else {
                        // Fall back to string representation.
                        ValueRecord::String {
                            text: value.clone(),
                            type_id,
                        }
                    };

                    TraceWriter::register_variable_with_full_value(
                        &mut *self.writer,
                        name,
                        val_record,
                    );
                }
                TraceEvent::Call { name } => {
                    let fn_id = TraceWriter::ensure_function_id(
                        &mut *self.writer,
                        name,
                        source_path,
                        Line(1),
                    );
                    TraceWriter::register_call(&mut *self.writer, fn_id, vec![]);
                }
                TraceEvent::Return {
                    value,
                    cadence_type,
                } => {
                    let type_name = cadence_type.as_deref().unwrap_or("Int");
                    let type_id = self
                        .type_ids
                        .get(type_name)
                        .copied()
                        .unwrap_or_else(|| self.type_ids.get("Int").copied().unwrap());

                    match value.as_deref() {
                        None | Some("") | Some("nil") | Some("Void") => {
                            TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
                        }
                        Some(v) => {
                            if let Ok(i) = v.parse::<i64>() {
                                let val = ValueRecord::Int { i, type_id };
                                TraceWriter::register_return(&mut *self.writer, val);
                            } else {
                                let val = ValueRecord::String {
                                    text: v.to_string(),
                                    type_id,
                                };
                                TraceWriter::register_return(&mut *self.writer, val);
                            }
                        }
                    }
                }

                // --- Resource lifecycle events (M4) ---
                TraceEvent::ResourceCreate {
                    resource_type,
                    uuid,
                    owner,
                    file: _,
                    line,
                } => {
                    // Emit a step at the resource creation site.
                    TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                    // Emit the resource as a variable with special naming convention.
                    let var_name = format!("@resource:{}#{}", resource_type, uuid);
                    let type_id = self.ensure_resource_type(resource_type);
                    let val = ValueRecord::String {
                        text: format!("created(owner={})", owner),
                        type_id,
                    };
                    TraceWriter::register_variable_with_full_value(
                        &mut *self.writer,
                        &var_name,
                        val,
                    );
                }

                TraceEvent::ResourceMove {
                    resource_type,
                    uuid,
                    from_owner,
                    to_owner,
                    file: _,
                    line,
                } => {
                    // Emit a step at the move site.
                    TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                    // Emit the resource as a variable showing the ownership transfer.
                    let var_name = format!("@resource:{}#{}", resource_type, uuid);
                    let type_id = self.ensure_resource_type(resource_type);
                    let val = ValueRecord::String {
                        text: format!("moved({} -> {})", from_owner, to_owner),
                        type_id,
                    };
                    TraceWriter::register_variable_with_full_value(
                        &mut *self.writer,
                        &var_name,
                        val,
                    );
                }

                TraceEvent::ResourceDestroy {
                    resource_type,
                    uuid,
                    owner,
                    file: _,
                    line,
                } => {
                    // Emit a step at the destroy site.
                    TraceWriter::register_step(&mut *self.writer, source_path, Line(*line as i64));

                    // Emit the resource as a variable showing destruction.
                    let var_name = format!("@resource:{}#{}", resource_type, uuid);
                    let type_id = self.ensure_resource_type(resource_type);
                    let val = ValueRecord::String {
                        text: format!("destroyed(owner={})", owner),
                        type_id,
                    };
                    TraceWriter::register_variable_with_full_value(
                        &mut *self.writer,
                        &var_name,
                        val,
                    );
                }
            }
        }

        Ok(())
    }

    /// Ensure we have a type ID registered for a resource type.
    fn ensure_resource_type(&mut self, resource_type: &str) -> codetracer_trace_types::TypeId {
        let key = format!("@Resource:{}", resource_type);
        if let Some(&id) = self.type_ids.get(&key) {
            return id;
        }
        let type_id = TraceWriter::ensure_type_id(
            &mut *self.writer,
            TypeKind::Int, // Using Int kind as a placeholder for resource types
            &key,
        );
        self.type_ids.insert(key, type_id);
        type_id
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // NDJSON parsing tests (no Go helper needed)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_ndjson_step() {
        let input = r#"{"type":"step","file":"test.cdc","line":3}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Step {
                file: "test.cdc".to_string(),
                line: 3,
            }
        );
    }

    #[test]
    fn test_parse_ndjson_variable() {
        let input = r#"{"type":"variable","name":"a","value":"10","cadence_type":"Int"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Variable {
                name: "a".to_string(),
                value: "10".to_string(),
                cadence_type: Some("Int".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_ndjson_variable_no_type() {
        let input = r#"{"type":"variable","name":"x","value":"42"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Variable {
                name: "x".to_string(),
                value: "42".to_string(),
                cadence_type: None,
            }
        );
    }

    #[test]
    fn test_parse_ndjson_call() {
        let input = r#"{"type":"call","name":"compute"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Call {
                name: "compute".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_ndjson_return() {
        let input = r#"{"type":"return","value":"94","cadence_type":"Int"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Return {
                value: Some("94".to_string()),
                cadence_type: Some("Int".to_string()),
            }
        );
    }

    #[test]
    fn test_parse_ndjson_multiple_events() {
        let input = r#"{"type":"call","name":"main"}
{"type":"step","file":"test.cdc","line":2}
{"type":"variable","name":"a","value":"10","cadence_type":"Int"}
{"type":"step","file":"test.cdc","line":3}
{"type":"variable","name":"b","value":"32","cadence_type":"Int"}
{"type":"step","file":"test.cdc","line":4}
{"type":"variable","name":"sum_val","value":"42","cadence_type":"Int"}
{"type":"step","file":"test.cdc","line":5}
{"type":"variable","name":"doubled","value":"84","cadence_type":"Int"}
{"type":"step","file":"test.cdc","line":6}
{"type":"variable","name":"final_result","value":"94","cadence_type":"Int"}
{"type":"step","file":"test.cdc","line":7}
{"type":"return","value":"94","cadence_type":"Int"}"#;

        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 13);

        // First event is a call.
        assert_eq!(
            events[0],
            TraceEvent::Call {
                name: "main".to_string(),
            }
        );

        // Last event is a return.
        assert_eq!(
            events[12],
            TraceEvent::Return {
                value: Some("94".to_string()),
                cadence_type: Some("Int".to_string()),
            }
        );

        // Check variable values.
        let variables: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                TraceEvent::Variable { name, value, .. } => Some((name.as_str(), value.as_str())),
                _ => None,
            })
            .collect();

        assert_eq!(variables.len(), 5);
        assert_eq!(variables[0], ("a", "10"));
        assert_eq!(variables[1], ("b", "32"));
        assert_eq!(variables[2], ("sum_val", "42"));
        assert_eq!(variables[3], ("doubled", "84"));
        assert_eq!(variables[4], ("final_result", "94"));
    }

    #[test]
    fn test_parse_ndjson_blank_lines() {
        let input = r#"{"type":"step","file":"test.cdc","line":1}

{"type":"step","file":"test.cdc","line":2}

"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn test_parse_ndjson_empty() {
        let events = parse_ndjson("").unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_parse_ndjson_invalid_json() {
        let result = parse_ndjson("not valid json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_ndjson_unknown_type() {
        let result = parse_ndjson(r#"{"type":"unknown_event","data":"foo"}"#);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Resource lifecycle event parsing tests (M4)
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_ndjson_resource_create() {
        let input = r#"{"type":"resource_create","resource_type":"FlowToken.Vault","uuid":1001,"owner":"0x01","file":"test.cdc","line":5}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::ResourceCreate {
                resource_type: "FlowToken.Vault".to_string(),
                uuid: 1001,
                owner: "0x01".to_string(),
                file: "test.cdc".to_string(),
                line: 5,
            }
        );
    }

    #[test]
    fn test_parse_ndjson_resource_move() {
        let input = r#"{"type":"resource_move","resource_type":"FlowToken.Vault","uuid":1001,"from_owner":"0x01","to_owner":"0x02","file":"test.cdc","line":10}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::ResourceMove {
                resource_type: "FlowToken.Vault".to_string(),
                uuid: 1001,
                from_owner: "0x01".to_string(),
                to_owner: "0x02".to_string(),
                file: "test.cdc".to_string(),
                line: 10,
            }
        );
    }

    #[test]
    fn test_parse_ndjson_resource_destroy() {
        let input = r#"{"type":"resource_destroy","resource_type":"FlowToken.Vault","uuid":1001,"owner":"0x02","file":"test.cdc","line":15}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::ResourceDestroy {
                resource_type: "FlowToken.Vault".to_string(),
                uuid: 1001,
                owner: "0x02".to_string(),
                file: "test.cdc".to_string(),
                line: 15,
            }
        );
    }

    // -----------------------------------------------------------------------
    // Full NDJSON-to-CodeTracer conversion tests (no Go helper needed)
    // -----------------------------------------------------------------------

    #[test]
    fn test_convert_ndjson_to_trace() {
        // Simulate the NDJSON that the Go helper would produce for flow_test.cdc.
        let ndjson = r#"{"type":"call","name":"main"}
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
{"type":"return","value":"94","cadence_type":"Int"}"#;

        let events = parse_ndjson(ndjson).unwrap();

        let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let out_dir = tmp_dir.path().join("traces");
        let source_path = std::path::PathBuf::from("flow_test.cdc");

        CadenceTracer::trace_program_from_events(
            &source_path,
            &events,
            &out_dir,
            TraceEventsFileFormat::Json,
        )
        .expect("trace_program_from_events should succeed");

        // Verify output files exist and are non-empty.
        for filename in &["trace.json", "trace_metadata.json", "trace_paths.json"] {
            let path = out_dir.join(filename);
            assert!(path.exists(), "{} should exist", filename);
            let size = std::fs::metadata(&path).unwrap().len();
            assert!(size > 0, "{} should be non-empty", filename);
        }

        // Parse trace events and verify content.
        let content = std::fs::read_to_string(out_dir.join("trace.json")).unwrap();
        let trace_events: serde_json::Value = serde_json::from_str(&content).unwrap();
        let trace_array = trace_events.as_array().unwrap();

        // Should have Step events.
        let step_count = trace_array
            .iter()
            .filter(|e| e.get("Step").is_some())
            .count();
        assert!(
            step_count >= 6,
            "should have at least 6 step events, got {}",
            step_count
        );

        // Should have Call events.
        let call_count = trace_array
            .iter()
            .filter(|e| e.get("Call").is_some())
            .count();
        assert!(
            call_count >= 2,
            "should have at least 2 Call events, got {}",
            call_count
        );

        // Should have Return events.
        let return_count = trace_array
            .iter()
            .filter(|e| e.get("Return").is_some())
            .count();
        assert!(
            return_count >= 2,
            "should have at least 2 Return events, got {}",
            return_count
        );

        // Verify that value 94 appears in the trace.
        let has_94 = trace_array.iter().any(|e| {
            if let Some(val) = e.get("Value") {
                if let Some(value) = val.get("value") {
                    if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                        return value.get("i").and_then(|v| v.as_i64()) == Some(94);
                    }
                }
            }
            false
        });
        assert!(has_94, "trace should contain value 94");

        // Verify variable names.
        let var_names: Vec<String> = trace_array
            .iter()
            .filter_map(|e| {
                e.get("VariableName")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(
            var_names.contains(&"a".to_string()),
            "should have variable 'a'"
        );
        assert!(
            var_names.contains(&"b".to_string()),
            "should have variable 'b'"
        );
        assert!(
            var_names.contains(&"sum_val".to_string()),
            "should have variable 'sum_val'"
        );
        assert!(
            var_names.contains(&"doubled".to_string()),
            "should have variable 'doubled'"
        );
        assert!(
            var_names.contains(&"final_result".to_string()),
            "should have variable 'final_result'"
        );
    }

    #[test]
    fn test_convert_variable_values() {
        let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"test.cdc","line":2}
{"type":"variable","name":"x","value":"42","cadence_type":"Int"}
{"type":"step","file":"test.cdc","line":3}
{"type":"variable","name":"msg","value":"hello","cadence_type":"String"}
{"type":"return","value":"42"}"#;

        let events = parse_ndjson(ndjson).unwrap();

        let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let out_dir = tmp_dir.path().join("traces");
        let source_path = std::path::PathBuf::from("test.cdc");

        CadenceTracer::trace_program_from_events(
            &source_path,
            &events,
            &out_dir,
            TraceEventsFileFormat::Json,
        )
        .expect("should succeed");

        let content = std::fs::read_to_string(out_dir.join("trace.json")).unwrap();
        let trace_events: serde_json::Value = serde_json::from_str(&content).unwrap();
        let trace_array = trace_events.as_array().unwrap();

        // Check Int value.
        let has_42 = trace_array.iter().any(|e| {
            if let Some(val) = e.get("Value") {
                if let Some(value) = val.get("value") {
                    if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                        return value.get("i").and_then(|v| v.as_i64()) == Some(42);
                    }
                }
            }
            false
        });
        assert!(has_42, "trace should contain Int value 42");

        // Check String value "hello".
        let has_hello = trace_array.iter().any(|e| {
            if let Some(val) = e.get("Value") {
                if let Some(value) = val.get("value") {
                    if value.get("kind").and_then(|k| k.as_str()) == Some("String") {
                        return value.get("text").and_then(|v| v.as_str()) == Some("hello");
                    }
                }
            }
            false
        });
        assert!(has_hello, "trace should contain String value 'hello'");
    }
}
