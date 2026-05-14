/// Cadence hash builtins (`HashAlgorithm.SHA2_256` / `SHA3_256`).
///
/// Cadence ships an in-runtime `HashAlgorithm` enum and a
/// `HashAlgorithm.hash(_: [UInt8])` method that returns the algorithm-
/// specific digest as a typed `[UInt8]`.  This fixture exercises the
/// two hash families called out as M10 deliverables — SHA-2/256 and
/// SHA-3/256 — over the canonical `"abc"` test vector.
///
/// The recorder must surface each `hash` call as a Call/Return pair
/// with:
///
///   * Input bytes typed as `ValueRecord::Sequence<UInt8>` (one
///     `ValueRecord::Int` element per byte).
///   * Output digest typed as `ValueRecord::Sequence<UInt8>` matching
///     the algorithm's 32-byte length and exact published vector.
///
/// Test-vector references:
///   * SHA-2/256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
///   * SHA-3/256("abc") = 3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532

access(all) fun compute(): UInt64 {
    let bytes: [UInt8] = [0x61, 0x62, 0x63]

    let d2: [UInt8] = HashAlgorithm.SHA2_256.hash(bytes)
    let d3: [UInt8] = HashAlgorithm.SHA3_256.hash(bytes)

    // Sum the first byte of each digest as a sanity check on the
    // arithmetic path; the hash payloads themselves are pinned via
    // the Call/Return shape, not via this scalar.
    return UInt64(d2[0]) + UInt64(d3[0])
}

access(all) fun main(): UInt64 {
    return compute()
}
