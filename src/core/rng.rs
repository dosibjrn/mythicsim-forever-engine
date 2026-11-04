//! SplitMix64 and labeled streams ported from the pinned MIT wowsims engine.

pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut result = self.0;
        result = (result ^ (result >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        result = (result ^ (result >> 27)).wrapping_mul(0x94d049bb133111eb);
        result ^ (result >> 31)
    }

    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
    }
}

pub fn labeled_seed(seed: u64, label: &str) -> u64 {
    let text = format!("{label}{seed:x}");
    let hash = text.bytes().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(16777619)
    });
    u64::from(hash)
}

/// Go's simulation random source: one shared stream, or one stream per label when the
/// request sets `useLabeledRands`. Every iteration reseeds with `seed + iteration`.
/// Cumulative quota RNG: E[# successes] follows sum of probabilities to within
/// 0.5 successes, independently for each event label across ALL fight iterations.
#[derive(Default)]
pub(crate) struct BalancedRng {
    binary: std::collections::HashMap<String, (f64, u64)>,
}
impl BalancedRng {
    fn proc(&mut self, p: f64, key: &str) -> bool {
        assert!(p.is_finite() && (0.0..=1.0).contains(&p),
            "balanced RNG: probability must be finite and in [0,1]");
        let (expected, successes) = self.binary.entry(key.to_owned()).or_default();
        *expected += p;
        let target = (*expected + 0.5).floor() as u64;
        // A single Bernoulli event can increase the rounded target by at most 1.
        let outcome = *successes < target;
        if outcome { *successes += 1; }
        outcome
    }
    fn float(&self, _key: &str) -> f64 {
        // For U[0,1), E[U] = 0.5. Callers doing NONLINEAR transformations
        // need explicit analytical expectations (or quadrature), separately.
        0.5
    }
}
#[derive(Clone, Copy)]
pub(crate) enum RandomRequest { Uniform01, Bernoulli(f64) }

/// Mandatory native runtime choice; never inferred from process environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RngMode { Balanced, Stochastic }

impl RngMode {
    pub fn parse_cli(text: &str) -> Result<Self, String> {
        match text {
            "balanced" => Ok(Self::Balanced),
            "stochastic" => Ok(Self::Stochastic),
            _ => Err(format!("invalid --rng-mode {text:?}; expected balanced or stochastic")),
        }
    }
}


pub(crate) enum RandomOutcome { Float(f64), Bool(bool) }

pub(crate) enum SimRng {
    Balanced(BalancedRng),
    Shared(SplitMix64),
    Labeled {
        seed: u64,
        streams: Vec<(String, SplitMix64)>,
    },
}

impl SimRng {
    /// The one entry point to both RNG modes. Callers must state whether
    /// their draw is continuous or a Bernoulli decision.
    pub(crate) fn sample(&mut self, request: RandomRequest, label: &str) -> RandomOutcome {
        match request {
            RandomRequest::Uniform01 => {
                let value = match self {
                    SimRng::Balanced(rng) => rng.float(label),
                    SimRng::Shared(rng) => rng.next_f64(),
                    SimRng::Labeled { seed, streams } => {
                        if let Some((_,rng)) = streams.iter_mut().find(|(name,_)| name == label) {
                            rng.next_f64()
                        } else {
                            let mut rng = SplitMix64::new(labeled_seed(*seed,label));
                            let value = rng.next_f64();
                            streams.push((label.to_owned(),rng));
                            value
                        }
                    }
                };
                RandomOutcome::Float(value)
            }
            RandomRequest::Bernoulli(p) => {
                // Match the pinned stochastic Proc(p) semantics at the
                // deterministic extremes before entering the quota sampler.
                // Do NOT clamp NaN: it remains an explicit invalid threshold.
                let value = match self {
                    _ if p>=1.0 => true,
                    _ if p<=0.0 => false,
                    SimRng::Balanced(rng) => rng.proc(p,label),
                    _ => matches!(self.sample(RandomRequest::Uniform01,label),
                        RandomOutcome::Float(x) if x < p),
                };
                RandomOutcome::Bool(value)
            }
        }
    }

