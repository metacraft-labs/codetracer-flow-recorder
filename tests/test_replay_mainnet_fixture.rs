//! A real Flow mainnet transaction, replayed by `go-replay-helper` and
//! committed as its NDJSON stream, executed sources and verdict, converted to
//! CTFS here and read back through `NimTraceReaderHandle` -- the structured
//! reader the db-backend uses. Nothing here touches the network; the
//! fixture's `replay-verdict.json` is the record of the run that produced it.
//!
//! Transaction `c8f91907…` at mainnet height 168,391,034: a TopShot offer
//! through DapperOffersV2. Its payer is neither its proposer nor either of its
//! two authorizers, and it saves a newly created `OffersV2.Offer` resource
//! into the proposer's account -- an ownership change with an identity on
//! both sides of it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use codetracer_flow_recorder::tracer::{parse_ndjson, CadenceTracer, TraceEvent};
use codetracer_trace_writer_nim::NimTraceReaderHandle;

const FIXTURE: &str =
    "tests/fixtures/replay/mainnet-c8f9190752baabcbe010c201d4b34bcda9c3de91b53a6fe6accf8d95442e0611";

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

fn events() -> Vec<TraceEvent> {
    parse_ndjson(&std::fs::read_to_string(fixture().join("trace.ndjson")).unwrap()).unwrap()
}

fn verdict() -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(fixture().join("replay-verdict.json")).unwrap())
        .unwrap()
}

/// Convert the fixture in a scratch directory holding a copy of its sources,
/// as `replay` lays them out, and return the reader over the container.
fn convert() -> (tempfile::TempDir, NimTraceReaderHandle) {
    convert_events(&events())
}

fn convert_events(events: &[TraceEvent]) -> (tempfile::TempDir, NimTraceReaderHandle) {
    let tmp = tempfile::tempdir().unwrap();
    let sources = tmp.path().join("sources");
    std::fs::create_dir_all(&sources).unwrap();
    let mut entry = None;
    for file in std::fs::read_dir(fixture().join("sources")).unwrap() {
        let file = file.unwrap().path();
        let name = file.file_name().unwrap().to_owned();
        std::fs::copy(&file, sources.join(&name)).unwrap();
        if name.to_string_lossy().starts_with("t.") {
            entry = Some(sources.join(&name));
        }
    }
    let entry = entry.expect("the fixture carries its transaction script");
    let out = tmp.path().join("out");
    CadenceTracer::trace_program_from_events(&entry, events, &out).unwrap();
    let ct = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "ct"))
        .expect("a .ct container");
    let reader = NimTraceReaderHandle::open(ct.to_str().unwrap()).unwrap();
    (tmp, reader)
}

#[test]
fn the_fixture_is_a_reproduced_unperturbed_replay() {
    let v = verdict();
    assert_eq!(
        v["reproduced"], true,
        "the replay matched the chain's events and status"
    );
    assert_eq!(
        v["observationPerturbed"], false,
        "tracing changed neither computation nor events against the untraced run"
    );
    assert_eq!(
        v["replay"]["computationUsed"],
        v["onChain"]["computationUsed"]
    );
    assert_eq!(v["eventsMatched"], v["eventsCompared"]);
    assert!(v["eventsCompared"].as_u64().unwrap() > 0);
}

/// Cadence locates a transaction's own script at `t.<transaction id>`, and
/// the chain's error messages point there. The helper rebuilds the body it
/// executes, and a body rebuilt without its signatures and reference block
/// hashes to another id, so the script was published under a name the chain
/// never used. Control: the fixture holds exactly one transaction-script
/// file, so the check is about its name and not about whether one exists.
#[test]
fn the_transaction_script_is_located_at_the_chains_transaction_id() {
    let v = verdict();
    let id = v["transactionId"].as_str().unwrap();
    let scripts: Vec<&str> = v["sources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["location"].as_str().unwrap())
        .filter(|l| l.starts_with("t."))
        .collect();
    assert_eq!(scripts, vec![format!("t.{id}")]);
    let files: Vec<String> = std::fs::read_dir(fixture().join("sources"))
        .unwrap()
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("t."))
        .collect();
    assert_eq!(files, vec![format!("t.{id}.cdc")]);
}

