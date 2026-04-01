//! On-chain transaction replay for Flow (M5).
//!
//! Fetches a transaction from a Flow Access Node, extracts its Cadence script
//! and arguments, runs it through the Go tracer helper in replay mode, and
//! converts the NDJSON output into CodeTracer trace files.

use std::path::{Path, PathBuf};
use std::process::Command;

use codetracer_trace_writer::TraceEventsFileFormat;
use eyre::{eyre, Context, Result};
use serde::{Deserialize, Serialize};

use crate::tracer::{parse_ndjson, CadenceTracer};

// ---------------------------------------------------------------------------
// Configuration and data types
// ---------------------------------------------------------------------------

/// Default Flow Access API endpoint (mainnet).
pub const DEFAULT_ACCESS_NODE_URL: &str = "access.mainnet.nodes.onflow.org:9000";

/// Client for the Flow Access API.
#[derive(Debug, Clone)]
pub struct FlowAccessClient {
    /// gRPC endpoint of the Flow Access Node.
    pub access_node_url: String,
}

impl FlowAccessClient {
    /// Create a new client pointing at the given Access Node URL.
    pub fn new(url: &str) -> Self {
        Self {
            access_node_url: url.to_string(),
        }
    }
}

impl Default for FlowAccessClient {
    fn default() -> Self {
        Self::new(DEFAULT_ACCESS_NODE_URL)
    }
}

/// Configuration for replaying a Flow transaction.
#[derive(Debug, Clone)]
pub struct ReplayConfig {
    /// Transaction hash to replay (hex-encoded, with or without 0x prefix).
    pub tx_hash: String,
    /// Flow Access Node URL.
    pub access_node_url: String,
    /// Optional directory containing Cadence source files for source mapping.
    pub source_dir: Option<PathBuf>,
}

impl ReplayConfig {
    /// Create a new replay configuration.
    pub fn new(tx_hash: &str, access_node_url: &str) -> Self {
        Self {
            tx_hash: normalise_tx_hash(tx_hash),
            access_node_url: access_node_url.to_string(),
            source_dir: None,
        }
    }

    /// Set the source directory.
    pub fn with_source_dir(mut self, dir: PathBuf) -> Self {
        self.source_dir = Some(dir);
        self
    }
}

/// A transaction's Cadence script and its JSON-CDC encoded arguments,
/// as fetched from the Access API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransactionScript {
    /// The Cadence source code of the transaction.
    pub script: String,
    /// JSON-CDC encoded arguments (each element is a JSON string).
    pub arguments: Vec<String>,
    /// The transaction hash this was fetched from.
    pub tx_hash: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalise a transaction hash: strip optional "0x" prefix, lowercase.
fn normalise_tx_hash(hash: &str) -> String {
    hash.strip_prefix("0x").unwrap_or(hash).to_ascii_lowercase()
}

/// Default binary name for the Go helper.
const DEFAULT_HELPER_BIN: &str = "cadence-trace-helper";

/// Environment variable to override the Go helper binary path.
const HELPER_BIN_ENV: &str = "CADENCE_HELPER_BIN";

// ---------------------------------------------------------------------------
// Replay pipeline
// ---------------------------------------------------------------------------

