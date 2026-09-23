//! Tracer implementation for Cadence programs.
//!
//! Shells out to a Go helper binary that uses the real Cadence runtime to
//! execute programs, then parses the NDJSON trace output and emits CodeTracer
//! trace events (steps, calls, returns, variables).
//!
//! The Go helper binary path is controlled by the `CADENCE_HELPER_BIN`
//! environment variable.  When unset it defaults to `cadence-trace-helper`
//! (looked up on `$PATH`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use codetracer_trace_types::{
    EventLogKind, FullValueRecord, Line, TraceLowLevelEvent, TypeId, TypeKind, ValueRecord,
    NONE_VALUE,
};
use codetracer_trace_writer_nim::non_streaming_trace_writer::NonStreamingTraceWriter;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{
    create_trace_writer, StreamingValueEncoder, TraceEventsFileFormat,
};
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
    Step {
        file: String,
        line: u32,
        /// 1-based source column the step landed on, as emitted by
        /// the Go helper after converting Cadence's 0-based
        /// `ast.Position.Column`.  `None` when the helper omits the
        /// field (legacy NDJSON without column info) — in that case
        /// the recorder still emits a column-aware step but with
        /// `column = None`, per the column-aware mode contract
        /// (`enable_column_aware_steps` is sticky; readers fall
        /// back to a `None` column for individual steps).
        #[serde(default)]
        column: Option<u32>,
    },
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
        /// Optional Cadence visibility modifier for the called
        /// function declaration (`access(self)` / `access(contract)` /
        /// `access(account)` / `access(all)`).  Recognised values are
        /// `"self"`, `"contract"`, `"account"`, `"all"`; any other
        /// (or absent) value skips the visibility-tag io_event.
        ///
        /// Spec rationale: the M10 strict pin
        /// `test_access_control_test_via_ct_print_full` requires every
        /// call frame to surface its source-declared visibility so the
        /// frontend can highlight access-violation paths.  We mirror
        /// the M9 `error_kind` dispatch and route the tag through a
        /// dedicated `CadenceAccess:<function>:<Tag>` io_event whose
        /// text payload survives the multi-stream writer's metadata
        /// drop.
        #[serde(default)]
        access: Option<String>,
        /// When `true`, the called function is the Cadence script
        /// entry point (`access(all) fun main(): X`).  The recorder
        /// emits a tagged `CadenceScriptEntry:<function>` io_event
        /// so the strict pin in
        /// `test_scripts_test_via_ct_print_full` can pin the script-
        /// entry boundary on the io-event channel — distinguishing
        /// the script form from a transaction form on the same
        /// `main`-named entry.
        #[serde(default)]
        script: bool,
    },
    #[serde(rename = "return")]
    Return {
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        cadence_type: Option<String>,
    },
    #[serde(rename = "error")]
    Error {
        message: String,
        /// Optional sub-kind discriminator emitted by the Go helper to
        /// distinguish Cadence pre-condition / post-condition violations
        /// from a user-issued `panic`.  Recognised values are
        /// `"pre"`, `"post"`, `"panic"`; any other (or absent) value is
        /// treated as the legacy generic Error.
        ///
        /// Spec rationale: the strict pin
        /// `test_error_paths_test_distinguishes_pre_post_from_panic`
        /// requires three distinct trace events for the three
        /// failure modes — historically all three collapsed onto a
        /// single `EventLogKind::Error` IO entry.
        #[serde(default, rename = "error_kind")]
        error_kind: Option<String>,
    },
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
        /// 1-based source column, optional.  See `Step::column`.
        #[serde(default)]
        column: Option<u32>,
    },

    #[serde(rename = "resource_move")]
    ResourceMove {
        resource_type: String,
        uuid: u64,
        from_owner: String,
        to_owner: String,
        file: String,
        line: u32,
        /// 1-based source column, optional.  See `Step::column`.
        #[serde(default)]
        column: Option<u32>,
    },

    #[serde(rename = "resource_destroy")]
    ResourceDestroy {
        resource_type: String,
        uuid: u64,
        owner: String,
        file: String,
        line: u32,
        /// 1-based source column, optional.  See `Step::column`.
        #[serde(default)]
        column: Option<u32>,
    },

    // ----- M10 events -----
    /// Tagged owner-change notification emitted alongside a move
    /// operator that transfers ownership of a resource.  Surfaces in
    /// the multi-stream trace as a `TraceLogEvent` (→ `ioStderr`) with
    /// the literal `ResourceOwnerChange:` prefix in the text payload
    /// so the tag survives the writer's metadata-drop on the
    /// multi-stream path.  This is the closing piece of the M10
    /// `resources_full_test` deliverable: the move operators
    /// (`let b <- a`, `<->`, shift, `<-!`, `destroy`) all surface
    /// owner transitions through this single tagged channel.
    #[serde(rename = "resource_owner_change")]
    ResourceOwnerChange {
        resource_type: String,
        uuid: u64,
        from_owner: String,
        to_owner: String,
        file: String,
        line: u32,
        /// 1-based source column, optional.  See `Step::column`.
        #[serde(default)]
        column: Option<u32>,
    },

    /// Force-unwrap of a `nil` optional (`foo!` over `nil`).  Surfaces
    /// as a tagged `ForceNilUnwrap:<context>` io_event routed through
    /// `EventLogKind::TraceLogEvent` so the failure mode is
    /// distinguishable from a generic Cadence `panic` on the
    /// io-event channel.  Closes the M10
    /// `optional_chaining_test` deliverable: the strict pin asserts
    /// `(io_kind, text) == ("ioStderr", "ForceNilUnwrap:<ctx>")`.
    #[serde(rename = "force_nil_unwrap")]
    ForceNilUnwrap {
        /// A short descriptor for the unwrap site (e.g. the local
        /// name or the chain expression text).  Free-form for now;
        /// the Go helper will fill in the source-printed form once
        /// it ships an OnStatement-based optional tracker.
        #[serde(default)]
        context: String,
    },

    /// Cadence `emit Foo(...)` event with structured field values.
    /// Routes to a tagged `CadenceEmit:` io_event whose text payload
    /// preserves the event name, field name/value pairs, and the
    /// concrete Cadence type of each field.  This is the closing
    /// piece of the M10 `events_emit_test` deliverable — a typed
    /// alternative to the legacy `event` variant which only carries a
    /// pre-rendered string payload.
    #[serde(rename = "emit")]
    Emit {
        name: String,
        #[serde(default)]
        fields: Vec<TraceArg>,
    },

    /// Composite-kind metadata tag.  Cadence has three composite
    /// declaration kinds (`struct`, `resource`, `event`), each
    /// surfaced as a distinct `composite_kind` discriminator on the
    /// declaration's type metadata.  The recorder routes each as a
    /// tagged io_event (`CompositeKindStructure:<Type>`,
    /// `CompositeKindResource:<Type>`, `CompositeKindEvent:<Type>`)
    /// through `EventLogKind::TraceLogEvent` → `ioStderr` so the
    /// kind is visible to the strict pin without re-deriving it
    /// from the declaration source.
    ///
    /// Closes the M10 `composite_types_test` deliverable: each of
    /// the three composite kinds surfaces on the io-event channel
    /// with a distinct tag prefix.
    #[serde(rename = "composite_kind")]
    CompositeKind {
        /// One of `"structure"` / `"resource"` / `"event"` — the
        /// canonical Cadence composite-kind discriminators.  Any
        /// other value is silently dropped (the recorder skips
        /// unrecognised kinds rather than emitting a malformed tag).
        kind: String,
        type_name: String,
    },

    /// Dynamic-type bind metadata for `AnyResource` / `AnyStruct`
    /// parameters.  Cadence permits passing concrete resources /
    /// structs through a function parameter typed as the dynamic
    /// supertype (`@AnyResource` / `AnyStruct` / `AnyAuthAccount` /
    /// `AnyPublicAccount`); the runtime preserves the concrete type
    /// information.  The recorder routes a tagged io_event
    /// (`CadenceAnyType:<static>:<runtime>:<varname>`) through
    /// `EventLogKind::TraceLogEvent` → `ioStderr` so both the static
    /// supertype and the concrete runtime type are visible to the
    /// strict pin without re-deriving them from the call frame's
    /// argument types.
    ///
    /// Closes the M10 `anyresource_anystruct_test` deliverable: each
    /// dynamic parameter surfaces with both type-ids visible on the
    /// io-event channel.
    #[serde(rename = "any_type_bind")]
    AnyTypeBind {
        /// The static (declared-parameter) type — one of
        /// `AnyResource` / `AnyStruct` / `AnyAuthAccount` /
        /// `AnyPublicAccount`.
        static_type: String,
        /// The concrete runtime type id — e.g. `Vault`, `Token`,
        /// `MyResource.Inner`.
        runtime_type: String,
        /// The variable name the dynamic value is bound to in the
        /// call frame (used for cross-referencing against the
        /// step-local).
        varname: String,
    },

    /// Cadence 1.x `type alias` declaration metadata.  Cadence permits
    /// top-level `access(all) type alias <Alias> = <Underlying>`
    /// declarations; the alias is a compile-time-only renaming so
    /// values declared with the alias type carry the underlying
    /// type's typed `ValueRecord` variant.  The recorder surfaces the
    /// alias → underlying linkage as a tagged
    /// `CadenceTypeAlias:<Alias>:<Underlying>` io_event so downstream
    /// consumers can resolve aliases without re-parsing the source.
    ///
    /// Closes the M10 `type_aliases_test` deliverable: the alias
    /// metadata is visible to the strict pin via the io-event channel.
    #[serde(rename = "type_alias")]
    TypeAlias {
        alias_name: String,
        underlying_type: String,
    },

    /// Cadence 1.x attachment-attach event.  The attachment value
    /// itself carries `<Att>@<Target>` as its type-id; this event
    /// surfaces the attach site as a tagged
    /// `CadenceAttachmentAttach:<Att>:<Target>` io_event so the
    /// attachment-to-base linkage is visible on the io-event channel
    /// independently of the typed-local snapshot.
    ///
    /// Closes the M10 `attachments_test` deliverable: each attach
    /// surfaces with the (attachment, target) pair on the io-event
    /// channel.
    #[serde(rename = "attachment_attach")]
    AttachmentAttach {
        attachment_type: String,
        target_type: String,
    },

    /// Cadence 1.x attachment-remove event — the symmetric counterpart
    /// to `AttachmentAttach`.  Surfaces as a tagged
    /// `CadenceAttachmentRemove:<Att>:<Target>` io_event so the
    /// attach/remove pair is visible end-to-end on the io-event
    /// channel.
    #[serde(rename = "attachment_remove")]
    AttachmentRemove {
        attachment_type: String,
        target_type: String,
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
// Cadence printed-form parsers (used by `value_record`)
// ---------------------------------------------------------------------------

/// Split a Cadence-printed array (`"[1, 2, 3, 4]"`) into its element
/// strings, respecting nested brackets/braces and double-quoted strings
/// so a nested compound element is not split mid-payload.
///
/// Returns an empty `Vec` for `"[]"` or any input that does not start
/// with `[`.
fn parse_cadence_array(value: &str) -> Vec<String> {
    let v = value.trim();
    if !(v.starts_with('[') && v.ends_with(']')) {
        return Vec::new();
    }
    let inner = &v[1..v.len() - 1];
    split_top_level(inner, ',')
}

/// Split a Cadence-printed dictionary (`"{\"apple\": 30, \"banana\": 10}"`)
/// into `(key, value)` string pairs.  Quoted string keys have the
/// surrounding `"` stripped so the caller can feed the key straight back
/// through `value_record` with the dictionary's key type.
///
/// Returns an empty `Vec` for `"{}"` or any input that does not start
/// with `{`.
fn parse_cadence_dict(value: &str) -> Vec<(String, String)> {
    let v = value.trim();
    if !(v.starts_with('{') && v.ends_with('}')) {
        return Vec::new();
    }
    let inner = &v[1..v.len() - 1];
    split_top_level(inner, ',')
        .into_iter()
        .filter_map(|entry| {
            // Find the top-level `:` that separates key from value.
            let split_idx = top_level_index_of(&entry, ':')?;
            let key = entry[..split_idx].trim().to_string();
            let val = entry[split_idx + 1..].trim().to_string();
            let key = strip_quotes(&key).to_string();
            Some((key, val))
        })
        .collect()
}

/// Parse a Cadence-printed struct (`"S.Point(x: 3, y: 4)"`) into an
/// ordered vector of `(field_name, field_value)` pairs.  Returns an
/// empty `Vec` if the input does not match the `Name(field: val, ...)`
/// shape.
fn parse_cadence_struct(value: &str) -> Vec<(String, String)> {
    let v = value.trim();
    let open = match v.find('(') {
        Some(i) => i,
        None => return Vec::new(),
    };
    if !v.ends_with(')') {
        return Vec::new();
    }
    let inner = &v[open + 1..v.len() - 1];
    split_top_level(inner, ',')
        .into_iter()
        .filter_map(|field| {
            let split_idx = top_level_index_of(&field, ':')?;
            let name = field[..split_idx].trim().to_string();
            let val = field[split_idx + 1..].trim().to_string();
            Some((name, val))
        })
        .collect()
}

/// Parse a Cadence resource handle (`"@Coin#7001"` or
/// `"@Vault#5001@0x01"`) into its `(type, uuid, owner)` components.
/// Returns `None` if the input does not match the `@<Type>#<uuid>`
/// shape.  The trailing `@<owner>` segment is optional — when absent
/// the owner is reported as `None` (the M4 form, used by the
/// pre-M10 fixtures where owner snapshots only flow through the
/// dedicated `resource_create` / `resource_move` / `resource_destroy`
/// trace events).
fn parse_resource_handle(value: &str) -> Option<(String, u64, Option<String>)> {
    let v = value.trim();
    let v = v.strip_prefix('@')?;
    let (ty, rest) = v.split_once('#')?;
    // Owner suffix (M10 resources_full_test): `<uuid>@<owner>`.  If
    // present, split it off so the resource's typed-Struct snapshot
    // can carry the implicit `owner` field alongside `uuid`.
    let (uuid_str, owner) = match rest.split_once('@') {
        Some((u, o)) => (u, Some(o.to_string())),
        None => (rest, None),
    };
    let uuid: u64 = uuid_str.parse().ok()?;
    Some((ty.to_string(), uuid, owner))
}

/// Split `s` on every top-level occurrence of `sep`, treating `[]`,
/// `{}`, `()` and double-quoted substrings as opaque blocks.  Used to
/// safely walk over Cadence printed compound forms without serde-style
/// re-parsing of every layer.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    let mut in_quote = false;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        if in_quote {
            if ch == '"' {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '"' => in_quote = true,
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(s[start..i].trim().to_string());
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    let last = s[start..].trim().to_string();
    if !last.is_empty() || !out.is_empty() {
        out.push(last);
    }
    out
}

/// Find the byte index of the first top-level occurrence of `sep` in
/// `s`, treating brackets/braces/parens/quotes as opaque.
fn top_level_index_of(s: &str, sep: char) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_quote = false;
    for (i, ch) in s.char_indices() {
        if in_quote {
            if ch == '"' {
                in_quote = false;
            }
            continue;
        }
        match ch {
            '"' => in_quote = true,
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            c if c == sep && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Encode the decimal-formatted integer in `value` as a typed
/// [`ValueRecord::BigInt`] using the writer's `writeBigInt` shape:
/// `negative` flag plus big-endian unsigned magnitude bytes.
///
/// Used by the `Int128` / `UInt128` / `UInt256` / `Word*` numeric
/// branches in `value_record` so wide integers preserve their exact
/// value end-to-end.  Falls back to a single zero byte when the input
/// is not a valid base-10 integer (the recorder treats that as the
/// big-int analogue of a parse failure).
fn value_to_bigint(value: &str, type_id: TypeId) -> ValueRecord {
    let v = value.trim();
    let (negative, magnitude_str) = match v.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, v),
    };
    let bytes = decimal_to_be_bytes(magnitude_str);
    ValueRecord::BigInt {
        b: bytes,
        negative,
        type_id,
    }
}

/// Convert a base-10 unsigned-magnitude string into big-endian bytes.
///
/// Standalone (no `num-bigint` dep) so the recorder stays at zero
/// extra deps.  Performs schoolbook divide-by-256 over a
/// little-endian digit buffer, which is O(n^2) in the digit count but
/// fine for the ≤256-bit numerics Cadence accepts.  An empty / non-
/// digit input returns the canonical empty-byte representation
/// (which the writer encodes as the literal 0).
fn decimal_to_be_bytes(decimal: &str) -> Vec<u8> {
    let digits: Vec<u8> = decimal
        .chars()
        .filter_map(|c| c.to_digit(10).map(|d| d as u8))
        .collect();
    if digits.is_empty() {
        return Vec::new();
    }
    // Strip leading zeros.
    let mut idx = 0usize;
    while idx + 1 < digits.len() && digits[idx] == 0 {
        idx += 1;
    }
    let mut digits: Vec<u8> = digits[idx..].to_vec();
    if digits == [0u8] {
        return vec![0u8];
    }
    let mut be: Vec<u8> = Vec::new();
    while !(digits.len() == 1 && digits[0] == 0) {
        let mut remainder: u32 = 0;
        for d in digits.iter_mut() {
            let acc = remainder * 10 + *d as u32;
            *d = (acc / 256) as u8;
            remainder = acc % 256;
        }
        be.push(remainder as u8);
        // Strip leading zero in the new digit buffer.
        while digits.len() > 1 && digits[0] == 0 {
            digits.remove(0);
        }
    }
    be.reverse();
    be
}

/// Compute a stable 63-bit hash of a value's printed form, used as
/// the `address` field on `ValueRecord::Reference` so capabilities
/// published at the same storage path (or references borrowed at the
/// same site) compare equal across the trace.
///
/// Uses FNV-1a mixing for low-collision determinism (no seeding).
/// The result is masked to the low 63 bits because the downstream
/// `ct-print` converts the writer's `uint64` address into `int64`
/// for the JSON dump and raises `RangeDefect` when the high bit is
/// set — Reference values would otherwise crash the decoder.
fn stable_address_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for byte in s.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    // Mask to i64::MAX so `int64(address)` in ct-print never overflows.
    h & 0x7fffffffffffffff
}

