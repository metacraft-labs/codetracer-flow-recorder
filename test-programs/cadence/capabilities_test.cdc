/// Cadence capability lifecycle (issue / publish / borrow / unpublish).
///
/// Capabilities are Cadence's defining security primitive: a typed
/// reference to a stored value, mintable by the storage owner and
/// publishable through `account.capabilities`.  This fixture
/// exercises the modern Cadence 1.x capability API end-to-end:
///
///   * `account.capabilities.storage.issue<&Vault>(/storage/Vault)`
///     mints a typed capability backed by `/storage/Vault`.
///   * `account.capabilities.publish(cap, at: /public/Vault)`
///     publishes the capability under a public path.
///   * `recipient.capabilities.borrow<&Vault>(/public/Vault)`
///     resolves the capability and borrows a reference.
///   * `account.capabilities.unpublish(/public/Vault)` revokes.
///
/// The recorder must surface each capability as a typed
/// `ValueRecord::Reference { dereferenced, address, mutable: false,
/// type_id }`, with the storage-path string captured in `address` (as
/// the canonical hash of `/storage/Vault` or similar id), and the
/// publish / unpublish transitions surface as paired io_events with
/// distinct text payloads (`CapabilityPublish:/public/Vault` and
/// `CapabilityUnpublish:/public/Vault`).

access(all) resource Vault {
    access(all) var balance: Int

    init() {
        self.balance = 0
    }
}

access(all) fun compute(): Int {
    // 1. Mint a capability via account.capabilities.storage.issue<&Vault>.
    let cap = self.account
        .capabilities.storage
        .issue<&Vault>(/storage/Vault)

    // 2. Publish at /public/Vault so other accounts can borrow it.
    self.account.capabilities.publish(cap, at: /public/Vault)

    // 3. Borrow the capability back as a typed reference.
    let ref: &Vault = self.account
        .capabilities.borrow<&Vault>(/public/Vault)
        ?? panic("borrow failed")

    let bal: Int = ref.balance

    // 4. Revoke by unpublishing.
    self.account.capabilities.unpublish(/public/Vault)

    return bal
}

access(all) fun main(): Int {
    return compute()
}
