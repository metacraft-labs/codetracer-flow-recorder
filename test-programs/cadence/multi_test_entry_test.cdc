/// Multiple top-level test entries in a single Cadence fixture.
///
/// Cadence supports multiple top-level entry points in a single source
/// file (a script form `main`, a transaction form `prepare/execute/
/// post`, plus auxiliary helpers).  Real-world usage includes the
/// Cadence test framework where one .cdc file declares multiple
/// `testCase*` functions exercised by the runner.
///
/// The recorder must process each top-level entry as an independent
/// Function in the function table so the call subtree for each entry
/// stays isolated from the others.  This fixture exercises three
/// disjoint top-level functions (`testFoo`, `testBar`, `testBaz`)
/// each with its own helper, and pins:
///
///   * The function table contains all 3 entry-point functions plus
///     their 3 helpers (6 total).
///   * Each entry's call subtree appears as an independent Call/Return
///     pair in event-emission order — the recorder processes them in
///     declaration order without cross-contaminating arg/local frames.

access(all) fun helperFoo(): Int {
    return 1
}

access(all) fun helperBar(): Int {
    return 2
}

access(all) fun helperBaz(): Int {
    return 3
}

access(all) fun testFoo(): Int {
    return helperFoo()
}

access(all) fun testBar(): Int {
    return helperBar()
}

access(all) fun testBaz(): Int {
    return helperBaz()
}
