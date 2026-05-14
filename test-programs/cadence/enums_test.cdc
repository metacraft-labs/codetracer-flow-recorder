/// Cadence enum with backing integer type.
///
/// Cadence enums shape: `enum Color: UInt8 { case red; case green;
/// case blue }`.  This fixture exercises the three observable
/// surfaces of an enum:
///
///   * Constructor:          `Color.red` / `Color.green` / `Color.blue`
///   * Switch pattern match: `switch c { case Color.red: ... }`
///   * `rawValue` accessor:  `c.rawValue` returns the backing UInt8
///
/// The recorder must surface each enum case as
/// `ValueRecord::Variant { discriminator: "Color.<case>", contents:
/// ValueRecord::None, type_id }` (the M9 Variant path), and the
/// `rawValue` access as `ValueRecord::Int` matching the backing-type
/// width (UInt8 → an Int with i in 0..255).

access(all) enum Color: UInt8 {
    access(all) case red
    access(all) case green
    access(all) case blue
}

access(all) fun classify(c: Color): Int {
    switch c {
        case Color.red:
            return 1
        case Color.green:
            return 2
        case Color.blue:
            return 3
    }
    return 0
}

access(all) fun compute(): Int {
    let r: Color = Color.red
    let g: Color = Color.green
    let b: Color = Color.blue

    let r_raw: UInt8 = r.rawValue
    let g_raw: UInt8 = g.rawValue
    let b_raw: UInt8 = b.rawValue

    let kind_r: Int = classify(c: r)
    let kind_g: Int = classify(c: g)
    let kind_b: Int = classify(c: b)

    return Int(r_raw) + Int(g_raw) + Int(b_raw) + kind_r + kind_g + kind_b
}

access(all) fun main(): Int {
    return compute()
}
