/// SplitMix64, written out rather than taken from a crate because the fixture's bytes are a contract:
/// the same seed must give the same workspace across releases, and `rand` doesn't promise value
/// stability for its generators between versions.
#[derive(Debug, Clone)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// uniform in `0..bound` by Lemire's widening multiply; the slight bias for bounds that don't
    /// divide 2^64 is far below anything a fixture shape could show.
    pub(crate) fn below(&mut self, bound: usize) -> usize {
        assert!(bound > 0, "below() needs a non-empty range");
        ((u128::from(self.next_u64()) * bound as u128) >> 64) as usize
    }

    pub(crate) fn percent(&mut self, percent: u8) -> bool {
        self.below(100) < usize::from(percent)
    }
}

#[cfg(test)]
mod tests {
    use super::SplitMix64;

    // reference outputs for seed 1234567 from the published SplitMix64 algorithm; a change here
    // changes every generated fixture.
    #[test]
    fn matches_reference_sequence() {
        let mut rng = SplitMix64::new(1_234_567);
        let got: Vec<u64> = (0..3).map(|_| rng.next_u64()).collect();
        assert_eq!(
            got,
            [
                6_457_827_717_110_365_317,
                3_203_168_211_198_807_973,
                9_817_491_932_198_370_423
            ]
        );
    }

    #[test]
    fn below_stays_in_range() {
        let mut rng = SplitMix64::new(7);
        for bound in [1, 2, 3, 10, 1000] {
            for _ in 0..1000 {
                assert!(rng.below(bound) < bound);
            }
        }
    }
}