/// Replay a Flow transaction and write CodeTracer trace output.
///
/// Pipeline:
/// 1. Invoke the Go helper in `replay` mode with the transaction hash and
///    access node URL. The helper fetches the transaction, executes its
///    Cadence script through the emulator in fork mode, and emits NDJSON
///    trace events on stdout.
/// 2. Parse the NDJSON output.
/// 3. Write CodeTracer trace files to `out_dir`.
pub fn replay_transaction(
    config: &ReplayConfig,
    out_dir: &Path,
    format: TraceEventsFileFormat,
) -> Result<()> {
    let helper_bin =
        std::env::var(HELPER_BIN_ENV).unwrap_or_else(|_| DEFAULT_HELPER_BIN.to_string());

    let mut cmd = Command::new(&helper_bin);
    cmd.arg("replay")
        .arg("--tx-hash")
        .arg(&config.tx_hash)
        .arg("--access-node")
        .arg(&config.access_node_url);

    if let Some(ref source_dir) = config.source_dir {
        cmd.arg("--source-dir").arg(source_dir);
    }

    let output = cmd.output().with_context(|| {
        format!(
            "failed to run Cadence helper binary '{}' in replay mode. \
             Make sure the Go helper is built and either:\n  \
             - it is on your $PATH as '{}', or\n  \
             - the {} environment variable points to the binary",
            helper_bin, DEFAULT_HELPER_BIN, HELPER_BIN_ENV
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "Cadence helper '{}' replay mode exited with status {}: {}",
            helper_bin,
            output.status,
            stderr.trim()
        ));
    }

    let stdout =
        String::from_utf8(output.stdout).with_context(|| "Go helper produced non-UTF-8 output")?;

    let events = parse_ndjson(&stdout)?;

    eprintln!(
        "Got {} trace events from replaying transaction {}",
        events.len(),
        config.tx_hash
    );

    // Use a synthetic source path based on the transaction hash.
    let source_path = config
        .source_dir
        .as_deref()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("tx_{}.cdc", &config.tx_hash));

    CadenceTracer::trace_program_from_events(&source_path, &events, out_dir, format)
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- ReplayConfig construction --

    #[test]
    fn test_replay_config_new() {
        let config =
            ReplayConfig::new("0xABCDef1234567890", "access.testnet.nodes.onflow.org:9000");
        assert_eq!(config.tx_hash, "abcdef1234567890");
        assert_eq!(
            config.access_node_url,
            "access.testnet.nodes.onflow.org:9000"
        );
        assert!(config.source_dir.is_none());
    }

    #[test]
    fn test_replay_config_with_source_dir() {
        let config = ReplayConfig::new("abc123", DEFAULT_ACCESS_NODE_URL)
            .with_source_dir(PathBuf::from("/tmp/sources"));
        assert_eq!(config.source_dir, Some(PathBuf::from("/tmp/sources")));
    }

    #[test]
    fn test_normalise_tx_hash_strips_prefix() {
        assert_eq!(normalise_tx_hash("0xABCD"), "abcd");
        assert_eq!(normalise_tx_hash("ABCD"), "abcd");
        assert_eq!(normalise_tx_hash("0x0"), "0");
    }

    // -- FlowAccessClient --

    #[test]
    fn test_flow_access_client_default() {
        let client = FlowAccessClient::default();
        assert_eq!(client.access_node_url, DEFAULT_ACCESS_NODE_URL);
    }

    #[test]
    fn test_flow_access_client_custom_url() {
        let client = FlowAccessClient::new("localhost:3569");
        assert_eq!(client.access_node_url, "localhost:3569");
    }

    // -- TransactionScript --

    #[test]
    fn test_transaction_script_serde_roundtrip() {
        let tx = TransactionScript {
            script: "transaction { execute { log(\"hello\") } }".to_string(),
            arguments: vec![r#"{"type":"String","value":"world"}"#.to_string()],
            tx_hash: "abc123".to_string(),
        };
        let json = serde_json::to_string(&tx).unwrap();
        let parsed: TransactionScript = serde_json::from_str(&json).unwrap();
        assert_eq!(tx, parsed);
    }

    #[test]
    fn test_transaction_script_empty_args() {
        let tx = TransactionScript {
            script: "transaction { execute {} }".to_string(),
            arguments: vec![],
            tx_hash: "deadbeef".to_string(),
        };
        assert!(tx.arguments.is_empty());
        assert_eq!(tx.tx_hash, "deadbeef");
    }

    // -- Mock NDJSON from a replayed transaction --

    #[test]
    fn test_parse_mock_replay_ndjson() {
        // Simulates what the Go helper would emit for a simple token transfer tx.
        let ndjson = r#"{"type":"call","name":"main"}
{"type":"step","file":"tx_abc123.cdc","line":1}
{"type":"variable","name":"amount","value":"100","cadence_type":"UFix64"}
{"type":"step","file":"tx_abc123.cdc","line":2}
{"type":"variable","name":"recipient","value":"0x1234","cadence_type":"Address"}
{"type":"step","file":"tx_abc123.cdc","line":5}
{"type":"call","name":"FlowToken.transfer"}
{"type":"step","file":"FlowToken.cdc","line":42}
{"type":"resource_move","resource_type":"FlowToken.Vault","uuid":9001,"from_owner":"0xSender","to_owner":"0x1234","file":"FlowToken.cdc","line":42}
{"type":"return","value":"Void"}
{"type":"return","value":"Void"}"#;

        let events = parse_ndjson(ndjson).unwrap();
        assert_eq!(events.len(), 11);

        // Verify we see the resource_move event.
        let has_move = events.iter().any(|e| {
            matches!(e, crate::tracer::TraceEvent::ResourceMove {
                resource_type, uuid, ..
            } if resource_type == "FlowToken.Vault" && *uuid == 9001)
        });
        assert!(has_move, "should contain a resource_move event");
    }

    #[test]
    fn test_replay_ndjson_to_codetracer() {
        // Full pipeline test: parse mock replay NDJSON and convert to CodeTracer format.
        let ndjson = r#"{"type":"call","name":"execute"}
{"type":"step","file":"tx_beef.cdc","line":1}
{"type":"variable","name":"recipient","value":"0xABC","cadence_type":"Address"}
{"type":"step","file":"tx_beef.cdc","line":2}
{"type":"variable","name":"amount","value":"50","cadence_type":"Int"}
{"type":"step","file":"tx_beef.cdc","line":3}
{"type":"return","value":"Void"}"#;

        let events = parse_ndjson(ndjson).unwrap();

        let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let out_dir = tmp_dir.path().join("replay_traces");
        let source_path = PathBuf::from("tx_beef.cdc");

        CadenceTracer::trace_program_from_events(
            &source_path,
            &events,
            &out_dir,
            TraceEventsFileFormat::Json,
        )
        .expect("trace_program_from_events should succeed");

        // Verify output files exist.
        for filename in &["trace.bin", "trace_metadata.json", "trace_paths.json"] {
            let path = out_dir.join(filename);
            assert!(path.exists(), "{} should exist", filename);
            let size = std::fs::metadata(&path).unwrap().len();
            assert!(size > 0, "{} should be non-empty", filename);
        }

        // Verify trace content has expected variable names.
        let content = std::fs::read_to_string(out_dir.join("trace.bin")).unwrap();
        let trace_events: serde_json::Value = serde_json::from_str(&content).unwrap();
        let trace_array = trace_events.as_array().unwrap();

        let var_names: Vec<String> = trace_array
            .iter()
            .filter_map(|e| {
                e.get("VariableName")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        assert!(
            var_names.contains(&"recipient".to_string()),
            "should have variable 'recipient'"
        );
        assert!(
            var_names.contains(&"amount".to_string()),
            "should have variable 'amount'"
        );
    }
}
