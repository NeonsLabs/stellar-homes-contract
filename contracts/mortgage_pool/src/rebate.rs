//! Early repayment discount curve calculation.

pub fn compute_early_payoff_rebate(principal_remaining: i128, elapsed_months: u32, total_months: u32) -> i128 {
    if elapsed_months >= total_months || total_months == 0 {
        return 0;
    }
    let remaining_months = (total_months - elapsed_months) as i128;
    // Rebate formula: proportional reduction on unaccrued interest
    (principal_remaining * remaining_months) / (total_months as i128 * 100)
}
