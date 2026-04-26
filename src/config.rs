use std::env;
use std::fmt;

#[derive(Clone)]
pub struct Config {
    pub jellyfin_url: String,
    pub jellyfin_api_key: String,
    pub port: u16,
    pub scrape_interval_ms: u64,
    pub log_level: LogLevel,
    pub request_timeout_ms: u64,
    pub retry_max_attempts: u32,
    pub retry_base_delay_ms: u64,
    pub retry_max_delay_ms: u64,
    pub circuit_breaker_threshold: u32,
    pub circuit_breaker_reset_ms: u64,
    pub metrics_token: Option<String>,
    /// Whether to expose `jellyfin_session_remote_address` (opt-in, default
    /// `false`). The metric labels each active session with the IP address
    /// the client connected from — useful for "is anyone playing from
    /// outside the LAN" panels, but PII-adjacent so it ships disabled.
    pub expose_remote_address: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace => write!(f, "trace"),
            Self::Debug => write!(f, "debug"),
            Self::Info => write!(f, "info"),
            Self::Warn => write!(f, "warn"),
            Self::Error => write!(f, "error"),
        }
    }
}

impl LogLevel {
    fn parse(s: &str) -> Result<Self, ConfigError> {
        match s.to_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(ConfigError::InvalidValue {
                name: "LOG_LEVEL".into(),
                value: s.into(),
                reason: "must be one of: trace, debug, info, warn, error".into(),
            }),
        }
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("jellyfin_url", &self.jellyfin_url)
            .field("jellyfin_api_key", &"[REDACTED]")
            .field("port", &self.port)
            .field("scrape_interval_ms", &self.scrape_interval_ms)
            .field("log_level", &self.log_level)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("retry_max_attempts", &self.retry_max_attempts)
            .field("retry_base_delay_ms", &self.retry_base_delay_ms)
            .field("retry_max_delay_ms", &self.retry_max_delay_ms)
            .field("circuit_breaker_threshold", &self.circuit_breaker_threshold)
            .field("circuit_breaker_reset_ms", &self.circuit_breaker_reset_ms)
            .field(
                "metrics_token",
                &self.metrics_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("expose_remote_address", &self.expose_remote_address)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("required environment variable {name} is not set")]
    MissingEnvVar { name: String },

    #[error("invalid value for {name}={value}: {reason}")]
    InvalidValue {
        name: String,
        value: String,
        reason: String,
    },
}

impl Config {
    /// Read configuration from process environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::MissingEnvVar`] when a required variable is
    /// unset or empty (`JELLYFIN_URL`, `JELLYFIN_API_KEY`).
    ///
    /// Returns [`ConfigError::InvalidValue`] when an optional variable is
    /// set to an unparsable value (e.g. `PORT=abc`) or fails its constraint
    /// (e.g. `SCRAPE_INTERVAL_MS` below the 1000 ms floor). The error
    /// includes the variable name, the offending value, and the constraint.
    pub fn from_env() -> Result<Self, ConfigError> {
        let jellyfin_url = require_env("JELLYFIN_URL")?
            .trim_end_matches('/')
            .to_owned();

        let jellyfin_api_key = require_env("JELLYFIN_API_KEY")?;

        let port = parse_optional::<u16>("PORT", 9711)?;
        validate_range("PORT", u64::from(port), 1, 65_535)?;

        let scrape_interval_ms = parse_optional::<u64>("SCRAPE_INTERVAL_MS", 10_000)?;
        validate_min("SCRAPE_INTERVAL_MS", scrape_interval_ms, 1000)?;

        let log_level = match env::var("LOG_LEVEL") {
            Ok(val) => LogLevel::parse(&val)?,
            Err(_) => LogLevel::Info,
        };

        let request_timeout_ms = parse_optional::<u64>("REQUEST_TIMEOUT_MS", 5000)?;
        validate_min("REQUEST_TIMEOUT_MS", request_timeout_ms, 100)?;

        let retry_max_attempts = parse_optional::<u32>("RETRY_MAX_ATTEMPTS", 3)?;

        let retry_base_delay_ms = parse_optional::<u64>("RETRY_BASE_DELAY_MS", 500)?;
        validate_min("RETRY_BASE_DELAY_MS", retry_base_delay_ms, 50)?;

        let retry_max_delay_ms = retry_base_delay_ms * 10;

        let circuit_breaker_threshold = parse_optional::<u32>("CIRCUIT_BREAKER_THRESHOLD", 5)?;
        validate_min(
            "CIRCUIT_BREAKER_THRESHOLD",
            u64::from(circuit_breaker_threshold),
            1,
        )?;

        let circuit_breaker_reset_ms = parse_optional::<u64>("CIRCUIT_BREAKER_RESET_MS", 60_000)?;
        validate_min("CIRCUIT_BREAKER_RESET_MS", circuit_breaker_reset_ms, 1000)?;

        let metrics_token = env::var("METRICS_TOKEN").ok().filter(|s| !s.is_empty());

        let expose_remote_address = parse_bool_optional("EXPOSE_REMOTE_ADDRESS", false)?;

        Ok(Self {
            jellyfin_url,
            jellyfin_api_key,
            port,
            scrape_interval_ms,
            log_level,
            request_timeout_ms,
            retry_max_attempts,
            retry_base_delay_ms,
            retry_max_delay_ms,
            circuit_breaker_threshold,
            circuit_breaker_reset_ms,
            metrics_token,
            expose_remote_address,
        })
    }
}

