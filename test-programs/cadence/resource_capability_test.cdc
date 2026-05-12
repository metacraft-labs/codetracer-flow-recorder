/// Cadence-specific: resources + capabilities.
///
/// Demonstrates the resource lifecycle (create / move / destroy) which
/// the recorder surfaces via `ResourceCreate`/`ResourceMove`/`ResourceDestroy`
/// trace events plus `register_special_event(EventLogKind::TraceLogEvent, ...)`.
/// Capabilities are issued and published; the recorder currently logs
/// these as plain `event` entries (see the `RECORDER BUG` notes).

access(all) resource Coin {
    access(all) let value: Int

    init(value: Int) {
        self.value = value
    }
}

access(all) resource Vault {
    access(all) var balance: Int

    init() {
        self.balance = 0
    }

    access(all) fun deposit(coin: @Coin) {
        self.balance = self.balance + coin.value
        destroy coin
    }
}

access(all) fun compute(): Int {
    // Create a coin, move it into a vault, then destroy the vault.
    let coin: @Coin <- create Coin(value: 42)
    let vault: @Vault <- create Vault()
    vault.deposit(coin: <- coin)
    let final_balance: Int = vault.balance
    destroy vault
    return final_balance
}

access(all) fun main(): Int {
    return compute()
}
