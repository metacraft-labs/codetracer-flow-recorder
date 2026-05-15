/// Cadence 1.x attachment syntax.
///
/// Cadence 1.x introduced attachments — first-class typed values
/// permanently attached to a resource (or any base composite):
///
///   ```
///   access(all) attachment Logger for AnyResource {
///       access(all) let label: String
///       init(label: String) {
///           self.label = label
///       }
///   }
///
///   let v: @Vault <- create Vault(balance: 100)
///   let attached <- attach Logger(label: "primary") to <-v
///   let l: &Logger? = attached[Logger]
///   destroy attached
///   ```
///
/// The recorder must surface:
///
///   * The attach site as a tagged `CadenceAttachmentAttach:<Att>:
///     <Target>` io_event so downstream tooling can correlate the
///     attachment-base linkage.
///   * The attachment value itself as a typed `ValueRecord::Struct`
///     with attachment-target metadata in the type-id (the type-id
///     surfaces as `<Att>@<Target>`).
///   * The remove operator as a tagged `CadenceAttachmentRemove:
///     <Att>:<Target>` io_event.
///
/// Strict pin asserts the exact (attach, remove) tag pair plus the
/// typed ValueRecord shape of the attachment local.

access(all) resource Vault {
    access(all) var balance: Int
    init(balance: Int) {
        self.balance = balance
    }
}

access(all) attachment Logger for AnyResource {
    access(all) let label: String
    init(label: String) {
        self.label = label
    }
}

access(all) fun compute(): Int {
    let v: @Vault <- create Vault(balance: 100)
    let attached <- attach Logger(label: "primary") to <-v
    destroy attached
    return 0
}

access(all) fun main(): Int {
    return compute()
}
