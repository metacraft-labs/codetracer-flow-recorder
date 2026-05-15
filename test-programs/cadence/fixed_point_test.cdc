/// Cadence `Fix64` / `UFix64` fixed-point arithmetic.
///
/// Cadence's fixed-point types carry 8 decimal places of precision
/// (`UFix64.max == 184467440737.09551615`) and are the canonical
/// types for FlowToken / FUSD balances.  Each value is internally
/// stored as a 64-bit signed integer multiplied by 10^8 (the
/// canonical scaled-integer form); the recorder surfaces every
/// fixed-point local as `ValueRecord::Int` with the scaling factor
/// captured implicitly via the type-id metadata (`Fix64` / `UFix64`
/// lang_type names).
///
/// This fixture exercises the four canonical operations:
///
///   * Negative fixed-point literal:  `-1.5 : Fix64` → `-150_000_000`
///   * Unsigned fractional literal:   `0.25 : UFix64` → `25_000_000`
///   * Cross-sign multiplication:     `-1.5 * 0.25` → `-37_500_000`
///   * Same-sign addition:            `0.25 + 1.0` → `125_000_000`
///
/// Closes the M9 known-limitation that `Fix64` / `UFix64` fell
/// through to `ValueRecord::Raw` — see the `emit:Transfer.amount`
/// strict pin in `test_events_emit_test_via_ct_print_full`.

access(all) fun compute(): Int {
    let f: Fix64 = -1.5
    let u: UFix64 = 0.25
    let prod: Fix64 = f * Fix64(u)
    let sum: UFix64 = u + 1.0
    return 0
}

access(all) fun main(): Int {
    return compute()
}