/// Parse a Cadence fixed-point literal (`-1.5`, `0.25`, `1.0`,
/// `12.5`) into the canonical 1e8 scaled integer form Cadence uses
/// internally for `Fix64` / `UFix64`.  Returns `None` for any input
/// that does not parse as a (signed) decimal with at most 8 fractional
/// digits — those cases bubble up to the residual `Raw` fallback in
/// `value_record`.
///
/// Examples:
///
/// * `parse_fixed_point_scaled("-1.5")` → `Some(-150_000_000)`
/// * `parse_fixed_point_scaled("0.25")` → `Some(25_000_000)`
/// * `parse_fixed_point_scaled("1.0")` → `Some(100_000_000)`
/// * `parse_fixed_point_scaled("12.5")` → `Some(1_250_000_000)`
/// * `parse_fixed_point_scaled("12")` → `Some(1_200_000_000)`
fn parse_fixed_point_scaled(value: &str) -> Option<i64> {
    let v = value.trim();
    let (sign, rest) = if let Some(stripped) = v.strip_prefix('-') {
        (-1i64, stripped)
    } else if let Some(stripped) = v.strip_prefix('+') {
        (1i64, stripped)
    } else {
        (1i64, v)
    };
    let (int_part, frac_part) = match rest.split_once('.') {
        Some((i, f)) => (i, f),
        None => (rest, ""),
    };
    if int_part.is_empty() && frac_part.is_empty() {
        return None;
    }
    if !int_part.chars().all(|c| c.is_ascii_digit()) && !int_part.is_empty() {
        return None;
    }
    if !frac_part.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if frac_part.len() > 8 {
        return None;
    }
    let int_val: i64 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().ok()?
    };
    let mut padded = frac_part.to_string();
    while padded.len() < 8 {
        padded.push('0');
    }
    let frac_val: i64 = padded.parse().ok()?;
    let magnitude = int_val.checked_mul(100_000_000)?.checked_add(frac_val)?;
    Some(sign * magnitude)
}

