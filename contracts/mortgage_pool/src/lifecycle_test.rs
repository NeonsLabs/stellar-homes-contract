#[cfg(test)]
mod tests {
    #[test]
    fn test_milestone_stage_sequence() {
        let stages: [u32; 5] = [0, 1, 2, 3, 4];
        for (i, stage) in stages.iter().enumerate() {
            assert_eq!(*stage, i as u32);
        }
    }
}