    pub(crate) fn new(mode: RngMode, labeled: bool, seed: u64) -> Self {
        if mode == RngMode::Balanced {
            return SimRng::Balanced(BalancedRng::default());
        }
        if labeled {
            SimRng::Labeled {
                seed,
                streams: Vec::new(),
            }
        } else {
            SimRng::Shared(SplitMix64::new(seed))
        }
    }

    /// Go `reseedRands`: existing label streams restart from the new iteration seed and
    /// later labels are created from it.
    pub(crate) fn reseed(&mut self, iteration_seed: u64) {
        match self {
            // Do NOT clear quotas between fights; else low-frequency procs are biased.
            SimRng::Balanced(_) => {},
            SimRng::Shared(rng) => *rng = SplitMix64::new(iteration_seed),
            SimRng::Labeled { seed, streams } => {
                *seed = iteration_seed;
                for (label, rng) in streams.iter_mut() {
                    *rng = SplitMix64::new(labeled_seed(iteration_seed, label));
                }
            }
        }
    }

    /// Go `Proc(p, label)`: no draw at the extremes.
    pub(crate) fn proc(&mut self, chance: f64, label: &str) -> bool {
        match self.sample(RandomRequest::Bernoulli(chance), label) {
            RandomOutcome::Bool(value) => value,
            _ => unreachable!(),
        }
    }

    /// Go `RandomFloat(label)`.
    pub(crate) fn next_f64(&mut self, label: &str) -> f64 {
        match self.sample(RandomRequest::Uniform01, label) {
            RandomOutcome::Float(value) => value,
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix_matches_published_reference_vector() {
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xe220a8397b1dcdaf);
        assert_eq!(rng.next_u64(), 0x6e789e6aa1b965f4);
        assert_eq!(rng.next_u64(), 0x06c45d188009454f);
    }

    #[test]
    fn labeled_streams_are_independent_and_reseed_per_iteration() {
        let mut shared = SimRng::new(RngMode::Stochastic, false, 42);
        let mut labeled = SimRng::new(RngMode::Stochastic, true, 42);
        let first_hit = labeled.next_f64("Magical Hit Roll");
        let _ = labeled.next_f64("Damage Roll");
        assert_eq!(
            first_hit,
            SplitMix64::new(labeled_seed(42, "Magical Hit Roll")).next_f64()
        );
        labeled.reseed(43);
        assert_eq!(
            labeled.next_f64("Magical Hit Roll"),
            SplitMix64::new(labeled_seed(43, "Magical Hit Roll")).next_f64()
        );
        let a = shared.next_f64("Damage Roll");
        let b = shared.next_f64("Magical Hit Roll");
        assert_eq!((a, b), {
            let mut rng = SplitMix64::new(42);
            (rng.next_f64(), rng.next_f64())
        });
    }
}

#[cfg(test)]
mod wf652_balanced_tests {
    use super::*;
    #[test]
    fn continuous_uniform_is_expectation() {
        let mut rng=SimRng::Balanced(BalancedRng::default());
        for _ in 0..100 {
            assert_eq!(rng.next_f64("Damage Roll"),0.5);
        }
    }
    #[test]
    fn bool_frequency_closest_to_cumulative_probability() {
        let mut rng=BalancedRng::default();
        for (p,n) in [(0.13,100u64),(0.72,100u64),(0.001,1000u64)] {
            let key=format!("event-{p}");
            let observed=(0..n).filter(|_|rng.proc(p,&key)).count();
            assert!((observed as f64 - (n as f64)*p).abs() <= 0.500000001);
        }
    }
    #[test]
    fn variable_probability_and_label_isolation() {
        let mut rng=BalancedRng::default();
        let mut expected=0.0;
        let mut observed=0u64;
        for i in 0..100 {
            let p=(i%17) as f64 / 17.0;
            expected+=p;
            if rng.proc(p,"a"){observed+=1;}
            assert!((observed as f64-expected).abs()<=0.500000001);
            assert!(rng.proc(1.0,"b"));
            assert!(!rng.proc(0.0,"c"));
        }
    }
    #[test]
    fn quotas_persist_over_reseed() {
        let mut rng=SimRng::Balanced(BalancedRng::default());
        let mut observed=0;
        for i in 0..100 {
            rng.reseed(i);
            if rng.proc(0.13,"rare") { observed+=1; }
        }
        assert_eq!(observed,13);
    }
    #[test]
    fn mode_switch_independent_of_seed_for_balanced() {
        // Avoid mutating process-global env in this test. Explicit constructors
        // validate mode mechanics; CLI smoke validates env selection.
        let mut a=SimRng::Balanced(BalancedRng::default());
        let mut b=SimRng::Balanced(BalancedRng::default());
        for i in 0..100 {
            a.reseed(i);b.reseed(i+10000);
            assert_eq!(a.proc(0.23,"crit"),b.proc(0.23,"crit"));
            assert_eq!(a.next_f64("damage"),b.next_f64("damage"));
        }
    }
}

#[cfg(test)]
mod wf_l60_rng_extreme_tests {
    use super::*;

