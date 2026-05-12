/// Collections + struct + optional + dictionary exercise.
///
/// Asserts the recorder surfaces:
///   * arrays as `Sequence` ValueRecord
///   * dictionaries as `HashMap` (or whatever the trace-format names them)
///   * structs as `Struct`
///   * optionals as `Variant`
///
/// The current Flow recorder encodes everything as String/Int via the
/// NDJSON variable path — see the `RECORDER BUG` notes attached to the
/// `#[ignore]`d sibling test for the spec-compliant expectation.

access(all) struct Point {
    access(all) let x: Int
    access(all) let y: Int

    init(x: Int, y: Int) {
        self.x = x
        self.y = y
    }
}

access(all) fun array_sum(xs: [Int]): Int {
    var total: Int = 0
    for x in xs {
        total = total + x
    }
    return total
}

access(all) fun dict_lookup(d: {String: Int}, key: String): Int {
    if let v = d[key] {
        return v
    }
    return -1
}

access(all) fun point_distance_sq(p: Point): Int {
    return p.x * p.x + p.y * p.y
}

access(all) fun maybe_double(o: Int?): Int {
    if let v = o {
        return v * 2
    }
    return 0
}

access(all) fun compute(): Int {
    let xs: [Int] = [1, 2, 3, 4]
    let total: Int = array_sum(xs: xs)

    let prices: {String: Int} = {"apple": 30, "banana": 10}
    let apple_price: Int = dict_lookup(d: prices, key: "apple")

    let p: Point = Point(x: 3, y: 4)
    let dist_sq: Int = point_distance_sq(p: p)

    let some_val: Int? = 7
    let doubled: Int = maybe_double(o: some_val)

    return total + apple_price + dist_sq + doubled
}

access(all) fun main(): Int {
    return compute()
}