/// Resolve the source path for a step event, dispatching on whether
/// the helper-side `file` field refers to the entry source or a
/// sibling fixture.  The Cadence entry point may `import` a contract
/// declared in a separate `.cdc` file; when the helper emits a step
/// with a different `file` basename, the recorder must surface that
/// step against the imported contract's source path so the trace's
/// path table carries each contract's source independently.
///
/// Returns:
///
/// * The original `source_path` when `file` is empty or matches the
///   entry source's basename.
/// * `<source_path's directory>/<file>` when `file` is a sibling
///   basename (no directory component).
/// * The literal `file` value otherwise (already an absolute or
///   directory-qualified path).
fn resolve_step_source_path(source_path: &Path, file: &str) -> std::path::PathBuf {
    let trimmed = file.trim();
    if trimmed.is_empty() {
        return source_path.to_path_buf();
    }
    let entry_name = source_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if trimmed == entry_name {
        return source_path.to_path_buf();
    }
    // If `file` already carries a directory component (contains '/'),
    // honour it verbatim.
    if trimmed.contains('/') {
        return std::path::PathBuf::from(trimmed);
    }
    // Bare basename: resolve against the entry source's parent dir.
    if let Some(parent) = source_path.parent() {
        parent.join(trimmed)
    } else {
        std::path::PathBuf::from(trimmed)
    }
}

/// Map a Cadence access modifier (`self` / `contract` / `account` /
/// `all`) to the canonical `Access<Tag>` visibility tag the recorder
/// surfaces in the `CadenceAccess:<function>:<Tag>` io_event.
/// Returns `None` for unrecognised modifiers so the recorder can
/// silently skip the io_event when the helper omits the field.
fn visibility_tag(access: &str) -> Option<&'static str> {
    match access {
        "self" => Some("AccessSelf"),
        "contract" => Some("AccessContract"),
        "account" => Some("AccessAccount"),
        "all" => Some("AccessAll"),
        _ => None,
    }
}

/// Strip a single leading + trailing pair of `"` (Cadence prints
/// dictionary keys as JSON strings).
fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
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
    /// FU-Column-Aware-Nav-Flow: paths already registered with their
    /// per-line UTF-8 byte-length tables via
    /// `register_path_with_line_lengths`.  Tracked so we only emit the
    /// `paths.dat` Layout A record once per source file (the first
    /// registration wins per the Nim writer's semantics — a later
    /// re-registration for an already-interned path is silently
    /// dropped, which would lose the line-length table the
    /// column-aware reader needs to map global positions back to
    /// (line, column)).  Mirrors the EVM recorder's
    /// `paths_with_line_lengths` set.
    paths_with_line_lengths: HashSet<PathBuf>,
    /// The subset of those whose table is NON-EMPTY, i.e. the files that
    /// actually have a column axis.
    ///
    /// A column is only addressable in a file the writer sized from its own
    /// per-line table; a file registered with an empty one is sized by the
    /// line-only fallback, where one address is one line — so a column folded
    /// into that address names a LATER LINE, not a column. Sources that are
    /// not on disk at the path the helper names (synthetic NDJSON fixtures,
    /// anything built elsewhere) land here, which makes this the normal case
    /// rather than an edge one.
    paths_with_column_axis: HashSet<PathBuf>,
    /// Whether the writer currently holds an open step for variables to
    /// attach to.
    ///
    /// The writer buffers one step at a time: `register_step` opens it, and
    /// the next step, call, or return flushes it together with every variable
    /// staged in between.  A variable staged while nothing is open has no step
    /// to attach to, so the writer invents one — on a column-aware trace a
    /// zero-delta column nudge, which occupies an exec index that is not a
    /// logical step.  The values ride along on that index and no real step's
    /// `StepValues` ever reports them.
    step_open: bool,
    /// Variables staged while no step was open, held until the next step.
    ///
    /// The Cadence helper emits a binding's value AFTER the `return` of the
    /// call that produced it (`let x = C.foo()` is `call` / `step` / `return`
    /// / `variable x`), and a call's arguments before the callee's first body
    /// step.  Both land in the gap.  They are in scope at the step that
    /// follows, which is where trace-events.md §"Value Stream Events" puts
    /// them: `StepValues` is "all variable values visible at this step".
    deferred_vars: Vec<(String, ValueRecord)>,
}

