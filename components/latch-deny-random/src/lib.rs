use std::cell::RefCell;
use std::collections::BTreeMap;

use filesystem_latch::{Decision, ErrorCode, FilesystemErrorCode, Latch, Operation};

const LATCH_NAME: &str = "latch-deny-random";

/// The fraction of operations denied when `probability` is not configured.
const DEFAULT_PROBABILITY: f64 = 0.1;

mod random {
    wit_bindgen::generate!({
        path: "../wit",
        world: "insecure-random",
        generate_all
    });
}

struct Config {
    /// The fraction of operations denied, from 0 to 1, unless the interface or operation has its
    /// own probability.
    probability: f64,
    /// The probability of the operations of an interface, by interface name, e.g. `preopens`.
    interfaces: BTreeMap<String, f64>,
    /// The probability of an operation, by operation name, e.g. `descriptor.open-at`.
    operations: BTreeMap<String, f64>,
    /// Seeds the decisions, the same seed denies the same operations.
    seed: u64,
    reason: FilesystemErrorCode,
}

impl Config {
    /// Load the config, logging why when the config is invalid.
    fn load() -> Result<Config, ErrorCode> {
        let config = filesystem_latch::load_config(LATCH_NAME, |config| {
            Config::parse(config, || {
                random::wasi::random::insecure::get_insecure_random_u64()
            })
        })?;
        // the seed reproduces the decisions, e.g. a failure the random seed uncovered
        let overrides: String = config
            .interfaces
            .iter()
            .chain(&config.operations)
            .map(|(name, probability)| format!(" PROBABILITY.{name}={probability}"))
            .collect();
        filesystem_latch::warn!(
            "Randomly denying wasi:filesystem operations PROBABILITY={} SEED={}{overrides}",
            config.probability,
            config.seed
        );
        Ok(config)
    }

    fn parse(
        config: Vec<(String, String)>,
        random_seed: impl FnOnce() -> u64,
    ) -> Result<Config, String> {
        let mut probability = DEFAULT_PROBABILITY;
        let mut interfaces = BTreeMap::new();
        let mut operations = BTreeMap::new();
        let mut seed = None;
        let mut reason = FilesystemErrorCode::Access;

        for (key, value) in config {
            let invalid = |err: &str| format!("KEY={key} VALUE={value} ERROR={err}");
            let parse_probability = || {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|probability| (0.0..=1.0).contains(probability))
                    .ok_or_else(|| invalid("expected a number from 0 to 1"))
            };
            match key.as_str() {
                "probability" => probability = parse_probability()?,
                _ if key.starts_with("probability.") => {
                    let name = &key["probability.".len()..];
                    if filesystem_latch::INTERFACE_NAMES.contains(&name) {
                        interfaces.insert(name.to_string(), parse_probability()?);
                    } else if filesystem_latch::OPERATION_NAMES.contains(&name) {
                        operations.insert(name.to_string(), parse_probability()?);
                    } else {
                        return Err(invalid("unknown interface or operation"));
                    }
                }
                "seed" => {
                    seed = Some(
                        value
                            .parse::<u64>()
                            .map_err(|_| invalid("expected an unsigned 64 bit integer"))?,
                    );
                }
                "reason" => {
                    reason = filesystem_latch::reason(value).unwrap_or(FilesystemErrorCode::Access);
                }
                _ => {}
            }
        }

        Ok(Config {
            probability,
            interfaces,
            operations,
            seed: seed.unwrap_or_else(random_seed),
            reason,
        })
    }

    /// The probability of the named operation, e.g. `descriptor.open-at`: its own, its interface's,
    /// or the default.
    fn probability(&self, operation: &str) -> f64 {
        let interface = operation.split_once('.').map_or(operation, |(interface, _)| interface);
        self.operations
            .get(operation)
            .or_else(|| self.interfaces.get(interface))
            .copied()
            .unwrap_or(self.probability)
    }

    /// The decision for the named operation observed after `observed` others.
    fn decide(&self, operation: &str, observed: u64) -> Decision {
        match sample(self.seed, observed) < self.probability(operation) {
            true => Decision::Denied(self.reason.clone()),
            false => Decision::Deferred,
        }
    }
}

