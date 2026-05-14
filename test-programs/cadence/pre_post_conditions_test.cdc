/// Cadence `pre { ... }` / `post { ... }` function clauses.
///
/// This fixture exercises a function defining both a `pre` block
/// (input precondition) and a `post` block (postcondition over the
/// `result` and `before(...)` snapshot).  Three calls are issued:
///
///   1. Passing pre + passing post  → silent (no io_event).
///   2. Failing pre                 → tagged `CadencePreCondition`
///                                     io_event (TraceLogEvent →
///                                     ioStderr).
///   3. Failing post                → tagged `CadencePostCondition`
///                                     io_event.
///
/// The pre/post tag distinction is the load-bearing part: the
/// recorder routes both pre and post failures through
/// `EventLogKind::TraceLogEvent` (vs. user `panic` which keeps
/// `EventLogKind::Error`), and the `metadata` carries the tag —
/// see `CadenceTracer::convert_events`'s `Error` arm and the
/// existing `error_paths_test` strict pin.

access(all) resource Account {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }

    /// Add `amount` to the balance, with both a `pre` and a `post`
    /// clause.
    access(all) fun deposit(amount: Int): Int {
        pre {
            amount > 0: "amount must be positive"
        }
        post {
            self.balance > before(self.balance): "balance must increase"
        }
        self.balance = self.balance + amount
        return self.balance
    }
}

access(all) fun compute(): Int {
    let acc <- create Account(balance: 100)

    // Call 1: passing pre + passing post.  Silent: no io event.
    let after1: Int = acc.deposit(amount: 25)

    // Call 2: failing pre — `amount = 0` violates `amount > 0`.
    let _ = acc.deposit(amount: 0)

    // Call 3: failing post — passing pre, but the body cheats by
    // restoring the original balance, so the post clause `balance >
    // before(balance)` violates.
    let _ = acc.deposit(amount: -1)

    destroy acc
    return after1
}

access(all) fun main(): Int {
    return compute()
}
