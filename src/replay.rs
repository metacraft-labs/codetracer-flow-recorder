//! On-chain transaction replay for Flow (M5).
//!
//! Fetches a transaction from a Flow Access Node, extracts its Cadence script
//! and arguments, runs it through the Go tracer helper in replay mode, and
//! converts the NDJSON output into CodeTracer trace files.

use std::path::{Path, PathBuf};
use std::process::Command;

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
    /// Directory the executed Cadence sources are written to.
    pub source_dir: Option<PathBuf>,
    /// Replay from the transcript in this directory (the helper's
    /// `--from-inputs`) instead of reading the access node.
    pub inputs: Option<PathBuf>,
    /// Write a transcript of the run's inputs to this directory (the
    /// helper's `--record-inputs`).
    pub record_inputs: Option<PathBuf>,
    /// The container's recording id: minted by the writer when `None`.
    pub recording_id: Option<RecordingId>,
}

/// How a replay's container recording id is chosen when it is not minted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingId {
    /// Use this UUIDv7 verbatim; the writer validates it.
    Pinned(String),
    /// Derive it from the block timestamp and the transaction id
    /// ([`derived_recording_id`]), so two replays of one transaction write
    /// the same container.
    Derive,
}

/// A valid UUIDv7 (RFC 9562) that is a function of the transaction alone: the
/// 48-bit time field is the block's timestamp in milliseconds, so ids still
/// sort by when the transaction ran, and the 74 "random" bits are the leading
/// bits of the transaction id. The same layout as `codetracer-evm-recorder`'s
/// `derived_recording_id`, from a millisecond timestamp because Flow's
/// header states one.
pub fn derived_recording_id(block_timestamp_ms: u64, tx_id: &[u8; 32]) -> String {
    let ms = block_timestamp_ms & 0xFFFF_FFFF_FFFF;
    let h = tx_id;
    let mut b = [0u8; 16];
    b[..6].copy_from_slice(&ms.to_be_bytes()[2..]);
    b[6] = 0x70 | (h[0] & 0x0f);
    b[7] = h[1];
    b[8] = 0x80 | (h[2] & 0x3f);
    b[9..].copy_from_slice(&h[3..10]);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

impl ReplayConfig {
    /// Create a new replay configuration.
    pub fn new(tx_hash: &str, access_node_url: &str) -> Self {
        Self {
            tx_hash: normalise_tx_hash(tx_hash),
            access_node_url: access_node_url.to_string(),
            source_dir: None,
            inputs: None,
            record_inputs: None,
            recording_id: None,
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

/// Default binary name for the replay helper (`go-replay-helper/`).
const DEFAULT_HELPER_BIN: &str = "cadence-replay-helper";

/// Environment variable to override the replay helper binary path.
const HELPER_BIN_ENV: &str = "CADENCE_REPLAY_HELPER_BIN";

/// File the helper's verdict is written to, inside the output directory.
pub const VERDICT_FILE: &str = "replay-verdict.json";

/// Directory the executed Cadence sources are written to, inside the output
/// directory, when `--source-dir` is not given.
pub const SOURCES_DIR: &str = "sources";

// ---------------------------------------------------------------------------
// Replay pipeline
// ---------------------------------------------------------------------------

/// Replay a Flow transaction and write a CodeTracer CTFS trace bundle.
///
/// Pipeline:
/// 1. Invoke the replay helper with the transaction id and access node. The
///    helper re-executes the transaction with the network's FVM against the
///    register state of its block's parent, after replaying the transactions
///    that precede it in the block, and emits the interpreter's statement
///    stream as NDJSON on stdout. It writes every Cadence source the stream
///    entered into the source directory -- read from the same register state,
///    so it is the code that ran -- and its verdict (whether the replay
///    reproduced the on-chain result) to `replay-verdict.json`.
/// 2. Parse the NDJSON output.
/// 3. Write a CTFS bundle to `out_dir`, with the transaction script as the
///    entry source.
///
/// The output format is fixed to CTFS — see
/// `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`.
pub fn replay_transaction(config: &ReplayConfig, out_dir: &Path) -> Result<()> {
    let helper_bin =
        std::env::var(HELPER_BIN_ENV).unwrap_or_else(|_| DEFAULT_HELPER_BIN.to_string());
    replay_transaction_with_helper(config, out_dir, &helper_bin)
}

fn replay_transaction_with_helper(
    config: &ReplayConfig,
    out_dir: &Path,
    helper_bin: &str,
) -> Result<()> {
    let source_dir = config
        .source_dir
        .clone()
        .unwrap_or_else(|| out_dir.join(SOURCES_DIR));
    std::fs::create_dir_all(&source_dir)
        .with_context(|| format!("cannot create source dir: {}", source_dir.display()))?;

    let mut command = Command::new(helper_bin);
    command
        .arg("replay")
        .arg("--tx-id")
        .arg(&config.tx_hash)
        .arg("--access-node")
        .arg(&config.access_node_url)
        .arg("--source-dir")
        .arg(&source_dir)
        .arg("--verdict-out")
        .arg(out_dir.join(VERDICT_FILE));
    if let Some(inputs) = &config.inputs {
        command.arg("--from-inputs").arg(inputs);
    }
    if let Some(record) = &config.record_inputs {
        command.arg("--record-inputs").arg(record);
    }
    let output = command.output().with_context(|| {
        format!(
            "failed to run replay helper '{}'. Build it from go-replay-helper/ \
                 and either:\n  \
                 - put it on your $PATH as '{}', or\n  \
                 - point the {} environment variable at the binary",
            helper_bin, DEFAULT_HELPER_BIN, HELPER_BIN_ENV
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!(
            "replay helper '{}' exited with status {}: {}",
            helper_bin,
            output.status,
            stderr.trim()
        ));
    }

    let stdout = String::from_utf8(output.stdout)
        .with_context(|| "replay helper produced non-UTF-8 output")?;
    let events = parse_ndjson(&stdout)?;

    eprintln!(
        "Got {} trace events from replaying transaction {}",
        events.len(),
        config.tx_hash
    );

    let source_path = entry_source_path(&events, &source_dir, &config.tx_hash);
    let recording_id = match &config.recording_id {
        None => None,
        Some(RecordingId::Pinned(id)) => Some(id.clone()),
        Some(RecordingId::Derive) => Some(derive_from_verdict(&out_dir.join(VERDICT_FILE))?),
    };
    CadenceTracer::trace_program_from_events_with_recording_id(
        &source_path,
        &events,
        out_dir,
        recording_id.as_deref(),
    )
}

/// The derived recording id of the replay whose verdict is at `path`.
fn derive_from_verdict(path: &Path) -> Result<String> {
    let verdict: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?,
    )?;
    let ms = verdict["blockTimestampMs"]
        .as_u64()
        .filter(|ms| *ms > 0)
        .ok_or_else(|| eyre!("{} states no blockTimestampMs", path.display()))?;
    let id = verdict["transactionId"]
        .as_str()
        .ok_or_else(|| eyre!("{} states no transactionId", path.display()))?;
    let mut bytes = [0u8; 32];
    if id.len() != 64 {
        return Err(eyre!("transaction id {id} is not 32 bytes of hex"));
    }
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&id[2 * i..2 * i + 2], 16)
            .map_err(|e| eyre!("transaction id {id}: {e}"))?;
    }
    Ok(derived_recording_id(ms, &bytes))
}

/// The entry source of a replayed transaction: the transaction script, which
/// the helper names after its Cadence location, `t.<script hash>.cdc`. It is
/// not simply the first step's file: the FVM runs the fee contracts' payer
/// balance check before the transaction body, so on mainnet the first step is
/// in `FlowFees`. Without a step in the script, the first step's file is
/// used, and with no step at all, a name in the source directory.
fn entry_source_path(
    events: &[crate::tracer::TraceEvent],
    source_dir: &Path,
    tx_hash: &str,
) -> PathBuf {
    let step_files = || {
        events.iter().filter_map(|event| match event {
            crate::tracer::TraceEvent::Step { file, .. } if !file.is_empty() => {
                // The helper names files relative to the source directory.
                Some(source_dir.join(file))
            }
            _ => None,
        })
    };
    step_files()
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("t."))
        })
        .or_else(|| step_files().next())
        .unwrap_or_else(|| source_dir.join(format!("t.{tx_hash}.cdc")))
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

    /// The pipeline hands the helper the transaction id, access node, a
    /// source directory and a verdict path, and takes the entry source from
    /// the first step rather than inventing one. A stand-in helper records
    /// its arguments and plays back NDJSON whose first step is in the
    /// transaction script and whose second is in an imported contract.
    #[cfg(unix)]
    #[test]
    fn test_replay_passes_helper_contract_and_uses_first_step_as_entry() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let out_dir = tmp.path().join("out");
        let args_file = tmp.path().join("args.txt");
        let helper = tmp.path().join("fake-helper.sh");
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\n\
                 printf '%s\\n' \"$@\" > '{args}'\n\
                 dir=''; prev=''\n\
                 for a in \"$@\"; do [ \"$prev\" = '--source-dir' ] && dir=\"$a\"; prev=\"$a\"; done\n\
                 printf 'transaction {{ execute {{ }} }}\\n' > \"$dir/t.abcd.cdc\"\n\
                 printf 'access(all) contract C {{}}\\n' > \"$dir/A.0000000000000001.C.cdc\"\n\
                 echo '{{\"type\":\"call\",\"name\":\"execute\"}}'\n\
                 echo \"{{\\\"type\\\":\\\"step\\\",\\\"file\\\":\\\"t.abcd.cdc\\\",\\\"line\\\":1,\\\"column\\\":15}}\"\n\
                 echo \"{{\\\"type\\\":\\\"step\\\",\\\"file\\\":\\\"A.0000000000000001.C.cdc\\\",\\\"line\\\":1}}\"\n\
                 echo '{{\"type\":\"return\"}}'\n",
                args = args_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

        std::fs::create_dir_all(&out_dir).unwrap();
        let config = ReplayConfig::new("0xABCD", "access.example:9000");
        replay_transaction_with_helper(&config, &out_dir, helper.to_str().unwrap())
            .expect("replay should succeed with the stand-in helper");

        let args: Vec<String> = std::fs::read_to_string(&args_file)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        let sources = out_dir.join(SOURCES_DIR);
        assert_eq!(
            args,
            vec![
                "replay".to_string(),
                "--tx-id".into(),
                "abcd".into(),
                "--access-node".into(),
                "access.example:9000".into(),
                "--source-dir".into(),
                sources.display().to_string(),
                "--verdict-out".into(),
                out_dir.join(VERDICT_FILE).display().to_string(),
            ]
        );
        assert!(
            std::fs::read_dir(&out_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .any(|e| e.path().extension().is_some_and(|ext| ext == "ct")),
            "CTFS container missing"
        );

        // The shape a mainnet replay has: the fee contract's balance check
        // runs before the transaction body.
        let fees = sources.join("A.f919ee77447b7497.FlowFees.cdc");
        let script = sources.join("t.abcd.cdc");
        let events = parse_ndjson(&format!(
            "{{\"type\":\"step\",\"file\":\"{}\",\"line\":1}}\n\
             {{\"type\":\"step\",\"file\":\"{}\",\"line\":1}}",
            fees.display(),
            script.display()
        ))
        .unwrap();
        assert_eq!(
            entry_source_path(&events, &sources, "abcd"),
            script,
            "the entry is the transaction script even when a fee contract steps first"
        );
        assert_eq!(
            entry_source_path(&events[..1], &sources, "abcd"),
            fees,
            "with no step in the script, the entry is the first step's file"
        );
        assert_eq!(
            entry_source_path(&[], &sources, "abcd"),
            sources.join("t.abcd.cdc"),
            "with no step, the entry falls back to the transaction location's name"
        );
    }
}

#[cfg(test)]
mod recording_id_tests {
    use super::derived_recording_id;

    /// The derived id is a well-formed UUIDv7 whose time field is the block's
    /// millisecond timestamp, and it depends on the transaction: the control
    /// is a second transaction in the same block, which must get another id.
    #[test]
    fn derived_ids_are_uuidv7_of_the_block_time_and_the_transaction() {
        // Mainnet block 168,391,034 and the two user transactions in it.
        let ms = 1_760_964_548_123_u64;
        let mut a = [0u8; 32];
        a[..4].copy_from_slice(&[0xc8, 0xf9, 0x19, 0x07]);
        let mut b = a;
        b[3] = 0x08;

        let id = derived_recording_id(ms, &a);
        assert_eq!(id, derived_recording_id(ms, &a), "a function of its inputs");
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(parts[2].starts_with('7'), "version 7: {id}");
        assert!(
            matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
            "RFC 9562 variant: {id}"
        );
        let time = u64::from_str_radix(&format!("{}{}", parts[0], parts[1]), 16).unwrap();
        assert_eq!(time, ms, "the time field is the block's timestamp");
        assert_ne!(
            id,
            derived_recording_id(ms, &b),
            "control: another transaction"
        );
    }
}