/// A uniformly distributed number from 0 up to 1, the same for the same seed and index.
fn sample(seed: u64, index: u64) -> f64 {
    // splitmix64, each index is a step of the sequence the seed starts
    let mut z = seed.wrapping_add(index.wrapping_add(1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    // the top 53 bits, the precision of an f64
    (z >> 11) as f64 / (1u64 << 53) as f64
}

thread_local! {
    /// `None` until the config is loaded.
    static CONFIG: RefCell<Option<Result<Config, ErrorCode>>> = const { RefCell::new(None) };

    /// The number of operations observed.
    static OBSERVED: RefCell<u64> = const { RefCell::new(0) };
}

struct RandomLatch {}

impl Latch for RandomLatch {
    fn authorize(operation: Operation) -> Result<Decision, ErrorCode> {
        // the decision only depends on how many operations were observed, authorizing does not
        // change it, so an operation another latch denied first does not shift the decisions
        CONFIG.with_borrow_mut(|config| match config.get_or_insert_with(Config::load) {
            Ok(config) => {
                let observed = OBSERVED.with_borrow(|observed| *observed);
                Ok(config.decide(filesystem_latch::operation_name(&operation), observed))
            }
            Err(err) => Err(err.clone()),
        })
    }

    fn observe_decision(_final_decision: Decision, _operation: Operation) -> Result<(), ErrorCode> {
        OBSERVED.with_borrow_mut(|observed| *observed += 1);
        Ok(())
    }
}

filesystem_latch::export!(RandomLatch with_types_in filesystem_latch::bindings);

#[cfg(test)]
mod tests {
    use super::*;

    fn config(entries: &[(&str, &str)]) -> Result<Config, String> {
        let entries = entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        Config::parse(entries, || 42)
    }

    fn denied(config: &Config, operations: u64) -> Vec<u64> {
        (0..operations)
            .filter(|observed| matches!(config.decide("descriptor.stat", *observed), Decision::Denied(_)))
            .collect()
    }

    #[test]
    fn defaults() {
        let config = config(&[]).expect("valid config");
        assert_eq!(config.probability, DEFAULT_PROBABILITY);
        assert_eq!(config.seed, 42, "seeded randomly");
        assert!(matches!(config.reason, FilesystemErrorCode::Access));
    }

    #[test]
    fn denies_about_the_configured_fraction() {
        for probability in [0.1, 0.5, 0.9] {
            let config = config(&[("probability", &probability.to_string()), ("seed", "7")])
                .expect("valid config");
            let fraction = denied(&config, 10_000).len() as f64 / 10_000.0;
            assert!(
                (fraction - probability).abs() < 0.02,
                "probability={probability} fraction={fraction}"
            );
        }
    }

    #[test]
    fn never_or_always_denies() {
        let never = config(&[("probability", "0")]).expect("valid config");
        assert!(denied(&never, 1_000).is_empty());
        let always = config(&[("probability", "1")]).expect("valid config");
        assert_eq!(denied(&always, 1_000).len(), 1_000);
    }

    #[test]
    fn same_seed_same_decisions() {
        let first = config(&[("probability", "0.5"), ("seed", "1")]).expect("valid config");
        let again = config(&[("probability", "0.5"), ("seed", "1")]).expect("valid config");
        let other = config(&[("probability", "0.5"), ("seed", "2")]).expect("valid config");
        assert_eq!(denied(&first, 100), denied(&again, 100));
        assert_ne!(denied(&first, 100), denied(&other, 100));
    }

    #[test]
    fn denies_with_configured_reason() {
        let config = config(&[("probability", "1"), ("reason", "io")]).expect("valid config");
        assert!(matches!(
            config.decide("descriptor.stat", 0),
            Decision::Denied(FilesystemErrorCode::Io)
        ));
    }

    #[test]
    fn unknown_reason_is_other() {
        let config =
            config(&[("probability", "1"), ("reason", "chaos-monkey")]).expect("valid config");
        assert!(matches!(
            config.decide("descriptor.stat", 0),
            Decision::Denied(FilesystemErrorCode::Other(Some(ref message))) if message == "chaos-monkey"
        ));
    }

    #[test]
    fn probability_varies_by_interface_and_operation() {
        let varied = config(&[
            ("probability", "0.2"),
            ("probability.preopens", "0"),
            ("probability.descriptor", "0.5"),
            ("probability.descriptor.open-at", "1"),
            ("probability.preopens.get-directories-item", "0.3"),
        ])
        .expect("valid config");
        assert_eq!(varied.probability("preopens.get-directories"), 0.0);
        assert_eq!(varied.probability("preopens.get-directories-item"), 0.3);
        assert_eq!(varied.probability("descriptor.stat"), 0.5);
        assert_eq!(varied.probability("descriptor.open-at"), 1.0);

        let default = config(&[("probability", "0.2")]).expect("valid config");
        assert_eq!(default.probability("preopens.get-directories"), 0.2);
        assert_eq!(default.probability("descriptor.open-at"), 0.2);
    }

    #[test]
    fn operation_probability_decides() {
        let config = config(&[("probability", "1"), ("probability.preopens", "0")])
            .expect("valid config");
        assert!(matches!(
            config.decide("preopens.get-directories", 0),
            Decision::Deferred
        ));
        assert!(matches!(
            config.decide("descriptor.stat", 0),
            Decision::Denied(_)
        ));
    }

    #[test]
    fn invalid_config() {
        for (key, value) in [
            ("probability", "1.5"),
            ("probability", "-0.1"),
            ("probability", "often"),
            ("seed", "-1"),
            ("seed", "random"),
            ("probability.preopens", "often"),
            ("probability.descriptor.open-at", "2"),
        ] {
            let err = config(&[(key, value)]).err().expect("invalid config");
            assert!(
                err.starts_with(&format!("KEY={key} VALUE={value} ERROR=")),
                "{err}"
            );
        }
    }

    #[test]
    fn unknown_interface_or_operation_is_invalid() {
        for key in [
            "probability.sockets",
            "probability.descriptor.open",
            "probability.open-at",
            "probability.",
        ] {
            assert_eq!(
                config(&[(key, "0")]).err().expect("invalid config"),
                format!("KEY={key} VALUE=0 ERROR=unknown interface or operation")
            );
        }
    }
}
