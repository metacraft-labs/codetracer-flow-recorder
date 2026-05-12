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

use codetracer_trace_types::{
    EventLogKind, FullValueRecord, Line, TraceLowLevelEvent, TypeKind, ValueRecord, NONE_VALUE,
};
use codetracer_trace_writer_nim::non_streaming_trace_writer::NonStreamingTraceWriter;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{create_trace_writer, StreamingValueEncoder, TraceEventsFileFormat};
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
    Call {
        name: String,
        #[serde(default)]
        args: Vec<TraceArg>,
    },
    #[serde(rename = "return")]
    Return {
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        cadence_type: Option<String>,
    },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "event")]
    Event { name: String, payload: String },

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

/// A helper-side call argument staged onto the next CodeTracer Call record.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct TraceArg {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub cadence_type: Option<String>,
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
    /// Reusable streaming encoder for typed leaf values (Bool, String,
    /// ...) that the writer's `register_variable_with_full_value`
    /// otherwise downgrades to `ValueRecord::Raw`.  Encoding the value
    /// to CBOR ourselves and routing it through `register_variable_cbor`
    /// preserves the typed `ValueRecord` variant tag end-to-end.
    streaming_encoder: StreamingValueEncoder,
}

impl CadenceTracer {
    /// Trace a Cadence program and write a CodeTracer CTFS bundle.
    ///
    /// 1. Shells out to the Go helper to execute the program and capture
    ///    NDJSON trace events.
    /// 2. Converts the trace events into CodeTracer format.
    /// 3. Writes a `.ct` CTFS multi-stream container plus
    ///    `trace_metadata.json` / `trace_paths.json` sidecars to `out_dir`.
    ///
    /// The output format is fixed to CTFS — see
    /// `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`.  Use
    /// `ct print` (from `codetracer-trace-format-nim`) to convert the
    /// produced bundle to JSON or other text forms.
    pub fn trace_program(source_path: &Path, _source_code: &str, out_dir: &Path) -> Result<()> {
        // CTFS-only.  Pre-2026-05-08 the recorder accepted a format
        // parameter (`TraceEventsFileFormat::{Json,Binary,Ctfs}`) and the
        // CLI exposed a `--format` flag.  The convention now mandates
        // CTFS exclusively.
        let format = TraceEventsFileFormat::Ctfs;

        // -- 1. Run the Go helper --
        let events = run_go_helper(source_path)?;

        eprintln!("Got {} trace events from Go helper", events.len());

        // -- 2. Create the trace writer --
        let program_str = source_path.to_string_lossy();
        let mut tracer = CadenceTracer {
            writer: create_trace_writer(&program_str, &[], format),
            type_ids: HashMap::new(),
            streaming_encoder: StreamingValueEncoder::new(),
        };

        // -- 3. Initialise output files --
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        // CTFS multi-stream container.
        let events_filename = "trace.bin";
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
        tracer.writer.close().map_err(|e| eyre!("{e}"))?;

        Ok(())
    }

    /// Trace a Cadence program from pre-parsed NDJSON events.
    ///
    /// This is used for testing without the Go helper binary.  Output is
    /// always written in the canonical CTFS multi-stream format
    /// (see `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`).
    pub fn trace_program_from_events(
        source_path: &Path,
        events: &[TraceEvent],
        out_dir: &Path,
    ) -> Result<()> {
        // CTFS-only.  Pre-2026-05-08 this helper accepted a format
        // parameter; the convention now mandates CTFS exclusively.
        let format = TraceEventsFileFormat::Ctfs;

        eprintln!("Processing {} trace events", events.len());

        // Create the trace writer.
        let program_str = source_path.to_string_lossy();
        let mut tracer = CadenceTracer {
            writer: create_trace_writer(&program_str, &[], format),
            type_ids: HashMap::new(),
            streaming_encoder: StreamingValueEncoder::new(),
        };

        // Initialise output files.
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        // CTFS multi-stream container.
        let events_filename = "trace.bin";
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
        tracer.writer.close().map_err(|e| eyre!("{e}"))?;

        Ok(())
    }

