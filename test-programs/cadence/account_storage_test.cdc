/// Cadence account.storage operations across all three path domains.
///
/// `/storage`, `/public`, `/private` paths underlie every Flow
/// contract.  This fixture exercises the canonical
/// `account.storage.{save, borrow, load, copy, type}` surface against
/// a `/storage/Vault` resource, with a parallel struct round-tripped
/// through `/storage/Config`.
///
/// The recorder must surface each `Path` argument as a typed
/// `ValueRecord::Struct { [String "domain", String "identifier"] }`
/// (the canonical Cadence `Path` shape), so the frontend can resolve
/// `/storage/Vault` to `domain="storage", identifier="Vault"` without
/// re-parsing the printed form.

access(all) resource Vault {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }
}

access(all) struct Config {
    access(all) let key: String
    access(all) let value: Int

    init(key: String, value: Int) {
        self.key = key
        self.value = value
    }
}

access(all) fun compute(): Int {
    // 1. Save a resource into /storage.
    let v: @Vault <- create Vault(balance: 100)
    self.account.storage.save(<- v, to: /storage/Vault)

    // 2. Borrow a reference at the same path.
    let ref: &Vault = self.account.storage.borrow<&Vault>(
        from: /storage/Vault
    ) ?? panic("borrow failed")
    let borrowed_balance: Int = ref.balance

    // 3. Save a struct (copy semantics) under /storage/Config.
    self.account.storage.save(
        Config(key: "max", value: 42),
        to: /storage/Config,
    )

    // 4. Copy the struct back out (struct = copy semantics).
    let cfg: Config = self.account.storage.copy<Config>(
        from: /storage/Config
    ) ?? panic("copy failed")

    // 5. Load the resource out (this consumes /storage/Vault).
    let v2: @Vault <- self.account.storage.load<@Vault>(
        from: /storage/Vault
    ) ?? panic("load failed")
    let loaded_balance: Int = v2.balance
    destroy v2

    return borrowed_balance + cfg.value + loaded_balance
}

access(all) fun main(): Int {
    return compute()
}
