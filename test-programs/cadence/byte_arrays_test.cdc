/// Cadence `[UInt8]` byte arrays.
///
/// Cadence treats byte arrays as a first-class typed sequence of
/// `UInt8` elements (NOT a coalesced byte-string).  Real-world usage
/// includes:
///
///   * Hash digest payloads (`[UInt8]` outputs of `HashAlgorithm.*.hash`).
///   * Public-key bytes (`PublicKey.publicKey`).
///   * Raw transaction signature bytes (`Crypto.verify` arguments).
///
/// The recorder must surface `[UInt8]` as `ValueRecord::Sequence` with
/// each element as an independent typed `ValueRecord::Int` so byte-
/// level inspection is possible in the object inspector.  This fixture
/// pins the canonical hex-form constants `[0xDE, 0xAD, 0xBE, 0xEF]`
/// (= `[222, 173, 190, 239]` in decimal) and asserts:
///
///   * The `[UInt8]` array surfaces as `ValueRecord::Sequence<Int>`
///     with all four bytes captured verbatim.
///   * Element access (`bytes[0]`) returns a typed `ValueRecord::Int`
///     matching the first element.
///   * `.length` returns a typed `ValueRecord::Int` of 4.

access(all) fun compute(): Int {
    let bytes: [UInt8] = [0xDE, 0xAD, 0xBE, 0xEF]
    let first: UInt8 = bytes[0]
    let n: Int = bytes.length
    return n
}

access(all) fun main(): Int {
    return compute()
}
