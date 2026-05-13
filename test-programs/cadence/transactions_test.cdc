/// Cadence transaction { prepare; execute; post } phases.
///
/// Cadence transactions have three distinct phases — `prepare`,
/// `execute`, and `post` — that the frontend renders as separate
/// top-level call frames in the call trace.  This fixture exercises
/// the canonical lifecycle:
///
///   * `prepare(signer: auth(Storage, Capabilities) &Account)` reads
///     the signer's storage so the transaction can mint/borrow.
///   * `execute` does the actual state mutation.
///   * `post` enforces a transaction-level invariant.
///
/// The recorder must:
///   * surface each phase as a distinct top-level call frame
///     (`transaction.prepare`, `transaction.execute`,
///     `transaction.post` in the function table);
///   * emit phase-boundary io_events (`CadenceTxPhase:prepare`,
///     `:execute`, `:post`) so the frontend can highlight the
///     phase transitions independently of the call sequence.

transaction(amount: Int) {
    let recipient: Address

    prepare(signer: auth(Storage, Capabilities) &Account) {
        // Stage the recipient address out of the signer's storage.
        self.recipient = 0x02
    }

    execute {
        // The actual mutation: log the amount that would be sent.
        log("transfer ".concat(amount.toString()))
    }

    post {
        // Transaction-level post-condition.
        amount > 0: "amount must be positive"
    }
}
