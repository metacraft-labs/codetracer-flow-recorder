/// Cadence integer-width matrix: Int8 / Int16 / Int32 / Int64 /
/// Int128 / UInt256 (and the Word* family).
///
/// Cadence supports signed and unsigned integers up to 256 bits.
/// Widths up to 64 bits round-trip through `i64`; widths beyond 64
/// bits must surface as `ValueRecord::BigInt` with the exact bytes
/// preserved.  This fixture pins the boundary by exercising at least
/// one value of each width, with the wide-integer values chosen to
/// exceed the next-smaller width's signed range so the recorder
/// cannot quietly truncate them.
///
/// Closes the M9 known limitation: `Int128` / `UInt128` / `UInt256`
/// previously fell back to `ValueRecord::Raw`.

access(all) fun compute(): Int {
    // i64-fitting widths surface as ValueRecord::Int.
    let i8_val:  Int8   = 100
    let i16_val: Int16  = 30000
    let i32_val: Int32  = 2000000000
    let i64_val: Int64  = 9223372036854775000
    let u32_val: UInt32 = 4000000000
    let u64_val: UInt64 = 9000000000000000000

    // Widths > 64 bits must surface as ValueRecord::BigInt.
    // Value = 2^126 = 85070591730234615865843651857942052864.
    let big_i128: Int128 = 85070591730234615865843651857942052864

    // Value = 2^128 - 1 (max UInt128).
    let big_u128: UInt128 = 340282366920938463463374607431768211455

    // Value = 2^200 (well beyond UInt128's range).
    let big_u256: UInt256 = 1606938044258990275541962092341162602522202993782792835301376

    return 1
}

access(all) fun main(): Int {
    return compute()
}
