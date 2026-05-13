/// Cadence reference forms: plain `&T`, restricted `&{Provider}`,
/// and authorized `auth(...) &T` (Cadence 1.x entitlement form).
///
/// References are non-resource pointers into stored values.  This
/// fixture exercises the three reference flavours back-to-back so the
/// recorder can be pinned on each one's typed representation:
///
///   * `&Vault`                         — plain reference
///   * `&{Provider}`                    — interface-restricted reference
///   * `auth(Withdraw) &Vault`          — entitlement-authorized reference
///
/// All three must surface as `ValueRecord::Reference { dereferenced,
/// address, mutable, type_id }`.  The plain reference is `mutable: false`,
/// the entitled reference is `mutable: true` (entitlement-authorized
/// references can mutate the underlying resource).  The restriction set
/// on `&{Provider}` and the authorization set on `auth(Withdraw)` are
/// captured in the type metadata via the recorded Cadence type-id.

access(all) resource interface Provider {
    access(all) fun get_balance(): Int
}

access(all) entitlement Withdraw

access(all) resource Vault: Provider {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }

    access(all) fun get_balance(): Int {
        return self.balance
    }

    access(Withdraw) fun withdraw(amount: Int): Int {
        self.balance = self.balance - amount
        return amount
    }
}

access(all) fun read_plain(r: &Vault): Int {
    return r.balance
}

access(all) fun read_restricted(r: &{Provider}): Int {
    return r.get_balance()
}

access(all) fun apply_withdraw(r: auth(Withdraw) &Vault, amount: Int): Int {
    return r.withdraw(amount: amount)
}

access(all) fun compute(): Int {
    let v: @Vault <- create Vault(balance: 100)

    let plain: &Vault = &v as &Vault
    let p: Int = read_plain(r: plain)

    let restricted: &{Provider} = &v as &{Provider}
    let r: Int = read_restricted(r: restricted)

    let entitled: auth(Withdraw) &Vault = &v as auth(Withdraw) &Vault
    let w: Int = apply_withdraw(r: entitled, amount: 25)

    destroy v
    return p + r + w
}

access(all) fun main(): Int {
    return compute()
}
