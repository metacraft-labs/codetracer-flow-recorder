//! CLI entry point for the CodeTracer Cadence/Flow recorder.
//!
//! Supports the `record` subcommand which takes a Cadence source file,
//! parses and evaluates variable assignments, and writes a CodeTracer
//! CTFS trace bundle.
//!
//! # Usage
//!
//! ```text
//! codetracer-flow-recorder record <cdc-file> --out-dir <output-dir>
//! ```
//!
//! The recorder always writes traces in the canonical CodeTracer multi-stream
//! CTFS format (see `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`).
//! No `--format` flag is exposed: human-readable conversion is handled
//! out-of-band by `ct print` (shipped with `codetracer-trace-format-nim`).
//!
//! # Environment variables
//!
//! * `CODETRACER_FLOW_RECORDER_OUT_DIR` — fallback for `--out-dir` when the
//!   flag is not given. The CLI flag always wins.
//! * `CODETRACER_FLOW_RECORDER_DISABLED` — set to `1` or `true` to skip
//!   recording entirely. The recorder still validates input but does not
//!   shell out to the Go helper or emit any trace artefacts.
//! * `CODETRACER_FLOW_RECORDER_LOG_LEVEL` — recorder log verbosity (advisory;
//!   the Flow recorder currently logs to stderr unconditionally).
//! * `CADENCE_HELPER_BIN` — path to the `cadence-trace-helper` Go binary
//!   (Flow-specific; not part of the standard recorder env-var set).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use eyre::{Context, Result};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Environment variable used as a fallback for `--out-dir` when the CLI
/// flag is omitted.  Convention: see `Recorder-CLI-Conventions.md` §5.
const ENV_OUT_DIR: &str = "CODETRACER_FLOW_RECORDER_OUT_DIR";

/// Environment variable that, when set to `1`/`true`, disables tracing
/// entirely — the recorder skips emitting any trace artefacts.
const ENV_DISABLED: &str = "CODETRACER_FLOW_RECORDER_DISABLED";

/// Default output directory used when neither `--out-dir` nor
/// `CODETRACER_FLOW_RECORDER_OUT_DIR` is set.
const DEFAULT_OUT_DIR: &str = "./ct-traces/";

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// CodeTracer Cadence/Flow recorder -- record Cadence smart contract execution traces.
///
/// Traces are always written in the canonical CTFS multi-stream format.
/// To convert a recorded `.ct` bundle to JSON / text for inspection, use
/// `ct print` from `codetracer-trace-format-nim`.
#[derive(Debug, Parser)]
#[command(
    name = "codetracer-flow-recorder",
    version,
    about = "Record Cadence smart contract execution traces for CodeTracer (CTFS-only). \
             Use `ct print` from codetracer-trace-format-nim for human-readable conversion.",
    long_about = "Record Cadence smart contract execution traces for CodeTracer.\n\
                  \n\
                  Output is always written in the canonical CodeTracer CTFS\n\
                  multi-stream format. Use `ct print` (shipped with the\n\
                  codetracer-trace-format-nim sibling) to convert a recorded\n\
                  `.ct` bundle to JSON or other human-readable forms.\n\
                  \n\
                  Environment variables:\n\
                    CODETRACER_FLOW_RECORDER_OUT_DIR    fallback for --out-dir\n\
                    CODETRACER_FLOW_RECORDER_DISABLED   set to 1/true to skip recording\n\
                    CODETRACER_FLOW_RECORDER_LOG_LEVEL  log verbosity (advisory)\n\
                    CADENCE_HELPER_BIN                  path to cadence-trace-helper Go binary"
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
    /// captures the execution trace, and writes a CTFS bundle to
    /// `--out-dir`.
    Record(RecordArgs),

    /// Replay a Flow on-chain transaction.
    ///
    /// Fetches the transaction from a Flow Access Node, extracts its
    /// Cadence script and arguments, replays execution through the
    /// Go tracer helper, and writes a CTFS bundle to `--out-dir`.
    Replay(ReplayArgs),

    /// Print version information.
    Version,
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
    ///
    /// Falls back to `CODETRACER_FLOW_RECORDER_OUT_DIR` when omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct RecordArgs {
    /// Path to the Cadence source (.cdc) file.
    program: PathBuf,

    /// Directory where the trace files will be written.
    ///
    /// The directory will be created if it does not exist.  Falls back to
    /// the `CODETRACER_FLOW_RECORDER_OUT_DIR` environment variable when the
    /// flag is omitted.
    #[arg(short = 'o', long)]
    out_dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the effective output directory:
///   1. `--out-dir` if given on the CLI.
///   2. `CODETRACER_FLOW_RECORDER_OUT_DIR` env var.
///   3. `DEFAULT_OUT_DIR` ("./ct-traces/").
fn resolve_out_dir(cli_out_dir: Option<PathBuf>) -> PathBuf {
    if let Some(path) = cli_out_dir {
        return path;
    }
    if let Some(value) = std::env::var_os(ENV_OUT_DIR) {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    PathBuf::from(DEFAULT_OUT_DIR)
}

/// Whether the recorder is disabled via env var.  When true, the CLI
/// must skip emitting any trace artefacts.
fn recording_disabled() -> bool {
    match std::env::var(ENV_DISABLED) {
        Ok(value) => {
            let v = value.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        }
        Err(_) => false,
    }
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

    if recording_disabled() {
        // Pass-through: the Flow recorder shells out to the Go helper.
        // When recording is disabled we skip the helper invocation
        // entirely and emit no trace artefacts.
        eprintln!("{ENV_DISABLED} is set; skipping trace recording (no output written).");
        return Ok(());
    }

    // 2. Resolve and create the output directory
    let out_dir = resolve_out_dir(args.out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    // 3. Run the recorder (CTFS only)
    codetracer_flow_recorder::recorder::record(&source_path, &out_dir)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}

// ---------------------------------------------------------------------------
// `replay` implementation
// ---------------------------------------------------------------------------

/// Execute the `replay` subcommand.
fn replay(args: ReplayArgs) -> Result<()> {
    let mut config =
        codetracer_flow_recorder::replay::ReplayConfig::new(&args.tx_hash, &args.access_node);
    if let Some(source_dir) = args.source_dir {
        config = config.with_source_dir(source_dir);
    }

    eprintln!(
        "Replaying transaction {} via {}",
        config.tx_hash, config.access_node_url
    );

    if recording_disabled() {
        eprintln!("{ENV_DISABLED} is set; skipping replay (no output written).");
        return Ok(());
    }

    let out_dir = resolve_out_dir(args.out_dir);
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

    codetracer_flow_recorder::replay::replay_transaction(&config, &out_dir)?;

    eprintln!("Trace files written to {}", out_dir.display());

    Ok(())
}
