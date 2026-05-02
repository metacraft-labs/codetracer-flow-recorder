//! CLI entry point for the CodeTracer Cadence/Flow recorder.
//!
//! Supports the `record` subcommand which takes a Cadence source file,
//! parses and evaluates variable assignments, and writes CodeTracer trace
//! output files.
//!
//! # Usage
//!
//! ```text
//! codetracer-flow-recorder record <cdc-file> \
//!     --out-dir <output-dir> \
//!     [--format ctfs|binary|json]
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use codetracer_trace_writer_nim::TraceEventsFileFormat;
use eyre::{Context, Result};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// CodeTracer Cadence/Flow recorder -- record Cadence smart contract execution traces.
#[derive(Debug, Parser)]
#[command(
    name = "codetracer-flow-recorder",
    version,
    about = "Record Cadence smart contract execution traces for CodeTracer"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Record execution of a Cadence program.
    ///
    /// Parses the given .cdc source file, evaluates variable assignments,
    /// captures the execution trace, and writes CodeTracer trace files
    /// to `--out-dir`.
    Record(RecordArgs),

    /// Replay a Flow on-chain transaction.
    ///
    /// Fetches the transaction from a Flow Access Node, extracts its
    /// Cadence script and arguments, replays execution through the
    /// Go tracer helper, and writes CodeTracer trace files to `--out-dir`.
    Replay(ReplayArgs),

    /// Print version information.
    Version,
}

/// Output format for the recorded trace.
///
/// `Ctfs` is the canonical CodeTracer multi-stream container — a single `.ct`
/// file consumed by the Nim `ct_reader_*` FFI and the db-backend's
/// `CTFSTraceReader`.  This is the recommended format and the default.
///
/// `Binary` is the legacy CBOR + Zstd events file (`trace.bin` +
/// `trace_metadata.json` + `trace_paths.json`); kept for backwards
/// compatibility with older readers that have not been migrated to CTFS.
///
/// `Json` is the human-readable variant of the legacy format; useful for
/// debugging the recorder itself.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    /// Canonical CodeTracer multi-stream container (recommended).
    Ctfs,
    /// Legacy CBOR + Zstd binary format.
    Binary,
    /// Human-readable JSON (slower; useful for debugging).
    Json,
}

impl From<OutputFormat> for TraceEventsFileFormat {
    fn from(f: OutputFormat) -> Self {
        match f {
            OutputFormat::Ctfs => TraceEventsFileFormat::Ctfs,
            OutputFormat::Binary => TraceEventsFileFormat::Binary,
            OutputFormat::Json => TraceEventsFileFormat::Json,
        }
    }
}

#[derive(Debug, clap::Args)]
struct ReplayArgs {
    /// Flow transaction hash to replay (hex, with or without 0x prefix).
    #[arg(long)]
    tx_hash: String,

    /// Flow Access Node gRPC endpoint.
    #[arg(long, default_value = codetracer_flow_recorder::replay::DEFAULT_ACCESS_NODE_URL)]
    access_node: String,

    /// Optional directory containing Cadence source files for source mapping.
    #[arg(long)]
    source_dir: Option<PathBuf>,

    /// Directory where the trace files will be written.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "ctfs")]
    format: OutputFormat,
}

#[derive(Debug, clap::Args)]
struct RecordArgs {
    /// Path to the Cadence source (.cdc) file.
    program: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// The directory will be created if it does not exist.
    #[arg(short = 'o', long, default_value = "./ct-traces/")]
    out_dir: PathBuf,

    /// Output format for the trace data.
    #[arg(short = 'f', long, default_value = "ctfs")]
    format: OutputFormat,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Record(args) => record(args),
        Commands::Replay(args) => replay(args),
        Commands::Version => {
            println!("codetracer-flow-recorder {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// `record` implementation
// ---------------------------------------------------------------------------

/// Execute the `record` subcommand.
fn record(args: RecordArgs) -> Result<()> {
    // 1. Validate the source file exists
    let source_path = args
        .program
        .canonicalize()
        .with_context(|| format!("source file not found: {}", args.program.display()))?;

    eprintln!("Source file: {}", source_path.display());

    let format: TraceEventsFileFormat = args.format.into();

    // 2. Create the output directory
    let out_dir = &args.out_dir;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 3. Run the recorder
    codetracer_flow_recorder::recorder::record(&source_path, out_dir, format)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// `replay` implementation
// ---------------------------------------------------------------------------

/// Execute the `replay` subcommand.
fn replay(args: ReplayArgs) -> Result<()> {
    let format: TraceEventsFileFormat = args.format.into();

    let mut config =
        codetracer_flow_recorder::replay::ReplayConfig::new(&args.tx_hash, &args.access_node);
    if let Some(source_dir) = args.source_dir {
        config = config.with_source_dir(source_dir);
    }

    eprintln!(
        "Replaying transaction {} via {}",
        config.tx_hash, config.access_node_url
    );

    let out_dir = &args.out_dir;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    codetracer_flow_recorder::replay::replay_transaction(&config, out_dir, format)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}
