//! Shared fixed-point arithmetic for interest and yield accrual.

pub const WAD: i128 = 1_000_000_000_000_000_000; // 18 decimals

pub fn wad_mul(a: i128, b: i128) -> i128 {
    (a.checked_mul(b).expect("overflow")) / WAD
}

pub fn wad_div(a: i128, b: i128) -> i128 {
    (a.checked_mul(WAD).expect("overflow")) / b
}
