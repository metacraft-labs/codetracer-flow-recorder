/// Cadence path values across the three path domains.
///
/// Cadence has three path domains:
///
///   * `/storage/<id>` — backing storage on the account.
///   * `/public/<id>` — capability publication slot.
///   * `/private/<id>` — pre-Cadence-1.x private capability slot
///     (still a valid path domain, see the Cadence-1.x migration
///     notes).
///
/// Each path is a first-class value with a `domain` and an
/// `identifier`, surfaced in the trace as
/// `ValueRecord::Struct { String "domain", String "identifier" }`.
/// Already supported by the recorder via the `Path` /
/// `StoragePath` / `PublicPath` / `PrivatePath` cadence_type
/// dispatch (see `src/tracer.rs` `value_record` Path branch); this
/// fixture closes the M10 `path_types_test` deliverable by pinning
/// the canonical shape end-to-end.

access(all) fun compute(): Int {
    let p1: StoragePath = /storage/Vault
    let p2: PublicPath = /public/Vault
    let p3: PrivatePath = /private/Vault
    return 0
}

access(all) fun main(): Int {
    return compute()
}