/// FU-Column-Aware-Nav-Flow: compute the per-line UTF-8 byte-length
/// table required by the `paths.dat` Layout A record (column-aware
/// mode).  `line_lengths[i]` is the byte count of source line `i+1`
/// (1-based, matching the CTFS spec), excluding the trailing `\n`.  A
/// file that doesn't end with `\n` still has its final line counted.
/// Identical algorithm to the EVM / Cairo recorders'
/// `compute_line_lengths` helper — extracted here so the Flow
/// recorder can register multi-file (imported) Cadence sources.
fn compute_line_lengths(source: &str) -> Vec<u32> {
    let mut lengths: Vec<u32> = Vec::new();
    let mut line_start: usize = 0;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            lengths.push((i - line_start) as u32);
            line_start = i + 1;
        }
    }
    if line_start < source.len() {
        lengths.push((source.len() - line_start) as u32);
    }
    lengths
}

impl CadenceTracer {
    /// FU-Column-Aware-Nav-Flow: register `path` with its per-line
    /// UTF-8 byte-length table via the `paths.dat` Layout A entry
    /// point, once per recorder lifetime.  Subsequent calls for the
    /// same path are no-ops.
    ///
    /// Reads the source file from disk to compute the line lengths.
    /// Soft-fails (logged to stderr) if the file can't be read or
    /// the FFI rejects the call — the trace remains usable, but
    /// columns on that file fall back to `None` at read time.
    /// Mirrors the EVM recorder's `ensure_path_with_line_lengths`.
    fn ensure_path_with_line_lengths(&mut self, path: &Path) {
        if self.paths_with_line_lengths.contains(path) {
            return;
        }
        // Try to read the source from disk.  When the file isn't
        // available (e.g. tests using synthetic NDJSON whose paths
        // don't exist on the filesystem), register with an empty
        // line-lengths slice so the path still gets the
        // column-aware-compatible `paths.dat` record — the writer
        // treats an empty slice as "no per-line data, fall back to
        // None at read time" (see
        // `NimTraceWriter::register_path_with_line_lengths`).
        let line_lengths = match std::fs::read_to_string(path) {
            Ok(src) => compute_line_lengths(&src),
            Err(_) => Vec::new(),
        };
        if let Err(err) =
            TraceWriter::register_path_with_line_lengths(&mut *self.writer, path, &line_lengths)
        {
            eprintln!(
                "[codetracer-flow-recorder] register_path_with_line_lengths failed for {}: {} \
                 (column resolution will fall back to None for this file)",
                path.display(),
                err,
            );
        }
        if !line_lengths.is_empty() {
            self.paths_with_column_axis.insert(path.to_path_buf());
        }
        self.paths_with_line_lengths.insert(path.to_path_buf());
    }

    /// The column to record for a step on `path`, or `None` when that file has
    /// no column axis to place one on.
    ///
    /// Offering one anyway does not fail — it resolves to a different line,
    /// which reads back as a plausible position that the program never
    /// executed.
    fn addressable_column(&self, path: &Path, column: Option<u32>) -> Option<Line> {
        if self.paths_with_column_axis.contains(path) {
            column.map(|c| Line(c as i64))
        } else {
            None
        }
    }

