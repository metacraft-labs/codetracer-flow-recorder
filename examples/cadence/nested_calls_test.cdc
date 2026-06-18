/// Four-deep nested call chain: main -> compute -> outer -> middle -> inner.
///
/// Each function takes scalar arguments, returns a scalar, and the
/// recorder must emit call_entry events in entry order and call_exit
/// events in LIFO order, with the correct return values surfacing on
/// each call_exit.

access(all) fun inner(a: Int, b: Int): Int {
    let c: Int = a + b
    return c
}

access(all) fun middle(x: Int): Int {
    let y: Int = inner(a: x, b: 10) + 0
    return y
}

access(all) fun outer(p: Int): Int {
    let q: Int = middle(x: p) + 100
    return q
}

access(all) fun compute(): Int {
    let result: Int = outer(p: 3)
    return result
}

access(all) fun main(): Int {
    return compute()
}
