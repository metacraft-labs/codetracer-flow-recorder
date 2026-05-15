/// Cadence import-resolution across multiple contract files.
///
/// Cadence on-chain identifiers carry the address an contract was
/// deployed to in the canonical fully-qualified form
/// `A.<address>.<ContractName>.<member>` (e.g. `A.0x01.ContractA.foo`).
/// This fixture wires three contracts at three distinct addresses:
///
///   * `0x01: ContractA.foo()` — declared in
///     `contracts_imports_test_a.cdc`.
///   * `0x02: ContractB.bar()` — declared in
///     `contracts_imports_test_b.cdc`.
///   * The aggregator entry point declared in this file imports both
///     and calls each.
///
/// The recorder must:
///
///   * Carry the fully-qualified `A.<address>.<Contract>.<member>`
///     names verbatim in the function table — no truncation, no
///     re-rendering.
///   * Surface each imported contract's source file as a distinct
///     entry in the trace's per-step source-mapping metadata
///     (so the frontend can navigate into the imported sources).

import ContractA from 0x01
import ContractB from 0x02

access(all) fun main(): Int {
    let x: Int = ContractA.foo()
    let y: Int = ContractB.bar()
    return x + y
}