fn require_env(name: &str) -> Result<String, ConfigError> {
    let value = env::var(name).map_err(|_| ConfigError::MissingEnvVar { name: name.into() })?;

    if value.is_empty() {
        return Err(ConfigError::MissingEnvVar { name: name.into() });
    }

    Ok(value)
}

fn parse_bool_optional(name: &str, default: bool) -> Result<bool, ConfigError> {
    env::var(name).map_or(Ok(default), |val| match val.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" | "" => Ok(false),
        _ => Err(ConfigError::InvalidValue {
            name: name.into(),
            value: val,
            reason: "must be one of: true/false, 1/0, yes/no, on/off".into(),
        }),
    })
}

fn parse_optional<T>(name: &str, default: T) -> Result<T, ConfigError>
where
    T: std::str::FromStr,
    T::Err: fmt::Display,
{
    env::var(name).map_or(Ok(default), |val| {
        val.parse::<T>().map_err(|e| ConfigError::InvalidValue {
            name: name.into(),
            value: val,
            reason: e.to_string(),
        })
    })
}

fn validate_min(name: &str, value: u64, min: u64) -> Result<(), ConfigError> {
    if value < min {
        return Err(ConfigError::InvalidValue {
            name: name.into(),
            value: value.to_string(),
            reason: format!("must be at least {min}"),
        });
    }
    Ok(())
}

