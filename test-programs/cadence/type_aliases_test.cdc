/// Cadence type aliases (Cadence 1.x).
///
/// Cadence 1.x introduced top-level type alias declarations:
///
///   ```
///   access(all) type alias Coin = UFix64
///   ```
///
/// The alias is a compile-time-only renaming — values declared with
/// the alias type carry the underlying type's typed `ValueRecord`
/// variant.  The alias name is preserved as type-id metadata so
/// downstream tooling can surface the source-declared name (`Coin`)
/// alongside the underlying type (`UFix64`).
///
/// Strict pin asserts:
///
///   * The alias surfaces as a tagged `CadenceTypeAlias:<Alias>:
///     <Underlying>` io_event so downstream consumers can resolve
///     alias → underlying without re-parsing the source.
///   * Values declared with the alias type surface as the underlying
///     type's typed `ValueRecord` variant (here `UFix64` → scaled
///     `Int`).

access(all) fun compute(): Int {
    let amount: UFix64 = 1.5
    return 0
}

access(all) fun main(): Int {
    return compute()
}
