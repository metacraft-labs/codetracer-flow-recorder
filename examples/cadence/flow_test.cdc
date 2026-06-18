access(all) fun compute(): Int {
    let a: Int = 10
    let b: Int = 32
    let sum_val: Int = a + b
    let doubled: Int = sum_val * 2
    let final_result: Int = doubled + a
    return final_result
}

access(all) fun main(): Int {
    return compute()
}
