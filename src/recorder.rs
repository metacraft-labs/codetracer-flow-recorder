//! Recording logic for Cadence execution traces.
//!
//! This module provides the top-level `record` function that shells out to the
//! Go helper binary (which uses the real Cadence runtime) and converts the
//! resulting NDJSON trace into CodeTracer output files.

use std::path::Path;

use codetracer_trace_writer_nim::TraceEventsFileFormat;
use eyre::Result;

use crate::tracer::CadenceTracer;

/// Record a Cadence execution trace.
///
/// Runs the Go helper binary on the Cadence source file at `source_path`,
/// parses the NDJSON trace output, and writes CodeTracer trace files to
/// `out_dir`.
pub fn record(source_path: &Path, out_dir: &Path, format: TraceEventsFileFormat) -> Result<()> {
    let source_code = std::fs::read_to_string(source_path).map_err(|e| {
        eyre::eyre!(
            "failed to read source file {}: {}",
            source_path.display(),
            e
        )
    })?;

    CadenceTracer::trace_program(source_path, &source_code, out_dir, format)
}
