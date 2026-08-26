pub fn lcm(a: i32, b: i32) -> i32 {
    if a == 0 || b == 0 {
        return 0;
    }
    (a.abs() / crate::gcd::gcd(a, b)) * b.abs()
}
