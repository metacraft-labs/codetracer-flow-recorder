/// Full Cadence move-operator family.
///
/// This fixture exercises every variant of the resource move
/// operator end-to-end so the recorder can be pinned on the typed
/// `ValueRecord::Struct` snapshot (carrying the resource's implicit
/// `uuid` and `owner` fields) and on the tagged
/// `ResourceOwnerChange` io_event emitted for every owner
/// transition.  The five operator forms exercised:
///
///   1. Plain move-assignment   `let b <- a`
///   2. Swap                    `x <-> y`
///   3. Shift                   `let old <- x <- new`
///   4. Force-unwrap move       `<-! optionalRef`
///   5. Explicit nested destroy `destroy <-something.inner`
///
/// The recorder must surface each move's pre/post snapshot as a
/// `ValueRecord::Struct { ResourceType, ResourceUuid, ResourceOwner }`
/// (the `ResourceOwner` field is the M10 extension that lets the
/// frontend track owner transitions without re-parsing the printed
/// form), AND emit one `ResourceOwnerChange:<Type>#<uuid>: <from> ->
/// <to>` io_event per ownership transfer (TraceLogEvent →
/// ioStderr).

access(all) resource Vault {
    access(all) var balance: Int

    init(balance: Int) {
        self.balance = balance
    }
}

access(all) resource Box {
    access(all) var inner: @Vault?

    init(inner: @Vault) {
        self.inner <- inner
    }
}

access(all) fun compute(): Int {
    // 1. Plain move-assignment: `let b <- a`.
    //    Vault#5001 transitions from owner=alice → owner=bob.
    let a: @Vault <- create Vault(balance: 100)
    let b: @Vault <- a

    // 2. Swap: `x <-> y`.
    //    Vault#5002 (owner=alice) ↔ Vault#5003 (owner=bob).
    var x: @Vault <- create Vault(balance: 10)
    var y: @Vault <- create Vault(balance: 20)
    x <-> y

    // 3. Shift: `let old <- x <- new`.
    //    Vault#5004 takes Vault#5002's slot in `x`; Vault#5002
    //    moves to `old` (transition alice → bob).
    let new_v: @Vault <- create Vault(balance: 30)
    let old: @Vault <- x <- new_v

    // 4. Force-unwrap move: `<-!` over an optional resource.
    //    Box#5005 holds Vault#5006; `<-! box.inner` force-unwraps.
    let box: @Box <- create Box(inner: <-create Vault(balance: 40))
    let unwrapped: @Vault <- box.inner <-! nil
    destroy box

    // 5. Explicit destroy of a nested resource.
    //    Vault#5006 destroyed (owner=bob).
    destroy unwrapped
    destroy b
    destroy old
    destroy x
    destroy y
    return 0
}

access(all) fun main(): Int {
    return compute()
}