    /// Emit a step at `(path, line, column)` and attach to it every variable
    /// that was staged while no step was open.
    ///
    /// Every step the recorder emits goes through here, so the "a variable is
    /// always registered against a step" invariant holds for the resource
    /// lifecycle events as much as for plain `step` records.
    fn open_step(&mut self, path: &Path, line: u32, column: Option<u32>) {
        let addressable = self.addressable_column(path, column);
        TraceWriter::register_step_with_column(
            &mut *self.writer,
            path,
            Line(line as i64),
            addressable,
        );
        self.step_open = true;
        for (name, value) in std::mem::take(&mut self.deferred_vars) {
            self.write_variable(&name, value);
        }
    }

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
            paths_with_line_lengths: HashSet::new(),
            paths_with_column_axis: HashSet::new(),
            step_open: false,
            deferred_vars: Vec::new(),
        };

        // -- 3. Initialise output files --
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        // CTFS multi-stream container.
        let events_filename = "trace.bin";
        let events_path = out_dir.join(events_filename);

        TraceWriter::begin_writing_trace_events(&mut *tracer.writer, &events_path)
            .map_err(|e| eyre!("{e}"))?;

        // FU-Column-Aware-Nav-Flow: opt the canonical CTFS writer into
        // column-aware step encoding *before* the first
        // `register_step` / `start` call.  `enable_column_aware_steps`
        // is sticky for the lifetime of the trace and gates the
        // writer's `DeltaColumn` (tag 0x07) emission path plus the
        // `meta.dat` bit 4 flag (`FLAG_HAS_COLUMN_AWARE_STEPS`).
        // Even when individual steps resolve to `column == None`
        // (e.g. the Go helper omitted the column field for some
        // statement, or resource lifecycle events that don't carry a
        // column), downstream readers rely on the flag to decide
        // whether to surface a column field at all — mirrors the
        // Solana / EVM / Cairo recorder contract.
        TraceWriter::enable_column_aware_steps(&mut *tracer.writer);

        // M-capability-flags: Cadence's Go helper resolves each
        // statement to a sharp `(line, column)` pair so per-column
        // breakpoints and per-column motions are both meaningful.
        // Advertise both so the GUI shows the per-column UI.
        tracer.writer.enable_column_breakpoints_support();
        tracer.writer.enable_column_motions_support();

        // FU-Column-Aware-Nav-Flow: register the entry source path's
        // per-line byte-length table BEFORE `TraceWriter::start`.
        // `start` internally interns the path (without line-length
        // data), and a later `register_path_with_line_lengths` for
        // an already-interned path is silently dropped by the Nim
        // writer — that drops the line-length table needed by the
        // reader's `decodeGlobalPositionIndex`, so the per-step
        // column field never surfaces in ct-print.
        tracer.ensure_path_with_line_lengths(source_path);

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
        tracer
            .writer
            .write_meta_dat("codetracer-flow-recorder")
            .map_err(|e| eyre!("{e}"))?;
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
            paths_with_line_lengths: HashSet::new(),
            paths_with_column_axis: HashSet::new(),
            step_open: false,
            deferred_vars: Vec::new(),
        };

        // Initialise output files.
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        // CTFS multi-stream container.
        let events_filename = "trace.bin";
        let events_path = out_dir.join(events_filename);

        TraceWriter::begin_writing_trace_events(&mut *tracer.writer, &events_path)
            .map_err(|e| eyre!("{e}"))?;

        // FU-Column-Aware-Nav-Flow: opt into column-aware encoding
        // before the first `register_step` / `start` call.  See the
        // matching block in `trace_program` for the full rationale.
        TraceWriter::enable_column_aware_steps(&mut *tracer.writer);
        // M-capability-flags: mirror the `trace_program` path.
        tracer.writer.enable_column_breakpoints_support();
        tracer.writer.enable_column_motions_support();
        tracer.ensure_path_with_line_lengths(source_path);

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
        tracer
            .writer
            .write_meta_dat("codetracer-flow-recorder")
            .map_err(|e| eyre!("{e}"))?;
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
            paths_with_line_lengths: HashSet::new(),
            paths_with_column_axis: HashSet::new(),
            step_open: false,
            deferred_vars: Vec::new(),
        };

        // FU-Column-Aware-Nav-Flow: even on the
        // `NonStreamingTraceWriter` test double the trait's default
        // `enable_column_aware_steps` / `register_path_with_line_lengths`
        // impls are no-ops, but we keep the call shape uniform across
        // all three `CadenceTracer` constructors so the column-aware
        // contract is documented in one place.
        TraceWriter::enable_column_aware_steps(&mut *tracer.writer);
        // M-capability-flags: keep the trait call shape uniform on
        // the test double so the capability contract is documented
        // in one place too.
        tracer.writer.enable_column_breakpoints_support();
        tracer.writer.enable_column_motions_support();
        tracer.ensure_path_with_line_lengths(source_path);

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
                TraceEvent::Step { file, line, column } => {
                    // Resolve the step's source path: if the helper-side
                    // `file` field is present and refers to a sibling
                    // fixture (a different file from the main entry-point
                    // source), use that path so multi-file imports
                    // surface as distinct entries in the trace's path
                    // table.  Closes the M10 `contracts_imports_test`
                    // deliverable: import resolution is visible via the
                    // per-step source path.  Falls back to the entry
                    // source when `file` is empty or matches the entry
                    // file's basename.
                    let resolved = resolve_step_source_path(source_path, file);
                    // FU-Column-Aware-Nav-Flow: register the imported
                    // source's per-line byte-length table on first
                    // contact before emitting the step.  See
                    // `ensure_path_with_line_lengths` for the
                    // already-interned / soft-fail contract.
                    self.ensure_path_with_line_lengths(&resolved);
                    // FU-Column-Aware-Nav-Flow: the helper-side column is
                    // already 1-based (the Go helper applies the `+ 1`
                    // adjustment from Cadence's 0-based
                    // `ast.Position.Column`), so it goes to `open_step`
                    // unchanged; `addressable_column` decides whether the
                    // file can carry it.  When the helper omits the column
                    // (legacy NDJSON without column info) the step is
                    // line-only — `enable_column_aware_steps` remains set, so
                    // the trace's column-aware flag survives and individual
                    // steps with `None` fall back cleanly at read time.
                    self.open_step(&resolved, *line, *column);
                }
                TraceEvent::Variable {
                    name,
                    value,
                    cadence_type,
                } => {
                    let val_record = self.value_record(value, cadence_type.as_deref());
                    self.register_typed_variable(name, val_record);
                }
                TraceEvent::Call {
                    name,
                    args,
                    access,
                    script,
                } => {
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
                    // A call flushes the writer's pending step, so nothing is
                    // open again until the callee's first body step.
                    self.step_open = false;

                    // Emit a tagged visibility io_event when the
                    // helper-side `access` discriminator is present so
                    // the call frame's source-declared visibility
                    // survives the multi-stream writer.  Mirrors the
                    // pre/post `error_kind` dispatch in `Error`.
                    if let Some(tag) = access.as_deref().and_then(visibility_tag) {
                        TraceWriter::register_special_event(
                            &mut *self.writer,
                            EventLogKind::TraceLogEvent,
                            &format!("CadenceAccess:{}:{}", name, tag),
                            &format!("CadenceAccess:{}:{}", name, tag),
                        );
                    }

                    // Emit a tagged script-entry io_event when the
                    // helper flags this call as the Cadence script
                    // entry point.  Surfaces the boundary on the
                    // io-event channel so the strict pin can pin
                    // the script form independently of a same-named
                    // transaction `main`.
                    if *script {
                        TraceWriter::register_special_event(
                            &mut *self.writer,
                            EventLogKind::TraceLogEvent,
                            &format!("CadenceScriptEntry:{}", name),
                            &format!("CadenceScriptEntry:{}", name),
                        );
                    }
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
                    // A return flushes the writer's pending step, so the
                    // binding the helper reports next belongs to the caller's
                    // next step, not to the callee's last one.
                    self.step_open = false;
                }
                TraceEvent::Error {
                    message,
                    error_kind,
                } => {
                    // Distinguish Cadence pre-condition / post-condition
                    // violations from a user-issued `panic`.  Pre/post
                    // failures route through `EventLogKind::TraceLogEvent`
                    // (which becomes `ioStderr` in the multi-stream
                    // container) with a dedicated `CadencePreCondition` /
                    // `CadencePostCondition` metadata tag, so consumers
                    // can grep them apart from a generic runtime error;
                    // the metadata also pins the failure mode in the
                    // event log itself, surviving the lossy `io_kind`
                    // bucketing.
                    let (log_kind, metadata) = match error_kind.as_deref() {
                        Some("pre") => (EventLogKind::TraceLogEvent, "CadencePreCondition"),
                        Some("post") => (EventLogKind::TraceLogEvent, "CadencePostCondition"),
                        Some("panic") | None | Some(_) => {
                            (EventLogKind::Error, "CadenceRuntimeError")
                        }
                    };
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        log_kind,
                        metadata,
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
                    column,
                } => {
                    // FU-Column-Aware-Nav-Flow: emit a column-aware
                    // step at the resource creation site.  When the
                    // helper omits the column (resource lifecycle
                    // events that don't carry a column) we pass
                    // `None` — the column-aware flag still surfaces
                    // on the trace.
                    self.open_step(source_path, *line, *column);

                    // Synthesise the implicit `<Type>.init` Cadence
                    // initializer in the function table so the function
                    // symbol surfaces alongside the explicit `call`
                    // entries.  Cadence resource creation always
                    // dispatches through the type's `init` member, but
                    // the helper-side NDJSON does not emit a `call`
                    // event for it (the `resource_create` channel is
                    // the lifecycle marker).  Registering the function
                    // name only — without a synthesised `call_entry` /
                    // `call_exit` — keeps the calls / event counts
                    // intact and matches how the Move 1.46 recorder
                    // surfaces compiler-generated members.
                    let init_fn_name = format!("{}.init", resource_type);
                    let _ = TraceWriter::ensure_function_id(
                        &mut *self.writer,
                        &init_fn_name,
                        source_path,
                        Line(*line as i64),
                    );

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
                    column,
                } => {
                    // FU-Column-Aware-Nav-Flow: column-aware step at
                    // the move site.  See `ResourceCreate` above.
                    self.open_step(source_path, *line, *column);

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
                    column,
                } => {
                    // FU-Column-Aware-Nav-Flow: column-aware step at
                    // the destroy site.  See `ResourceCreate` above.
                    self.open_step(source_path, *line, *column);

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

                // --- M10 events ---
                TraceEvent::ResourceOwnerChange {
                    resource_type,
                    uuid,
                    from_owner,
                    to_owner,
                    file: _,
                    line,
                    column,
                } => {
                    // FU-Column-Aware-Nav-Flow: column-aware step at
                    // the owner-change site so the trace can line the
                    // ownership transition up against source.  See
                    // `ResourceCreate` above for the `column = None`
                    // contract.
                    self.open_step(source_path, *line, *column);

                    // Tagged owner-change io event.  Multi-stream io
                    // writer drops the metadata field, so the
                    // `ResourceOwnerChange:` tag is duplicated into
                    // the content text payload — that is the only
                    // surface the strict tests can pin against
                    // (`io_kind` + `text`).  The text format
                    // (`<Type>#<uuid>: <from> -> <to>`) parses cleanly
                    // back into the four fields downstream tooling
                    // needs.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!("ResourceOwnerChange:{}#{}", resource_type, uuid),
                        &format!(
                            "ResourceOwnerChange:{}#{}: {} -> {}",
                            resource_type, uuid, from_owner, to_owner
                        ),
                    );
                }

                TraceEvent::ForceNilUnwrap { context } => {
                    // Tag carries the literal `ForceNilUnwrap:` prefix
                    // in the text payload so the multi-stream writer
                    // (which drops the metadata field) preserves the
                    // distinguishing tag in the strict pin's `text`
                    // surface.  Symmetric with `ResourceOwnerChange`.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!("ForceNilUnwrap:{}", context),
                        &format!("ForceNilUnwrap:{}", context),
                    );
                }

                TraceEvent::CompositeKind { kind, type_name } => {
                    // Map the helper-side `kind` discriminator onto
                    // the canonical Cadence composite-kind tag (the
                    // `interpreter.CompositeKind` enum surface, see
                    // `onflow/cadence runtime/sema/check_composite.go`).
                    // Unknown kinds are silently dropped — the recorder
                    // never emits a malformed tag.
                    let tag = match kind.as_str() {
                        "structure" => Some("CompositeKindStructure"),
                        "resource" => Some("CompositeKindResource"),
                        "event" => Some("CompositeKindEvent"),
                        _ => None,
                    };
                    if let Some(tag) = tag {
                        TraceWriter::register_special_event(
                            &mut *self.writer,
                            EventLogKind::TraceLogEvent,
                            &format!("{}:{}", tag, type_name),
                            &format!("{}:{}", tag, type_name),
                        );
                    }
                }

                TraceEvent::AnyTypeBind {
                    static_type,
                    runtime_type,
                    varname,
                } => {
                    // Surface both the static (`AnyResource` /
                    // `AnyStruct` / `AnyAuthAccount` /
                    // `AnyPublicAccount`) supertype and the concrete
                    // runtime type id of the bound value through the
                    // `CadenceAnyType:` tag channel.  The varname
                    // suffix lets the strict pin cross-reference
                    // against the step-local that carries the typed
                    // ValueRecord variant.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!(
                            "CadenceAnyType:{}:{}:{}",
                            static_type, runtime_type, varname
                        ),
                        &format!(
                            "CadenceAnyType:{}:{}:{}",
                            static_type, runtime_type, varname
                        ),
                    );
                }

                TraceEvent::TypeAlias {
                    alias_name,
                    underlying_type,
                } => {
                    // Cadence 1.x type-alias declarations are
                    // compile-time-only renamings — the alias name is
                    // not visible in the bound value's typed
                    // ValueRecord variant.  Surface the alias →
                    // underlying linkage via a tagged
                    // `CadenceTypeAlias:<Alias>:<Underlying>` io event
                    // so downstream consumers can resolve aliases on
                    // the io-event channel.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!("CadenceTypeAlias:{}:{}", alias_name, underlying_type),
                        &format!("CadenceTypeAlias:{}:{}", alias_name, underlying_type),
                    );
                }

                TraceEvent::AttachmentAttach {
                    attachment_type,
                    target_type,
                } => {
                    // Cadence 1.x attach-site metadata.  Surface the
                    // (attachment, target) pair via a tagged
                    // `CadenceAttachmentAttach:<Att>:<Target>` io
                    // event so the attachment-to-base linkage is
                    // independently visible from the typed-local
                    // snapshot of the attachment value.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!(
                            "CadenceAttachmentAttach:{}:{}",
                            attachment_type, target_type
                        ),
                        &format!(
                            "CadenceAttachmentAttach:{}:{}",
                            attachment_type, target_type
                        ),
                    );
                }

                TraceEvent::AttachmentRemove {
                    attachment_type,
                    target_type,
                } => {
                    // Symmetric counterpart to `AttachmentAttach`.
                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::TraceLogEvent,
                        &format!(
                            "CadenceAttachmentRemove:{}:{}",
                            attachment_type, target_type
                        ),
                        &format!(
                            "CadenceAttachmentRemove:{}:{}",
                            attachment_type, target_type
                        ),
                    );
                }

                TraceEvent::Emit { name, fields } => {
                    // Stage each field as a typed call-arg-style local
                    // (so the locals pane carries the typed
                    // ValueRecord variants), then emit the tagged
                    // `CadenceEmit:` io event whose text payload
                    // serialises (name, [(field, type, value), ...]).
                    // The text shape is stable so the strict pin in
                    // `tests/test_tracer.rs` can `assert_eq!` against
                    // it.
                    let mut payload = format!("CadenceEmit:{}(", name);
                    for (i, f) in fields.iter().enumerate() {
                        if i > 0 {
                            payload.push_str(", ");
                        }
                        payload.push_str(&f.name);
                        payload.push_str(": ");
                        payload.push_str(&f.value);
                    }
                    payload.push(')');

                    // Emit each field as a typed local so the value
                    // surfaces as ValueRecord::Struct/Int/String/...
                    // (whatever its concrete cadence_type maps to)
                    // — the spec calls out "event payload captured
                    // as a typed ValueRecord::Struct with all
                    // parameters preserved", which we surface via
                    // the standard `value_record` typed-variant path.
                    for f in fields {
                        let val = self.value_record(&f.value, f.cadence_type.as_deref());
                        let var_name = format!("emit:{}.{}", name, f.name);
                        self.register_typed_variable(&var_name, val);
                    }

                    TraceWriter::register_special_event(
                        &mut *self.writer,
                        EventLogKind::EvmEvent,
                        &format!("CadenceEmit:{}", name),
                        &payload,
                    );
                }
            }
        }

        // No step follows, so these have nowhere better to go. Handing them to
        // the writer anyway keeps them in `values.dat`, where a reader that
        // walks the value stream can still find them; holding them here would
        // discard them outright.
        for (name, value) in std::mem::take(&mut self.deferred_vars) {
            self.write_variable(&name, value);
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

    /// Lazily register a Cadence type and return its `TypeId`.
    ///
    /// The Nim writer interns by `(kind, lang_type)`, so re-issuing the
    /// same call is cheap; we still memoise on the Rust side so the
    /// `value_record` recursion does not pay an FFI hop per element.
    fn ensure_type(&mut self, kind: TypeKind, lang_type: &str) -> TypeId {
        if let Some(&id) = self.type_ids.get(lang_type) {
            return id;
        }
        let id = TraceWriter::ensure_type_id(&mut *self.writer, kind, lang_type);
        self.type_ids.insert(lang_type.to_string(), id);
        id
    }

    /// Decode a NDJSON `value` string into a typed `ValueRecord`, using
    /// the helper-side `cadence_type` discriminator when present.
    ///
    /// Spec: every typed leaf MUST surface as its dedicated
    /// `ValueRecord` variant — `Bool` as `Bool { b }`, `String` as
    /// `String { text }`, integer types as `Int { i }`,
    /// arrays as `Sequence { elements }`, dictionaries as a
    /// `Sequence` of `Tuple` key/value pairs, structs / resources as
    /// `Struct { field_values }`, and optionals as `Variant { Some|None }`.
    /// Falling back to `Raw` violates
    /// `metacraft-specs/policies/recorder-test-requirements.md` §1.
    /// Only genuinely unrecognised types fall through to a stringified
    /// `Raw` representation — and even those should be tightened up as
    /// the recorder learns more Cadence types.
    fn value_record(&mut self, value: &str, cadence_type: Option<&str>) -> ValueRecord {
        let trimmed_type = cadence_type.map(|t| t.trim());
        match trimmed_type {
            // -------- Cadence enums: `enum:<EnumName>:<BackingType>` ---
            // Marker scheme used by NDJSON fixtures to flag a Cadence
            // enum case to the recorder.  Cadence enums have the shape
            // `enum Color: UInt8 { case red; case green; case blue }`,
            // so the marker captures both the enum name (used as the
            // type-id and the discriminator-prefix sanity check) and
            // the backing-integer width (used for the inner contents'
            // `ValueRecord::Int` width).  The enum case itself is
            // surfaced as `ValueRecord::Variant { discriminator:
            // "<EnumName>.<case>", contents: ValueRecord::None,
            // type_id }` — the spec calls out
            // `Variant { name: "Color.red", fields: [] }`, and an
            // empty inner `None` payload is the canonical "no fields"
            // shape used by the Move recorder.
            Some(t) if t.starts_with("enum:") => {
                let rest = &t["enum:".len()..];
                let (_enum_name, _backing) = match rest.split_once(':') {
                    Some((n, b)) => (n, b),
                    None => (rest, "Int"),
                };
                let variant_id = self.ensure_type(TypeKind::Variant, t);
                let inner_id = self.ensure_type(TypeKind::None, "EnumCase");
                ValueRecord::Variant {
                    discriminator: value.trim().to_string(),
                    contents: Box::new(ValueRecord::None { type_id: inner_id }),
                    type_id: variant_id,
                }
            }

            // -------- Optionals: `T?` ----------------------------------
            Some(t) if t.ends_with('?') && t.len() > 1 => {
                let inner_type = &t[..t.len() - 1];
                let variant_id = self.ensure_type(TypeKind::Variant, t);
                let v = value.trim();
                if v.is_empty() || v == "nil" || v == "Optional()" {
                    // Cadence absent optional.  Carry an inner `None`
                    // payload so consumers walking `contents` see a
                    // typed `ValueRecord::None` rather than an empty
                    // box.
                    let inner_id = self.ensure_type(TypeKind::None, inner_type);
                    ValueRecord::Variant {
                        discriminator: "None".to_string(),
                        contents: Box::new(ValueRecord::None { type_id: inner_id }),
                        type_id: variant_id,
                    }
                } else {
                    let inner = self.value_record(v, Some(inner_type));
                    ValueRecord::Variant {
                        discriminator: "Some".to_string(),
                        contents: Box::new(inner),
                        type_id: variant_id,
                    }
                }
            }

            // -------- Arrays: `[T]` ------------------------------------
            Some(t) if t.starts_with('[') && t.ends_with(']') && t.len() >= 2 => {
                let inner_type = &t[1..t.len() - 1];
                let seq_id = self.ensure_type(TypeKind::Seq, t);
                let elements = parse_cadence_array(value)
                    .into_iter()
                    .map(|elem| self.value_record(&elem, Some(inner_type)))
                    .collect();
                ValueRecord::Sequence {
                    elements,
                    is_slice: false,
                    type_id: seq_id,
                }
            }

            // -------- Dictionaries: `{K: V}` ---------------------------
            Some(t) if t.starts_with('{') && t.ends_with('}') && t.contains(':') => {
                // Strip the outer braces, then split on the first ':'
                // to recover (key_type, value_type).  Cadence dict
                // values render as JSON-ish objects: `{"apple": 30}`.
                let inner = &t[1..t.len() - 1];
                let (key_type, val_type) = match inner.split_once(':') {
                    Some((k, v)) => (k.trim(), v.trim()),
                    None => ("String", "Int"),
                };
                let dict_id = self.ensure_type(TypeKind::Seq, t);
                let pair_id = self.ensure_type(TypeKind::Tuple, "DictEntry");
                let pairs = parse_cadence_dict(value);
                let elements = pairs
                    .into_iter()
                    .map(|(k, v)| {
                        let key_value = self.value_record(&k, Some(key_type));
                        let val_value = self.value_record(&v, Some(val_type));
                        ValueRecord::Tuple {
                            elements: vec![key_value, val_value],
                            type_id: pair_id,
                        }
                    })
                    .collect();
                ValueRecord::Sequence {
                    elements,
                    is_slice: false,
                    type_id: dict_id,
                }
            }

            // -------- Cadence Path values ------------------------------
            // `Path` (and the more specific `StoragePath` /
            // `PublicPath` / `PrivatePath`) values render in Cadence as
            // `/<domain>/<identifier>` — e.g. `/storage/Vault`.
            // Surface as a typed `Struct { String "domain", String
            // "identifier" }` (the canonical Cadence Path shape) so
            // downstream consumers can resolve the domain and
            // identifier without re-parsing the slash form.
            Some(t)
                if matches!(
                    t,
                    "Path" | "StoragePath" | "PublicPath" | "PrivatePath" | "CapabilityPath"
                ) =>
            {
                let struct_id = self.ensure_type(TypeKind::Struct, t);
                let domain_id = self.ensure_type(TypeKind::String, "PathDomain");
                let ident_id = self.ensure_type(TypeKind::String, "PathIdentifier");
                let v = value.trim();
                let stripped = v.strip_prefix('/').unwrap_or(v);
                let (domain, identifier) = match stripped.split_once('/') {
                    Some((d, i)) => (d.to_string(), i.to_string()),
                    None => (stripped.to_string(), String::new()),
                };
                ValueRecord::Struct {
                    field_values: vec![
                        ValueRecord::String {
                            text: domain,
                            type_id: domain_id,
                        },
                        ValueRecord::String {
                            text: identifier,
                            type_id: ident_id,
                        },
                    ],
                    type_id: struct_id,
                }
            }

            // -------- Cadence Address literals -------------------------
            // `Address` values are 8-byte unsigned integers shown in
            // hex form (e.g. `0x01`, `0xf8d6e0586b0a20c7`).  When the
            // value fits in `i64` (signed) we surface it as
            // `ValueRecord::Int` (matching the M2 hex-form convention);
            // otherwise we encode it as a `ValueRecord::BigInt` with
            // the 8 raw big-endian bytes preserved.  Closes the M9
            // known limitation that `Address` fell back to `Raw`.
            Some("Address") => {
                let type_id = self.ensure_type(TypeKind::Int, "Address");
                let v = value.trim();
                let hex = v
                    .strip_prefix("0x")
                    .or_else(|| v.strip_prefix("0X"))
                    .unwrap_or(v);
                if let Ok(u) = u64::from_str_radix(hex, 16) {
                    if u <= i64::MAX as u64 {
                        ValueRecord::Int {
                            i: u as i64,
                            type_id,
                        }
                    } else {
                        // 8-byte big-endian unsigned magnitude.
                        ValueRecord::BigInt {
                            b: u.to_be_bytes().to_vec(),
                            negative: false,
                            type_id,
                        }
                    }
                } else {
                    ValueRecord::Raw {
                        r: value.to_string(),
                        type_id,
                    }
                }
            }

            // -------- Wide integers: > 64-bit ---------------------------
            // Cadence supports integer types up to 256 bits.  Widths
            // that do not fit in `i64` (signed) MUST surface as a typed
            // `ValueRecord::BigInt` so the exact value is preserved
            // across the trace (the new `writeBigInt` encoder from
            // workspace commit `25435ac` is the closing dependency).
            // Closes the M9 known limitation that `Int128` / `UInt128`
            // / `UInt256` fell back to `ValueRecord::Raw`.
            Some(t)
                if matches!(
                    t,
                    "Int128" | "Int256" | "UInt128" | "UInt256" | "Word128" | "Word256"
                ) =>
            {
                let type_id = self.ensure_type(TypeKind::Int, t);
                value_to_bigint(value, type_id)
            }

            // -------- Capability and Reference types -------------------
            // Cadence references (`&Vault`, `&{Provider}`,
            // `auth(Withdraw) &Vault`) and capabilities
            // (`Capability<&Vault>`) are pointer-like values backed by
            // an underlying stored resource.  Surface them as
            // `ValueRecord::Reference { dereferenced, address, mutable,
            // type_id }` so the typed shape from the new
            // `beginReference` encoder (workspace commit `25435ac`) is
            // preserved end-to-end.
            //
            //   * `address` carries a stable hash of the value's
            //     printed form so capabilities published at the same
            //     storage path compare equal across the trace.
            //   * `mutable` is `true` for entitlement-authorized
            //     references (`auth(...) &T`), `false` otherwise.
            //   * `dereferenced` is a typed `String` payload with the
            //     printed form of the borrowed value (e.g.
            //     `/storage/Vault`).  A future enhancement could route
            //     this through a per-resource snapshot.
            Some(t)
                if t.starts_with('&')
                    || t.starts_with("auth(")
                    || t.starts_with("Capability<")
                    || t == "Capability" =>
            {
                let ref_type_id = self.ensure_type(TypeKind::Ref, t);
                let inner_type_id = self.ensure_type(TypeKind::String, "ReferencePayload");
                let mutable = t.starts_with("auth(");
                let address = stable_address_hash(value);
                ValueRecord::Reference {
                    dereferenced: Box::new(ValueRecord::String {
                        text: value.to_string(),
                        type_id: inner_type_id,
                    }),
                    address,
                    mutable,
                    type_id: ref_type_id,
                }
            }

            // -------- Resource references: `@Type` ---------------------
            // The Cadence printed form is `@<Type>#<uuid>` (e.g.
            // `@Coin#7001`).  Surface this as a typed `Struct` carrying
            // the resource's identifying fields so downstream consumers
            // can query lineage by uuid/owner without re-parsing the
            // printed form.  Spec: `metacraft-specs/policies/
            // recorder-test-requirements.md` §1 (typed payload).
            //
            // M10 (resources_full_test) extension: the printed form
            // optionally carries a trailing `@<owner>` segment
            // (`@Vault#5001@0x01`).  When present, the typed Struct
            // additionally carries the implicit `owner` field as a
            // third Struct field so move-operator snapshots
            // (`let b <- a`, swap, shift, force-unwrap, nested
            // destroy) preserve the owner transition end-to-end.
            Some(t) if t.starts_with('@') => {
                let resource_name = &t[1..];
                let struct_id = self.ensure_type(TypeKind::Struct, t);
                let type_field_id = self.ensure_type(TypeKind::String, "ResourceType");
                let uuid_field_id = self.ensure_type(TypeKind::Int, "ResourceUuid");
                let (parsed_type, parsed_uuid, parsed_owner) = parse_resource_handle(value)
                    .unwrap_or_else(|| (resource_name.to_string(), 0, None));
                let mut field_values = vec![
                    ValueRecord::String {
                        text: parsed_type,
                        type_id: type_field_id,
                    },
                    ValueRecord::Int {
                        i: parsed_uuid as i64,
                        type_id: uuid_field_id,
                    },
                ];
                if let Some(owner) = parsed_owner {
                    let owner_field_id = self.ensure_type(TypeKind::String, "ResourceOwner");
                    field_values.push(ValueRecord::String {
                        text: owner,
                        type_id: owner_field_id,
                    });
                }
                ValueRecord::Struct {
                    field_values,
                    type_id: struct_id,
                }
            }

            // -------- Structs: `S.Point` (and other named struct types) -
            // Cadence prints structs as `Name(field: val, ...)`.  We
            // recognise the discriminator via the printed form rather
            // than the type name (no leading `[`, `{`, `@`, no `?`,
            // contains `(...)` and a `:` separator).
            Some(t)
                if value.contains('(')
                    && value.contains(')')
                    && value.contains(':')
                    && !t.starts_with('[')
                    && !t.starts_with('{')
                    && !t.starts_with('@')
                    && !t.ends_with('?') =>
            {
                let struct_id = self.ensure_type(TypeKind::Struct, t);
                let fields = parse_cadence_struct(value);
                let field_values = fields
                    .into_iter()
                    .map(|(_field_name, field_value)| {
                        // Field cadence_type is unknown without a schema
                        // — best-effort numeric parse, fall back to
                        // String so the value is still typed.
                        if let Ok(i) = field_value.parse::<i64>() {
                            ValueRecord::Int {
                                i,
                                type_id: self.ensure_type(TypeKind::Int, "Int"),
                            }
                        } else {
                            ValueRecord::String {
                                text: field_value,
                                type_id: self.ensure_type(TypeKind::String, "String"),
                            }
                        }
                    })
                    .collect();
                ValueRecord::Struct {
                    field_values,
                    type_id: struct_id,
                }
            }

            // Cadence Bools come as the literal strings "true" / "false".
            Some("Bool") => {
                let type_id = self.ensure_type(TypeKind::Bool, "Bool");
                match value {
                    "true" => ValueRecord::Bool { b: true, type_id },
                    "false" => ValueRecord::Bool { b: false, type_id },
                    _ => ValueRecord::Raw {
                        r: value.to_string(),
                        type_id,
                    },
                }
            }
            // Cadence Strings round-trip verbatim.
            Some("String") => {
                let type_id = self.ensure_type(TypeKind::String, "String");
                ValueRecord::String {
                    text: value.to_string(),
                    type_id,
                }
            }
            // -------- Cadence Fix64 / UFix64 fixed-point ---------------
            // Cadence fixed-point types carry 8 decimal places of
            // precision (`UFix64.max == 184467440737.09551615`).  The
            // canonical scaled integer form (value * 10^8) preserves
            // the exact decimal payload across the trace; the type-id
            // metadata (`Fix64` / `UFix64` lang_type) carries the
            // scaling factor implicitly.  Surface as
            // `ValueRecord::Int` (matching the M2 numeric-width
            // convention for typed leaves) so the decoder sees a
            // typed scalar rather than the residual `Raw` fallback
            // that fractional decimals previously hit.  Closes the
            // M9 known limitation that `Fix64` / `UFix64` fell back
            // to `Raw` (see the `emit:Transfer.amount` strict pin in
            // `test_events_emit_test_via_ct_print_full`).
            Some(t) if matches!(t, "Fix64" | "UFix64") => {
                let type_id = self.ensure_type(TypeKind::Int, t);
                let v = value.trim();
                if let Some(scaled) = parse_fixed_point_scaled(v) {
                    ValueRecord::Int { i: scaled, type_id }
                } else {
                    ValueRecord::Raw {
                        r: value.to_string(),
                        type_id,
                    }
                }
            }

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
                let type_id = self.ensure_type(TypeKind::Int, t);
                if let Ok(i) = value.parse::<i64>() {
                    ValueRecord::Int { i, type_id }
                } else {
                    ValueRecord::Raw {
                        r: value.to_string(),
                        type_id,
                    }
                }
            }
            // No (or unknown) `cadence_type` discriminator.  Best-effort
            // shape recognition from the value text:
            //
            //   * `[...]`  surfaces as `ValueRecord::Sequence` with each
            //              element decoded recursively (no inner-type
            //              hint, so leaves fall through this same arm).
            //   * `{...}`  surfaces as `ValueRecord::Sequence` of
            //              `Tuple { key, value }` pairs, matching the
            //              typed-dict shape used by the
            //              `Some(t) if t.starts_with('{')` branch.
            //   * numeric  surfaces as `ValueRecord::Int { i }`.
            //
            // Only genuinely unrecognised payloads fall through to the
            // residual `Raw` representation so the ct-print decoder
            // treats the bytes as opaque rather than as a typed String.
            // This closes the long-standing recorder bug where Cadence
            // array / dict literals reaching the recorder without a
            // `cadence_type` discriminator were forced into
            // `ValueRecord::Raw`.
            _ => {
                let lang = trimmed_type.unwrap_or("Int");
                let v = value.trim();
                if v.starts_with('[') && v.ends_with(']') && v.len() >= 2 {
                    let seq_id = self.ensure_type(TypeKind::Seq, "[]");
                    let elements = parse_cadence_array(value)
                        .into_iter()
                        .map(|elem| self.value_record(&elem, None))
                        .collect();
                    ValueRecord::Sequence {
                        elements,
                        is_slice: false,
                        type_id: seq_id,
                    }
                } else if v.starts_with('{')
                    && v.ends_with('}')
                    && v.len() >= 2
                    && (v == "{}" || v.contains(':'))
                {
                    let dict_id = self.ensure_type(TypeKind::Seq, "{}");
                    let pair_id = self.ensure_type(TypeKind::Tuple, "DictEntry");
                    let elements = parse_cadence_dict(value)
                        .into_iter()
                        .map(|(k, val)| {
                            let key_value = self.value_record(&k, None);
                            let val_value = self.value_record(&val, None);
                            ValueRecord::Tuple {
                                elements: vec![key_value, val_value],
                                type_id: pair_id,
                            }
                        })
                        .collect();
                    ValueRecord::Sequence {
                        elements,
                        is_slice: false,
                        type_id: dict_id,
                    }
                } else {
                    let type_id = self.ensure_type(TypeKind::Int, lang);
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
    }

    /// Stage a step-local variable against the open step, or hold it for the
    /// next one when no step is open.
    ///
    /// See `step_open` for what a variable staged into the gap costs: the
    /// writer parks it on an exec index that is not a logical step, and no
    /// step's `StepValues` reports it afterwards.
    fn register_typed_variable(&mut self, name: &str, value: ValueRecord) {
        if self.step_open {
            self.write_variable(name, value);
        } else {
            self.deferred_vars.push((name.to_string(), value));
        }
    }

    /// Hand a variable to the writer, preserving the value's typed
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
    fn write_variable(&mut self, name: &str, value: ValueRecord) {
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
                column: None,
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
                access: None,
                script: false,
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
                access: None,
                script: false,
            }
        );
    }

    #[test]
    fn test_parse_ndjson_call_access() {
        // The optional `access` discriminator carries the source-
        // declared Cadence visibility modifier (`access(self)` etc.)
        // through to the recorder, where it surfaces as a tagged
        // `CadenceAccess:<function>:<Tag>` io_event so the strict
        // pin in `tests/test_tracer.rs` can `assert_eq!` against it.
        let input = r#"{"type":"call","name":"reveal","access":"self"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(
            events[0],
            TraceEvent::Call {
                name: "reveal".to_string(),
                args: Vec::new(),
                access: Some("self".to_string()),
                script: false,
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
                error_kind: None,
            }
        );
    }

    #[test]
    fn test_parse_ndjson_error_kind() {
        // The optional `error_kind` discriminator distinguishes Cadence
        // pre-condition / post-condition violations from a user-issued
        // panic; see `convert_events`'s `EventLogKind` dispatch.
        let input =
            r#"{"type":"error","message":"pre-condition failed: b != 0","error_kind":"pre"}"#;
        let events = parse_ndjson(input).unwrap();
        assert_eq!(
            events[0],
            TraceEvent::Error {
                message: "pre-condition failed: b != 0".to_string(),
                error_kind: Some("pre".to_string()),
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
                access: None,
                script: false,
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
                column: None,
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
                column: None,
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
                column: None,
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
