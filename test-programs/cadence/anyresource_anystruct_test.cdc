/// Cadence dynamic supertypes: `AnyResource`, `AnyStruct`,
/// `AnyAuthAccount`, `AnyPublicAccount`.
///
/// Cadence permits passing concrete resources / structs through a
/// function parameter typed as the dynamic supertype:
///
///   * `fun store(r: @AnyResource)` accepts any `@T` resource.
///   * `fun process(s: AnyStruct)` accepts any value-typed value.
///   * `fun inspect_auth(a: &AnyAuthAccount)` /
///     `fun inspect_public(p: &AnyPublicAccount)` accept any account
///     reference.
///
/// The runtime preserves the concrete type information across the
/// dynamic dispatch.  The recorder must surface BOTH:
///
///   * The concrete runtime type's typed `ValueRecord` variant on
///     the parameter local (NOT a `Raw` fallback).
///   * Both the static (declared-parameter) type and the concrete
///     runtime type id via a tagged `CadenceAnyType:<static>:
///     <runtime>:<varname>` io_event so downstream tooling can
///     correlate the dynamic dispatch with the call frame.
///
/// This fixture exercises the resource and struct dynamic supertypes;
/// the account-reference flavours follow the same shape and reuse
/// the same recorder dispatch.

access(all) resource Vault {
    access(all) var balance: Int
    init(balance: Int) {
        self.balance = balance
    }
}

access(all) struct Token {
    access(all) let id: Int
    init(id: Int) {
        self.id = id
    }
}

access(all) fun store(r: @AnyResource) {
    destroy r
}

access(all) fun process(s: AnyStruct): Int {
    return 0
}

access(all) fun compute(): Int {
    let v: @Vault <- create Vault(balance: 50)
    store(<-v)
    let t: Token = Token(id: 9)
    let _: Int = process(s: t)
    return 0
}

access(all) fun main(): Int {
    return compute()
}
