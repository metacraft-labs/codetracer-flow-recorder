/// Cadence script entry point.
///
/// Cadence has two top-level execution forms:
///
///   * Scripts: `access(all) fun main(): X { ... }` — read-only,
///     return a typed value.  Used by clients to query chain state.
///   * Transactions: `transaction(...) { prepare ... execute ... }`
///     — mutate chain state, no return.
///
/// This fixture exercises the script form: `main` reads from a
/// public capability published by the imported `MarketContract` and
/// returns a typed `UInt64` value (the market floor price).  The
/// recorder must:
///
///   * Recognise `main` as the script entry point — surfaced as a
///     tagged `CadenceScriptEntry:main` io_event so the strict pin
///     can pin the script-form boundary independently of a same-
///     named transaction `main`.
///   * Resolve the imported-contract qualified name across files —
///     `MarketContract.floor_price` shows up verbatim in the function
///     table.
///   * Carry the returned value with its concrete typed
///     `ValueRecord` variant (here: `ValueRecord::Int` from a
///     `UInt64`).
///
/// Convention: the imported contract is a sibling fixture file
/// `scripts_test_market.cdc` that ships alongside this script.

import MarketContract from "scripts_test_market.cdc"

access(all) fun main(): UInt64 {
    let floor: UInt64 = MarketContract.floor_price()
    return floor
}
