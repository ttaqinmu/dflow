use std::sync::OnceLock;

static CONFIG: OnceLock<Config> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub include_test: bool,
}

impl Config {
    /// Initialize process-wide settings. Repeating the same settings is harmless.
    pub fn init(config: Self) -> Result<(), Self> {
        let current = CONFIG.get_or_init(|| config);
        if *current == config {
            Ok(())
        } else {
            Err(*current)
        }
    }

    pub fn current() -> &'static Self {
        CONFIG
            .get()
            .expect("dflow-core Config must be initialized before capture or analysis")
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn conflicting_reinitialization_preserves_first_value() {
        let include_tests = Config { include_test: true };
        assert_eq!(Config::init(include_tests), Ok(()));
        assert_eq!(Config::init(include_tests), Ok(()));
        assert_eq!(
            Config::init(Config {
                include_test: false
            }),
            Err(include_tests)
        );
        assert_eq!(*Config::current(), include_tests);
    }
}
