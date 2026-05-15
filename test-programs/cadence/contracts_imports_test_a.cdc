/// Imported contract A for `contracts_imports_test.cdc`.
///
/// Declared at address `0x01`; the canonical on-chain qualified
/// name is `A.0x01.ContractA.foo`.

access(all) contract ContractA {
    access(all) fun foo(): Int {
        return 11
    }
}
