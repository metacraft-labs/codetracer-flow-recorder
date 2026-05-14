/// Resources nested in arrays / dictionaries / optionals.
///
/// Cadence's collection types parameterised over a resource-typed
/// element (`@[Vault]`, `@{Address: Vault}`, `@Vault?`) keep the
/// per-element resource semantics intact: every element is an owned
/// resource with its own `uuid` and `owner`, and `append` / `remove` /
/// `swap` operations transfer ownership accordingly.
///
/// The recorder must surface:
///
///   * `@[Vault]` as `ValueRecord::Sequence` whose elements are
///     typed `ValueRecord::Struct` snapshots (`{ResourceType,
///     ResourceUuid, ResourceOwner}`) — not stringified `Raw`.
///   * `@{Address: Vault}` as a `Sequence` of `Tuple` (key/value)
///     entries: each tuple holds an `Address`-typed key and a typed
///     `Struct` resource value.
///   * `@Vault?` as `ValueRecord::Variant { Some(Struct ...) }` when
///     populated and `ValueRecord::Variant { None }` after the
///     resource moves out.

access(all) resource Vault {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }
}

access(all) fun compute(): Int {
    // 1. Resource array `@[Vault]` — append two vaults, then remove
    //    the head (transfers ownership of Vault#9001 to `head`).
    let arr: @[Vault] <- []
    arr.append(<-create Vault(balance: 10))
    arr.append(<-create Vault(balance: 20))
    let head: @Vault <- arr.remove(at: 0)

    // 2. Resource dictionary `@{Address: Vault}` keyed by address.
    //    `swap` exchanges Vault#9003 (alice) and Vault#9004 (bob).
    let dict: @{Address: Vault} <- {}
    dict[0x01] <-! create Vault(balance: 30)
    dict[0x02] <-! create Vault(balance: 40)
    let removed: @Vault <- dict.remove(key: 0x01)!

    // 3. Optional resource `@Vault?` — populated then drained.
    var opt: @Vault? <- create Vault(balance: 50)
    let drained: @Vault <- opt <- nil

    destroy head
    destroy removed
    destroy drained
    destroy arr
    destroy dict
    destroy opt
    return 0
}

access(all) fun main(): Int {
    return compute()
}
