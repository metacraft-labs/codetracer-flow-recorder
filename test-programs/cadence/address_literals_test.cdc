/// Cadence Address literals — `0x1`, `0xf8d6e0586b0a20c7`, `==` cmp.
///
/// Cadence `Address` values are 8-byte unsigned integers shown in
/// hex form.  Small addresses (e.g. `0x01`, `0x02`) fit in `i64` and
/// must surface as `ValueRecord::Int` (matching the M2 hex-form
/// convention).  Full 8-byte addresses that exceed `i64::MAX`
/// (e.g. real testnet addresses like `0xf8d6e0586b0a20c7`) must
/// surface as `ValueRecord::BigInt` with the 8 raw bytes preserved.
///
/// Closes the M9 known limitation: `Address` previously fell back
/// to `ValueRecord::Raw`.

access(all) fun compute(): Bool {
    // Small address: fits in i64, must surface as ValueRecord::Int.
    let a: Address = 0x01

    // Same address as a different literal form.
    let a_alt: Address = 0x01

    // Real Flow testnet address: > i64::MAX, must surface as BigInt.
    let b: Address = 0xf8d6e0586b0a20c7

    // Equality on Address values.
    let eq_small: Bool = a == a_alt
    let eq_mixed: Bool = a == b

    return eq_small && !eq_mixed
}

access(all) fun main(): Bool {
    return compute()
}
