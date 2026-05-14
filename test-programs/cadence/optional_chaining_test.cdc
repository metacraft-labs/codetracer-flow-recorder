/// Cadence optional chaining and force-unwrap.
///
/// Cadence's `?.` chained-optional access lazily short-circuits to
/// `nil` if any step in the chain is `nil`, while `!` forcibly
/// unwraps an optional and panics with a runtime tag if the value is
/// `nil`.  This fixture exercises both:
///
///   1. A populated chain `box.inner?.balance` resolves to
///      `Some(Some(Int))` — surfacing through the recorder as a
///      nested `ValueRecord::Variant { Some(Variant { Some(Int) }) }`.
///   2. A short-circuit chain `empty.inner?.balance` returns `None`
///      with no further calls into `Vault.balance`.
///   3. A successful force-unwrap `let bal = present!` returns the
///      typed `Int` payload directly.
///   4. A failing force-unwrap `let bad = absent!` surfaces as a
///      tagged `ForceNilUnwrap` io_event distinguishable from a
///      generic `panic`.

access(all) resource Vault {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }
}

access(all) resource Box {
    access(all) var inner: @Vault?

    init(inner: @Vault?) {
        self.inner <- inner
    }
}

access(all) fun compute(): Int {
    // 1. Populated chain: box.inner?.balance evaluates through.
    let box: @Box <- create Box(inner: <-create Vault(balance: 42))
    let chained: Int? = box.inner?.balance
    destroy box

    // 2. Short-circuit chain: empty.inner?.balance == nil.
    let empty: @Box <- create Box(inner: nil)
    let short: Int? = empty.inner?.balance
    destroy empty

    // 3. Successful force-unwrap.
    let present: Int? = 7
    let bal: Int = present!

    // 4. Failing force-unwrap (caught by helper, surfaces as tag).
    let absent: Int? = nil
    let _bad: Int = absent!

    return bal
}

access(all) fun main(): Int {
    return compute()
}
