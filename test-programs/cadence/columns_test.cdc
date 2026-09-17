// Column-aware navigation fixture.
//
// Line 9 deliberately carries two statements. A line-only trace cannot tell
// the two steps on it apart; a column-aware one can, and that is the whole
// property this fixture exists to pin.

access(all) fun compute(): Int {
    var a = 1
    a = a + 10; var b = a * 2
    return b
}

access(all) fun main(): Int {
    return compute()
}
