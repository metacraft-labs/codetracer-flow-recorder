/// Cadence access-control modifiers.
///
/// Cadence's four visibility levels — `access(self)`,
/// `access(contract)`, `access(account)`, `access(all)` — gate which
/// callers may invoke a function or read a field.  The recorder must
/// surface each call frame's source-declared visibility so the
/// frontend can render access-violation paths and highlight the
/// security boundary at each call site.
///
/// This fixture defines a contract `Vault` with one field at each of
/// the four access levels and a struct `Reader` exposing one accessor
/// per level.  `compute` calls each accessor in declaration order.
///
/// The recorder must emit one tagged `CadenceAccess:<function>:<Tag>`
/// io_event per call, where `<Tag>` is one of `AccessSelf`,
/// `AccessContract`, `AccessAccount`, `AccessAll`.

access(all) contract Vault {
    access(self)     let secret: Int
    access(contract) let internal_balance: Int
    access(account)  let account_balance: Int
    access(all)      let public_balance: Int

    init() {
        self.secret = 1
        self.internal_balance = 10
        self.account_balance = 100
        self.public_balance = 1000
    }

    // Mirror the four access levels on accessor functions so the
    // recorder can pin each call frame's visibility tag.
    access(self)     fun reveal_self():     Int { return self.secret }
    access(contract) fun reveal_contract(): Int { return self.internal_balance }
    access(account)  fun reveal_account():  Int { return self.account_balance }
    access(all)      fun reveal_all():      Int { return self.public_balance }
}

access(all) fun compute(): Int {
    let s: Int = Vault.reveal_self()
    let c: Int = Vault.reveal_contract()
    let a: Int = Vault.reveal_account()
    let p: Int = Vault.reveal_all()
    return s + c + a + p
}

access(all) fun main(): Int {
    return compute()
}
