/// Cadence composite declaration kinds: `struct`, `resource`,
/// `event`.
///
/// Cadence has three composite-declaration kinds that the runtime
/// surfaces as distinct `interpreter.CompositeKind` discriminators:
///
///   * `struct Coord { ... }` — value-typed copy semantics.
///   * `resource Vault { ... }` — linear-typed `<-` move semantics.
///   * `event Spawned(...)` — emit-only, no-instance composite.
///
/// This fixture declares one of each kind in the same contract and
/// exercises construction (and destruction, where applicable) so the
/// recorder surfaces the composite-kind tag for each declaration on
/// the io-event channel.  The `event` declaration additionally
/// surfaces as a tagged `CadenceEmit:` io_event when emitted (the
/// existing `emit Spawned(...)` path).

access(all) contract C {
    access(all) struct Coord {
        access(all) let x: Int
        access(all) let y: Int

        init(x: Int, y: Int) {
            self.x = x
            self.y = y
        }
    }

    access(all) resource Vault {
        access(all) var balance: Int

        init(balance: Int) {
            self.balance = balance
        }
    }

    access(all) event Spawned(id: Int)

    access(all) fun create_vault(balance: Int): @Vault {
        return <-create Vault(balance: balance)
    }
}

access(all) fun compute(): Int {
    let p: C.Coord = C.Coord(x: 3, y: 4)
    let v: @C.Vault <- C.create_vault(balance: 100)
    emit C.Spawned(id: 1)
    destroy v
    return p.x + p.y
}

access(all) fun main(): Int {
    return compute()
}