    #[test]
    fn over_one_physical_crit_matches_stochastic() {
        let mut balanced = SimRng::Balanced(BalancedRng::default());
        let mut stochastic = SimRng::Shared(SplitMix64::new(2710));
        for p in [1.3085, 1.317, 1.3185, 1.229995, 1.22586, 1.227925] {
            assert!(balanced.proc(p, "Physical Crit Roll"));
            assert!(stochastic.proc(p, "Physical Crit Roll"));
        }
    }

    #[test]
    fn negative_crit_is_false_in_both_modes() {
        let mut balanced = SimRng::Balanced(BalancedRng::default());
        let mut stochastic = SimRng::Shared(SplitMix64::new(2710));
        for p in [-0.25, -0.001, 0.0] {
            assert!(!balanced.proc(p, "Physical Crit Roll"));
            assert!(!stochastic.proc(p, "Physical Crit Roll"));
        }
    }

    #[test]
    #[should_panic(expected = "balanced RNG: probability must be finite and in [0,1]")]
    fn nan_remains_explicit_error_in_balanced_mode() {
        let mut balanced = SimRng::Balanced(BalancedRng::default());
        balanced.proc(f64::NAN, "Physical Crit Roll");
    }
}

#[cfg(test)]
mod typed_rng_api_tests {
    use super::*;
    #[test]
    fn required_mode_parses_without_environment_fallback() {
        assert_eq!(RngMode::parse_cli("balanced"), Ok(RngMode::Balanced));
        assert_eq!(RngMode::parse_cli("stochastic"), Ok(RngMode::Stochastic));
        assert!(RngMode::parse_cli("").is_err());
        assert!(RngMode::parse_cli("unknown").is_err());
    }
    #[test]
    fn balanced_constructor_is_seed_independent_and_stochastic_is_not() {
        let mut balanced_a = SimRng::new(RngMode::Balanced, false, 101);
        let mut balanced_b = SimRng::new(RngMode::Balanced, false, 100001);
        for _ in 0..100 {
            assert_eq!(balanced_a.proc(0.17, "crit"), balanced_b.proc(0.17, "crit"));
            assert_eq!(balanced_a.next_f64("damage"), balanced_b.next_f64("damage"));
        }
        let mut stochastic_a = SimRng::new(RngMode::Stochastic, false, 101);
        let mut stochastic_b = SimRng::new(RngMode::Stochastic, false, 100001);
        assert_ne!(stochastic_a.next_f64("damage"), stochastic_b.next_f64("damage"));
    }
}
