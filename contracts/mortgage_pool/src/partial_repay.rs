//! Partial installment deduction and arrears allocation.

pub struct RepaymentSplit {
    pub interest_paid: i128,
    pub principal_paid: i128,
    pub remaining_arrears: i128,
}

pub fn split_payment(amount: i128, interest_due: i128, principal_due: i128) -> RepaymentSplit {
    if amount <= interest_due {
        RepaymentSplit {
            interest_paid: amount,
            principal_paid: 0,
            remaining_arrears: (interest_due - amount) + principal_due,
        }
    } else {
        let principal_paid = (amount - interest_due).min(principal_due);
        RepaymentSplit {
            interest_paid: interest_due,
            principal_paid,
            remaining_arrears: principal_due - principal_paid,
        }
    }
}
