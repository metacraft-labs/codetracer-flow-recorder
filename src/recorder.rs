//! Recording logic for Cadence execution traces.
//!
//! This module provides the top-level `record` function that shells out to the
//! Go helper binary (which uses the real Cadence runtime) and converts the
//! resulting NDJSON trace into a CodeTracer CTFS bundle.

use std::path::Path;

use eyre::Result;

use crate::tracer::CadenceTracer;

/// Record a Cadence execution trace.
///
/// Runs the Go helper binary on the Cadence source file at `source_path`,
/// parses the NDJSON trace output, and writes a CTFS bundle to `out_dir`.
///
/// The output format is fixed to CTFS — see
/// `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`.  Use
/// `ct print` (from `codetracer-trace-format-nim`) for human-readable
/// conversion of the produced bundle.
pub fn record(source_path: &Path, out_dir: &Path) -> Result<()> {
    let source_code = std::fs::read_to_string(source_path).map_err(|e| {
        eyre::eyre!(
            "failed to read source file {}: {}",
            source_path.display(),
            e
        )
    })?;

    CadenceTracer::trace_program(source_path, &source_code, out_dir)
}
