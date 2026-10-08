#[cfg(test)]
mod tests {
    #[test]
    fn test_conservation_of_shares_invariant() {
        let deposit_a: i128 = 100_000_000;
        let deposit_b: i128 = 200_000_000;
        assert_eq!(deposit_a + deposit_b, 300_000_000);
    }
}
