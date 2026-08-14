pub mod add;
pub mod sub;
pub mod mul;
pub mod div;
pub mod clamp;
pub mod saturate;
pub mod gcd;
pub mod lcm;
pub mod min;
pub mod max;

#[test]
fn add_works() {
    assert_eq!(crate::add::add(2, 3), 5);
}

#[test]
fn sub_works() {
    assert_eq!(crate::sub::sub(5, 2), 3);
}

#[test]
fn mul_works() {
    assert_eq!(crate::mul::mul(3, 4), 12);
}

#[test]
fn div_works() {
    assert_eq!(crate::div::div(10, 2), 5);
}

#[test]
fn clamp_works() {
    assert_eq!(crate::clamp::clamp(15, 0, 10), 10);
}

#[test]
fn saturate_works() {
    assert_eq!(crate::saturate::saturate(-5, 0, 10), 0);
}

#[test]
fn gcd_works() {
    assert_eq!(crate::gcd::gcd(12, 8), 4);
}

#[test]
fn lcm_works() {
    assert_eq!(crate::lcm::lcm(4, 6), 12);
}

#[test]
fn min_works() {
    assert_eq!(crate::min::min(3, 1), 1);
}

#[test]
fn max_works() {
    assert_eq!(crate::max::max(3, 1), 3);
}
