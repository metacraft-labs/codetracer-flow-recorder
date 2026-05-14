/// Cadence `event` declarations + `emit` statements.
///
/// This fixture defines two events with mixed parameter shapes and
/// emits each from a dedicated function:
///
///   * `Transfer(amount: UFix64, from: Address, to: Address)`
///     — 3 parameters, mixed scalar types, a frequent FlowToken /
///     FUSD shape.
///   * `NFTMinted(id: UInt64, metadata: {String: String})`
///     — scalar id + dictionary payload, the canonical NFT event
///     shape.
///
/// The recorder must surface each `emit` as a tagged
/// `CadenceEmit:` io_event whose text payload preserves
/// (event-name, [(field, value), ...]).  The event payload is
/// additionally captured field-by-field as typed
/// `ValueRecord` variants under `emit:<EventName>.<field>` locals
/// so the frontend can render the event with full type fidelity.

access(all) event Transfer(amount: UFix64, from: Address, to: Address)

access(all) event NFTMinted(id: UInt64, metadata: {String: String})

access(all) fun do_transfer() {
    emit Transfer(amount: 12.5, from: 0x01, to: 0x02)
}

access(all) fun do_mint() {
    emit NFTMinted(
        id: 1001,
        metadata: {"name": "Cat", "rarity": "common"}
    )
}

access(all) fun compute(): Int {
    do_transfer()
    do_mint()
    return 0
}

access(all) fun main(): Int {
    return compute()
}
