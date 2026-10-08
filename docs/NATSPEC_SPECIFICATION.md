# StellarHomes Smart Contract NatSpec Reference

## Contracts Overview
1. **PropertyRegistry** (`sh-property-registry`):
   - `submit_property(trustee, title_hash, survey_doc_hash)`: Register property parcel
   - `verify_title(oracle, property_id)`: Land registry title verification
   - `submit_milestone_evidence(trustee, property_id, stage, evidence_hash)`: Progress report

2. **LendingPool** (`sh-lending-pool`):
   - `deposit(investor, amount)`: Supply USDC liquidity
   - `withdraw(investor, shares)`: Redeem uncommitted capital
   - `claim(investor)`: Harvest accrued interest

3. **MortgagePool** (`sh-mortgage-pool`):
   - `apply(borrower, property_id, principal, rate_bps, term_months)`: Originate facility
   - `approve(underwriter, mortgage_id)`: Reserve pool capital
   - `disburse(mortgage_id, stage)`: Release milestone funds
   - `repay(mortgage_id, amount)`: Monthly debt service
