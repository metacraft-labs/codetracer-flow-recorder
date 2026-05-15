/// Imported contract B for `contracts_imports_test.cdc`.
///
/// Declared at address `0x02`; the canonical on-chain qualified
/// name is `A.0x02.ContractB.bar`.

access(all) contract ContractB {
    access(all) fun bar(): Int {
        return 22
    }
}