fn validate_range(name: &str, value: u64, min: u64, max: u64) -> Result<(), ConfigError> {
    if value < min || value > max {
        return Err(ConfigError::InvalidValue {
            name: name.into(),
            value: value.to_string(),
            reason: format!("must be between {min} and {max}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // SAFETY invariant for all unsafe blocks in this module:
    // `env::set_var`/`remove_var` are unsafe in Rust 2024 because they mutate
    // global process state. These tests run with `--test-threads=1` (enforced in
    // CI) so no concurrent env mutation occurs.

    fn set_required_env() {
        // SAFETY: single-threaded test execution (see module comment)
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("JELLYFIN_URL", "http://jellyfin:8096");
            env::set_var("JELLYFIN_API_KEY", "test-api-key");
        }
    }

    fn clear_all_env() {
        let vars = [
            "JELLYFIN_URL",
            "JELLYFIN_API_KEY",
            "PORT",
            "SCRAPE_INTERVAL_MS",
            "LOG_LEVEL",
            "REQUEST_TIMEOUT_MS",
            "RETRY_MAX_ATTEMPTS",
            "RETRY_BASE_DELAY_MS",
            "CIRCUIT_BREAKER_THRESHOLD",
            "CIRCUIT_BREAKER_RESET_MS",
            "METRICS_TOKEN",
            "EXPOSE_REMOTE_ADDRESS",
        ];
        for var in vars {
            // SAFETY: single-threaded test execution (see module comment)
            unsafe {
                env::remove_var(var);
            }
        }
    }

    #[test]
    fn valid_config_with_defaults() {
        clear_all_env();
        set_required_env();

        let config = Config::from_env().unwrap();

        assert_eq!(config.jellyfin_url, "http://jellyfin:8096");
        assert_eq!(config.jellyfin_api_key, "test-api-key");
        assert_eq!(config.port, 9711);
        assert_eq!(config.scrape_interval_ms, 10_000);
        assert_eq!(config.log_level, LogLevel::Info);
        assert_eq!(config.request_timeout_ms, 5000);
        assert_eq!(config.retry_max_attempts, 3);
        assert_eq!(config.retry_base_delay_ms, 500);
        assert_eq!(config.retry_max_delay_ms, 5000);
        assert_eq!(config.circuit_breaker_threshold, 5);
        assert_eq!(config.circuit_breaker_reset_ms, 60_000);
        assert!(!config.expose_remote_address);
    }

    #[test]
    fn expose_remote_address_parses() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("EXPOSE_REMOTE_ADDRESS", "true");
        }
        let config = Config::from_env().unwrap();
        assert!(config.expose_remote_address);
    }

    #[test]
    fn expose_remote_address_rejects_garbage() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("EXPOSE_REMOTE_ADDRESS", "maybe");
        }
        let err = Config::from_env().unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "EXPOSE_REMOTE_ADDRESS")
        );
    }

    #[test]
    fn trailing_slash_stripped() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("JELLYFIN_URL", "http://jellyfin:8096///");
        }

        let config = Config::from_env().unwrap();
        assert_eq!(config.jellyfin_url, "http://jellyfin:8096");
    }

    #[test]
    fn missing_url_fails() {
        clear_all_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("JELLYFIN_API_KEY", "key");
        }

        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::MissingEnvVar { ref name } if name == "JELLYFIN_URL"));
    }

    #[test]
    fn missing_api_key_fails() {
        clear_all_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("JELLYFIN_URL", "http://jellyfin:8096");
        }

        let err = Config::from_env().unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingEnvVar { ref name } if name == "JELLYFIN_API_KEY")
        );
    }

    #[test]
    fn empty_required_var_fails() {
        clear_all_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("JELLYFIN_URL", "http://jellyfin:8096");
            env::set_var("JELLYFIN_API_KEY", "");
        }

        let err = Config::from_env().unwrap_err();
        assert!(
            matches!(err, ConfigError::MissingEnvVar { ref name } if name == "JELLYFIN_API_KEY")
        );
    }

    #[test]
    fn invalid_port_fails() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("PORT", "not-a-number");
        }

        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "PORT"));
    }

    #[test]
    fn port_zero_fails() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("PORT", "0");
        }

        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "PORT"));
    }

    #[test]
    fn scrape_interval_too_low_fails() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("SCRAPE_INTERVAL_MS", "500");
        }

        let err = Config::from_env().unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "SCRAPE_INTERVAL_MS")
        );
    }

    #[test]
    fn invalid_log_level_fails() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("LOG_LEVEL", "verbose");
        }

        let err = Config::from_env().unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "LOG_LEVEL"));
    }

    #[test]
    fn log_level_case_insensitive() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("LOG_LEVEL", "DEBUG");
        }

        let config = Config::from_env().unwrap();
        assert_eq!(config.log_level, LogLevel::Debug);
    }

    #[test]
    fn custom_optional_values() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("PORT", "3000");
            env::set_var("SCRAPE_INTERVAL_MS", "30000");
            env::set_var("REQUEST_TIMEOUT_MS", "10000");
            env::set_var("RETRY_MAX_ATTEMPTS", "5");
            env::set_var("RETRY_BASE_DELAY_MS", "1000");
            env::set_var("CIRCUIT_BREAKER_THRESHOLD", "10");
            env::set_var("CIRCUIT_BREAKER_RESET_MS", "120000");
        }

        let config = Config::from_env().unwrap();
        assert_eq!(config.port, 3000);
        assert_eq!(config.scrape_interval_ms, 30_000);
        assert_eq!(config.request_timeout_ms, 10_000);
        assert_eq!(config.retry_max_attempts, 5);
        assert_eq!(config.retry_base_delay_ms, 1000);
        assert_eq!(config.retry_max_delay_ms, 10_000);
        assert_eq!(config.circuit_breaker_threshold, 10);
        assert_eq!(config.circuit_breaker_reset_ms, 120_000);
    }

    #[test]
    fn debug_redacts_api_key() {
        clear_all_env();
        set_required_env();

        let config = Config::from_env().unwrap();
        let debug_output = format!("{config:?}");

        assert!(debug_output.contains("[REDACTED]"));
        assert!(!debug_output.contains("test-api-key"));
    }

    #[test]
    fn circuit_breaker_threshold_zero_fails() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("CIRCUIT_BREAKER_THRESHOLD", "0");
        }

        let err = Config::from_env().unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "CIRCUIT_BREAKER_THRESHOLD")
        );
    }

    #[test]
    fn request_timeout_too_low_fails() {
        clear_all_env();
        set_required_env();
        // SAFETY: single-threaded test execution (see module comment)
        unsafe {
            env::set_var("REQUEST_TIMEOUT_MS", "50");
        }

        let err = Config::from_env().unwrap_err();
        assert!(
            matches!(err, ConfigError::InvalidValue { ref name, .. } if name == "REQUEST_TIMEOUT_MS")
        );
    }
}
