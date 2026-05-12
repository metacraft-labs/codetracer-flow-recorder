/// Error paths: explicit `panic`, pre-condition violation, post-condition
/// violation.  The driving NDJSON for this fixture exercises only the
/// safe path through `safe_compute` — the panic + pre/post condition
/// failures are documented as `RECORDER BUG`s on `#[ignore]`d sibling
/// tests until the recorder grows real Error-event support beyond
/// the existing `EventLogKind::Error` channel.

access(all) fun divide_strict(a: Int, b: Int): Int {
    pre {
        b != 0: "denominator must be non-zero"
    }
    post {
        result * b == a: "division must be exact"
    }
    return a / b
}

access(all) fun panicking_call(): Int {
    panic("intentional panic for trace coverage")
    return 0
}

access(all) fun safe_compute(): Int {
    let a: Int = 10
    let b: Int = 5
    let q: Int = divide_strict(a: a, b: b)
    return q + 1
}

access(all) fun compute(): Int {
    return safe_compute()
}

access(all) fun main(): Int {
    return compute()
}
