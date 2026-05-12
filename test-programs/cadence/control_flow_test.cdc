/// Control-flow exercise: if/else, while, for-in, switch, early return.
///
/// Driven via the NDJSON test path (the Go helper would emit equivalent
/// events when this script is executed by the real Cadence runtime).
///
/// Semantics being asserted by `test_control_flow_test_via_ct_print_full`:
///   raw           = 7
///   sign          = +1   (if raw > 0)
///   loop_total    = 6    (sum of 1+2+3 via while)
///   for_total     = 10   (sum of 1+2+3+4 via for-in over [1,2,3,4])
///   switch_label  = "small"  (when sign == 1)
///   early         = 999  (early return short-circuits the rest)

access(all) fun classify(n: Int): String {
    if n > 10 {
        return "big"
    } else if n > 0 {
        return "small"
    } else {
        return "zero-or-negative"
    }
}

access(all) fun while_sum(limit: Int): Int {
    var i: Int = 1
    var total: Int = 0
    while i <= limit {
        total = total + i
        i = i + 1
    }
    return total
}

access(all) fun for_sum(xs: [Int]): Int {
    var total: Int = 0
    for x in xs {
        total = total + x
    }
    return total
}

access(all) fun switch_label(s: Int): String {
    switch s {
        case 1:
            return "small"
        case 2:
            return "medium"
        default:
            return "other"
    }
}

access(all) fun early_return(flag: Bool): Int {
    if flag {
        return 999
    }
    return 0
}

access(all) fun compute(): Int {
    let raw: Int = 7
    let sign_label: String = classify(n: raw)
    let loop_total: Int = while_sum(limit: 3)
    let for_total: Int = for_sum(xs: [1, 2, 3, 4])
    let switch_result: String = switch_label(s: 1)
    let early: Int = early_return(flag: true)
    return raw + loop_total + for_total + early
}

access(all) fun main(): Int {
    return compute()
}
