/// Cadence String method surface beyond construction.
///
/// Cadence `String` is a UTF-8 byte-counted value type with the
/// following method surface used by real-world contracts:
///
///   * `s.concat(other: String): String` — string concatenation.
///   * `s.slice(from: Int, upTo: Int): String` — substring extraction
///     by UTF-8 code-unit indices (half-open `[from, upTo)`).
///   * `s.utf8: [UInt8]` — UTF-8 byte representation surfaced as a
///     typed `[UInt8]` array.
///   * `s.length: Int` — UTF-8 grapheme/code-unit count.
///
/// Each method call surfaces in the trace as a Call/Return pair with
/// the input + output values typed end-to-end.  The strict pin asserts
/// the exact resulting string / byte / length values across every
/// method.

access(all) fun compute(): Int {
    let s: String = "hello"
    let greeted: String = s.concat(" world")
    let mid: String = s.slice(from: 1, upTo: 4)
    let bytes: [UInt8] = s.utf8
    let len: Int = s.length
    return len
}

access(all) fun main(): Int {
    return compute()
}
