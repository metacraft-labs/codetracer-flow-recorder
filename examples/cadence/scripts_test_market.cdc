/// Imported contract for `scripts_test.cdc`.
///
/// Publishes a `floor_price()` accessor returning a `UInt64` —
/// imported via `import MarketContract from "scripts_test_market.cdc"`
/// from the script.  The qualified name `MarketContract.floor_price`
/// must surface in the trace's function table without truncation.

access(all) contract MarketContract {
    access(all) fun floor_price(): UInt64 {
        return 12345
    }
}
