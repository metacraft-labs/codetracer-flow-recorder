/// Cadence `PublicKey.verify` signature verification.
///
/// Cadence ships `PublicKey` as a built-in struct carrying a raw
/// public-key byte array plus a `signatureAlgorithm` discriminator
/// (`SignatureAlgorithm.ECDSA_P256` or `ECDSA_secp256k1`).  The
/// `verify` method:
///
///   ```
///   pub fun verify(
///       signature: [UInt8],
///       signedData: [UInt8],
///       domainSeparationTag: String,
///       hashAlgorithm: HashAlgorithm,
///   ): Bool
///   ```
///
/// returns a typed `Bool` matching whether the signature over
/// `signedData` (with the given domain-separation tag and hash algo)
/// validates against this public key.  The strict pin asserts:
///
///   * The `verify` call surfaces as a Call/Return pair.
///   * The result surfaces as `ValueRecord::Bool` (NOT a `Raw`
///     stringified `"true"`).
///   * The four typed args surface verbatim.

access(all) fun compute(): Bool {
    let pk: PublicKey = PublicKey(
        publicKey: [1, 2, 3],
        signatureAlgorithm: SignatureAlgorithm.ECDSA_P256
    )
    let ok: Bool = pk.verify(
        signature: [4, 5, 6],
        signedData: [7, 8, 9],
        domainSeparationTag: "FLOW",
        hashAlgorithm: HashAlgorithm.SHA2_256
    )
    return ok
}

access(all) fun main(): Bool {
    return compute()
}