/// Flow's authorization is a trio. Here the payer is a third account, so a
/// producer that reports one sender is wrong about who paid.
#[test]
fn proposer_payer_and_authorizers_are_reported_separately() {
    let v = verdict();
    let proposer = v["proposer"].as_str().unwrap();
    let payer = v["payer"].as_str().unwrap();
    let authorizers: Vec<&str> = v["authorizers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(authorizers.len(), 2);
    assert_ne!(payer, proposer);
    assert!(!authorizers.contains(&payer));
    assert!(authorizers.contains(&proposer));
}

/// One container step per statement the interpreter stopped at, plus the
/// opening step the writer's `start` registers, in the file of the location
/// it stopped in. Two controls: the same stream with its resource records
/// stripped converts to the same count, so a create, move or destroy adds no
/// step of its own; and every step file is in the path table, where a
/// converter that put them all in the entry source would report one path.
#[test]
fn steps_are_one_per_statement_across_every_executed_location() {
    let events = events();
    let without_resources: Vec<TraceEvent> = events
        .iter()
        .filter(|e| {
            !matches!(
                e,
                TraceEvent::ResourceCreate { .. }
                    | TraceEvent::ResourceMove { .. }
                    | TraceEvent::ResourceDestroy { .. }
            )
        })
        .cloned()
        .collect();
    assert!(without_resources.len() < events.len());
    let steps = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::Step { .. }))
        .count() as u64;
    let step_files: HashSet<&str> = events
        .iter()
        .filter_map(|e| match e {
            TraceEvent::Step { file, .. } => Some(file.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        step_files.len() > 1,
        "the stream enters more than one location"
    );

    let (_tmp, reader) = convert();
    assert_eq!(
        reader.step_count(),
        steps + 1,
        "statements plus the opening step"
    );
    let (_tmp2, control) = convert_events(&without_resources);
    assert_eq!(
        reader.step_count(),
        control.step_count(),
        "a resource record inside a statement must not add a step of its own"
    );
    let paths: HashSet<String> = (0..reader.path_count())
        .map(|i| {
            Path::new(&reader.path(i).unwrap())
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    for file in &step_files {
        assert!(
            paths.contains(*file),
            "{file} is missing from the path table"
        );
    }
}

/// Calls are named by the declaration that encloses the statement, qualified
/// by composite, in every location -- the transaction's blocks, the offer
/// contract, and the fee contracts the FVM runs around the body.
#[test]
fn calls_are_named_after_their_declarations() {
    let (_tmp, reader) = convert();
    let functions: HashSet<String> = (0..reader.function_count())
        .map(|i| reader.function(i).unwrap())
        .collect();
    for name in [
        "prepare",
        "execute",
        "FlowFees.deductTransactionFee",
        "FungibleToken.Vault.withdraw",
    ] {
        assert!(
            functions.contains(name),
            "missing function {name}; have {functions:?}"
        );
    }
}

/// The resource the transaction saves keeps one identity from its creation
/// through its move into the proposer's account, and is visible as that
/// identity in the values. The control is the other resource created in the
/// same run, of a different type and UUID, which is not conflated with it.
#[test]
fn a_resource_keeps_its_identity_across_a_move() {
    let events = events();
    let (moved_type, moved_uuid, to) = events
        .iter()
        .find_map(|e| match e {
            TraceEvent::ResourceMove {
                resource_type,
                uuid,
                to_owner,
                ..
            } => Some((resource_type.clone(), *uuid, to_owner.clone())),
            _ => None,
        })
        .expect("the fixture moves a resource");
    assert_eq!(to, verdict()["proposer"].as_str().unwrap());

    let created: Vec<(String, u64)> = events
        .iter()
        .filter_map(|e| match e {
            TraceEvent::ResourceCreate {
                resource_type,
                uuid,
                ..
            } => Some((resource_type.clone(), *uuid)),
            _ => None,
        })
        .collect();
    assert!(
        created.contains(&(moved_type.clone(), moved_uuid)),
        "the moved resource is the one created in this transaction"
    );
    let others: Vec<&(String, u64)> = created.iter().filter(|c| c.1 != moved_uuid).collect();
    assert!(!others.is_empty(), "control: a second resource is created");
    assert!(others
        .iter()
        .all(|c| c.0 != moved_type || c.1 != moved_uuid));

    let named = format!("@resource:{moved_type}#{moved_uuid}");
    let seen = events
        .iter()
        .filter(|e| matches!(e, TraceEvent::Variable { value, .. } if value == &named))
        .count();
    assert!(
        seen > 1,
        "the resource is observed by identity in the values ({seen} times)"
    );
}
