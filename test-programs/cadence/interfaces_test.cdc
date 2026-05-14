/// Cadence resource interface + interface-restricted reference.
///
/// This fixture defines a `resource interface Provider` with a
/// single `provide(): @Vault` method, a concrete implementer
/// `MyVault: Provider`, and a function that calls
/// `myVault.provide()` via the interface restriction set
/// (`&{Provider}`).  The recorder must surface:
///
///   * The interface-restricted reference as
///     `ValueRecord::Reference` with the restriction set captured
///     in the type-id metadata (`&{Provider}` in the cadence_type).
///   * BOTH the concrete `MyVault.provide` call frame AND the
///     interface-typed `Provider.provide` dispatch frame visible in
///     the function table — so the frontend can render the
///     interface dispatch in the call trace independently of the
///     concrete implementation.

access(all) resource Vault {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }
}

access(all) resource interface Provider {
    access(all) fun provide(): @Vault
}

access(all) resource MyVault: Provider {
    access(all) fun provide(): @Vault {
        return <-create Vault(balance: 77)
    }
}

access(all) fun consume(p: &{Provider}): Int {
    let v: @Vault <- p.provide()
    let bal: Int = v.balance
    destroy v
    return bal
}

access(all) fun compute(): Int {
    let mv: @MyVault <- create MyVault()
    let restricted: &{Provider} = &mv as &{Provider}
    let result: Int = consume(p: restricted)
    destroy mv
    return result
}

access(all) fun main(): Int {
    return compute()
}
