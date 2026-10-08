//! Reserve ratio verification before mortgage capital commitment.

pub const MIN_RESERVE_RATIO_BPS: i128 = 1000; // 10% reserve floor

pub fn check_reserve_ratio(total_capital: i128, committed_capital: i128) -> bool {
    if total_capital <= 0 {
        return false;
    }
    let uncommitted = total_capital - committed_capital;
    (uncommitted * 10_000) / total_capital >= MIN_RESERVE_RATIO_BPS
}