    /// Convert pre-parsed NDJSON events into inspectable low-level events.
    ///
    /// This library helper is used by focused audit tests that need to assert
    /// exact event payloads without depending on a platform-specific trace
    /// container reader.
    pub fn trace_low_level_events_from_events(
        source_path: &Path,
        events: &[TraceEvent],
    ) -> Result<Vec<TraceLowLevelEvent>> {
        let program_str = source_path.to_string_lossy();
        let mut tracer = CadenceTracer {
            writer: Box::new(NonStreamingTraceWriter::new(&program_str, &[])),
            type_ids: HashMap::new(),
            streaming_encoder: StreamingValueEncoder::new(),
        };

        TraceWriter::start(&mut *tracer.writer, source_path, Line(1));
        for type_name in &["Int", "UInt64", "Fix64", "Bool", "String", "Address"] {
            let type_id =
                TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Int, type_name);
            tracer.type_ids.insert(type_name.to_string(), type_id);
        }

        tracer.convert_events(source_path, events)?;
        Ok(tracer.writer.events().to_vec())
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
                    let val_record = self.value_record(value, cadence_type.as_deref());
                    self.register_typed_variable(name, val_record);
                }
                TraceEvent::Call { name, args } => {
                    let fn_id = TraceWriter::ensure_function_id(
                        &mut *self.writer,
                        name,
                        source_path,
                        Line(1),
                    );
                    let mut call_args: Vec<FullValueRecord> = Vec::with_capacity(args.len());
                    for arg in args {
                        let val_record = self.value_record(&arg.value, arg.cadence_type.as_deref());
                        let full_arg = self.register_typed_arg(&arg.name, val_record);
                        call_args.push(full_arg);
                    }
                    TraceWriter::register_call(&mut *self.writer, fn_id, call_args);
                }
                TraceEvent::Return {
                    value,
                    cadence_type,
                } => {
                    match value.as_deref() {
                        None | Some("") | Some("nil") | Some("Void") => {
                            TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
                        }
                        Some(v) => {
                            let val = self.value_record(v, cadence_type.as_deref());
                            self.register_typed_return(val);
                        }
                    }
                }
                TraceEvent::Error { message } => {
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::Error,
                        "CadenceRuntimeError",
                        message,
                    );
                }
                TraceEvent::Event { name, payload } => {
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::EvmEvent,
                        &format!("CadenceEvent:{}", name),
                        payload,
                    );
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
                    // Variable form survives in the locals pane.
                    let var_name = format!("@resource:{}#{}", resource_type, uuid);
                    let type_id = self.ensure_resource_type(resource_type);
                    let val = ValueRecord::String {
                        text: format!("created(owner={})", owner),
                        type_id,
                    };
                    self.register_typed_variable(&var_name, val);

                    // Route through the structured event log too so the multi-stream
                    // IO event reader captures the resource lifecycle, not just the
                    // locals pane.  Mirrors Move 1.46's External-effect routing.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!("CadenceResourceCreate:{}#{}", resource_type, uuid),
                        &format!("owner={}", owner),
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
                    self.register_typed_variable(&var_name, val);

                    // Structured-event mirror (see ResourceCreate above).
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!("CadenceResourceMove:{}#{}", resource_type, uuid),
                        &format!("from={} to={}", from_owner, to_owner),
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
                    self.register_typed_variable(&var_name, val);

                    // Structured-event mirror (see ResourceCreate above).
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!("CadenceResourceDestroy:{}#{}", resource_type, uuid),
                        &format!("owner={}", owner),
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

    /// Decode a NDJSON `value` string into a typed `ValueRecord`, using
    /// the helper-side `cadence_type` discriminator when present.
    ///
    /// Spec: every typed leaf MUST surface as its dedicated
    /// `ValueRecord` variant — `Bool` as `Bool { b }`, `String` as
    /// `String { text }`, integer types as `Int { i }`.  Falling back to
    /// `Raw` (or to `String` for Bools) violates
    /// `metacraft-specs/policies/recorder-test-requirements.md` §1.
    /// Only genuinely unrecognised types fall through to a stringified
    /// `Raw` representation — and even those should be tightened up as
    /// the recorder learns more Cadence types.
    fn value_record(&self, value: &str, cadence_type: Option<&str>) -> ValueRecord {
        let type_name = cadence_type.unwrap_or("Int");
        let type_id = self
            .type_ids
            .get(type_name)
            .copied()
            .unwrap_or_else(|| self.type_ids.get("Int").copied().unwrap());

        match cadence_type {
            // Cadence Bools come as the literal strings "true" / "false".
            Some("Bool") => match value {
                "true" => ValueRecord::Bool { b: true, type_id },
                "false" => ValueRecord::Bool { b: false, type_id },
                _ => ValueRecord::Raw {
                    r: value.to_string(),
                    type_id,
                },
            },
            // Cadence Strings round-trip verbatim.
            Some("String") => ValueRecord::String {
                text: value.to_string(),
                type_id,
            },
            // Numeric types parse to i64 when they fit (this is the same
            // set the previous implementation handled implicitly via the
            // `value.parse::<i64>()` branch).
            Some(t)
                if matches!(
                    t,
                    "Int"
                        | "Int8"
                        | "Int16"
                        | "Int32"
                        | "Int64"
                        | "UInt"
                        | "UInt8"
                        | "UInt16"
                        | "UInt32"
                        | "UInt64"
                        | "Word8"
                        | "Word16"
                        | "Word32"
                        | "Word64"
                ) =>
            {
                if let Ok(i) = value.parse::<i64>() {
                    ValueRecord::Int { i, type_id }
                } else {
                    ValueRecord::Raw {
                        r: value.to_string(),
                        type_id,
                    }
                }
            }
            // No (or unknown) `cadence_type` discriminator: best-effort
            // numeric parse, otherwise leave as a stringified Raw so the
            // ct-print decoder treats the bytes as opaque rather than as
            // a typed String.
            _ => {
                if let Ok(i) = value.parse::<i64>() {
                    ValueRecord::Int { i, type_id }
                } else {
                    ValueRecord::Raw {
                        r: value.to_string(),
                        type_id,
                    }
                }
            }
        }
    }

    /// Register a step-local variable, preserving the value's typed
    /// `ValueRecord` variant tag.
    ///
    /// The Nim writer's `register_variable_with_full_value` only
    /// special-cases `Int` / `Sequence` / `Tuple` / `Struct`; every
    /// other variant — including `Bool` and `String` — is downgraded to
    /// a stringified `Raw` payload that the reader cannot tell apart
    /// from a real `ValueRecord::Raw`.  We bypass that downgrade for
    /// the typed leaf variants by encoding the value to CBOR via the
    /// streaming encoder (which honours the variant tag) and routing
    /// the bytes through `register_variable_cbor`.
    fn register_typed_variable(&mut self, name: &str, value: ValueRecord) {
        match &value {
            ValueRecord::Bool { .. } | ValueRecord::String { .. } => {
                let cbor = self.streaming_encoder.encode(&value).to_vec();
                TraceWriter::register_variable_cbor(&mut *self.writer, name, &cbor);
            }
            _ => {
                TraceWriter::register_variable_with_full_value(&mut *self.writer, name, value);
            }
        }
    }

    /// Register a return value, preserving the typed `ValueRecord`
    /// variant tag.  Same rationale as `register_typed_variable` — the
    /// Nim writer's `register_return` downgrades non-Int leaves to
    /// stringified Raw, so we route Bool/String through the streaming
    /// encoder + `register_return_cbor` instead.
    fn register_typed_return(&mut self, value: ValueRecord) {
        match &value {
            ValueRecord::Bool { .. } | ValueRecord::String { .. } => {
                let cbor = self.streaming_encoder.encode(&value).to_vec();
                TraceWriter::register_return_cbor(&mut *self.writer, &cbor);
            }
            _ => {
                TraceWriter::register_return(&mut *self.writer, value);
            }
        }
    }

    /// Stage a typed call argument both as a step-local (so it appears
    /// in the locals pane for the call site) and on the call record's
    /// pending-args buffer (so the calltrace pane shows
    /// `f(name=value)`).
    ///
    /// Mirrors the writer's `arg()` helper but routes Bool / String
    /// through the typed-CBOR path so the variant tag survives the
    /// round-trip — see `register_typed_variable` for the rationale.
    fn register_typed_arg(&mut self, name: &str, value: ValueRecord) -> FullValueRecord {
        let cbor = self.streaming_encoder.encode(&value).to_vec();
        self.register_typed_variable(name, value.clone());
        TraceWriter::register_call_arg(&mut *self.writer, name, &cbor);
        FullValueRecord {
            variable_id: codetracer_trace_types::VariableId(0),
            value,
        }
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
                args: Vec::new(),
            }
        );
    }

    #[test]
    fn test_parse_ndjson_call_args() {
        let input = r#"{"type":"call","name":"add","args":[{"name":"x","value":"10","cadence_type":"Int"},{"name":"y","value":"20","cadence_type":"Int"}]}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Call {
                name: "add".to_string(),
                args: vec![
                    TraceArg {
                        name: "x".to_string(),
                        value: "10".to_string(),
                        cadence_type: Some("Int".to_string()),
                    },
                    TraceArg {
                        name: "y".to_string(),
                        value: "20".to_string(),
                        cadence_type: Some("Int".to_string()),
                    },
                ],
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
    fn test_parse_ndjson_error() {
        let input = r#"{"type":"error","message":"pre-condition failed"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Error {
                message: "pre-condition failed".to_string(),
            }
        );
    }

    #[test]
    fn test_parse_ndjson_event() {
        let input = r#"{"type":"event","name":"MyEvent","payload":"MyEvent(message: \"done\")"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            TraceEvent::Event {
                name: "MyEvent".to_string(),
                payload: "MyEvent(message: \"done\")".to_string(),
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
                args: Vec::new(),
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

        CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
            .expect("trace_program_from_events should succeed");

        // Verify .ct output with CTFS magic bytes.
        let ct_files: Vec<_> = std::fs::read_dir(&out_dir)
            .expect("read output dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
            .collect();
        assert!(
            !ct_files.is_empty(),
            "expected at least one .ct file in output dir"
        );
        let content = std::fs::read(&ct_files[0]).expect("read .ct file");
        assert!(content.len() >= 5, ".ct file too small");
        assert_eq!(
            &content[..5],
            &[0xC0u8, 0xDE, 0x72, 0xAC, 0xE2],
            "CTFS magic bytes mismatch"
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

        CadenceTracer::trace_program_from_events(&source_path, &events, &out_dir)
            .expect("should succeed");

        // Verify .ct output with CTFS magic bytes.
        let ct_files: Vec<_> = std::fs::read_dir(&out_dir)
            .expect("read output dir")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
            .collect();
        assert!(
            !ct_files.is_empty(),
            "expected at least one .ct file in output dir"
        );
        let ct_content = std::fs::read(&ct_files[0]).expect("read .ct file");
        assert!(ct_content.len() >= 5, ".ct file too small");
        assert_eq!(
            &ct_content[..5],
            &[0xC0u8, 0xDE, 0x72, 0xAC, 0xE2],
            "CTFS magic bytes mismatch"
        );
    }
}
