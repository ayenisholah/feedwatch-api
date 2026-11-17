//! Configuration from the environment (see `.env.example`), validated once at start.
//!
//! Supports FR-FW-6 (InfluxDB settings) and GR-2 (the CORS origin).

use std::net::SocketAddr;

use thiserror::Error;

use crate::influx::InfluxConfig;

/// Everything the API reads from the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Listen address, `FEEDWATCH_API_ADDR` (default `0.0.0.0:8080`).
    pub addr: SocketAddr,
    /// InfluxDB connection settings.
    pub influx: InfluxConfig,
    /// The site origin CORS allows, `FEEDWATCH_ALLOWED_ORIGIN`.
    pub allowed_origin: String,
}

/// A missing or invalid setting.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    /// A required variable is unset or empty.
    #[error("{0} is required")]
    Missing(&'static str),
    /// A variable is set but can't be used.
    #[error("{name} is invalid: {reason}")]
    Invalid {
        /// The variable's name.
        name: &'static str,
        /// What's wrong with it.
        reason: String,
    },
}

impl Config {
    /// Reads the process environment.
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads settings through `lookup`, so tests needn't touch the process environment.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Result<Config, ConfigError> {
        let get = |name: &'static str| {
            lookup(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let required = |name: &'static str| get(name).ok_or(ConfigError::Missing(name));

        let addr_text = get("FEEDWATCH_API_ADDR").unwrap_or_else(|| "0.0.0.0:8080".to_owned());
        let addr =
            addr_text
                .parse()
                .map_err(|e: std::net::AddrParseError| ConfigError::Invalid {
                    name: "FEEDWATCH_API_ADDR",
                    reason: e.to_string(),
                })?;

        let url = required("INFLUX_URL")?.trim_end_matches('/').to_owned();
        http_url("INFLUX_URL", &url)?;
        let allowed_origin = required("FEEDWATCH_ALLOWED_ORIGIN")?
            .trim_end_matches('/')
            .to_owned();
        http_url("FEEDWATCH_ALLOWED_ORIGIN", &allowed_origin)?;

        Ok(Config {
            addr,
            influx: InfluxConfig {
                url,
                org: required("INFLUX_ORG")?,
                bucket: required("INFLUX_BUCKET")?,
                token: required("INFLUX_TOKEN")?,
            },
            allowed_origin,
        })
    }
}

fn http_url(name: &'static str, value: &str) -> Result<(), ConfigError> {
    if value.starts_with("http://") || value.starts_with("https://") {
        Ok(())
    } else {
        Err(ConfigError::Invalid {
            name,
            reason: "must start with http:// or https://".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(overrides: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let mut vars = vec![
            ("INFLUX_URL", "http://influxdb:8086/"),
            ("INFLUX_ORG", "feedwatch"),
            ("INFLUX_BUCKET", "feedwatch"),
            ("INFLUX_TOKEN", "secret"),
            ("FEEDWATCH_ALLOWED_ORIGIN", "https://feedwatch.example/"),
        ];
        vars.extend_from_slice(overrides);
        let vars: Vec<(String, String)> = vars
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        move |name| {
            vars.iter()
                .rev()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn fr_fw_6_reads_valid_settings_with_defaults() {
        let config = Config::from_lookup(env(&[])).unwrap();
        assert_eq!(config.addr.to_string(), "0.0.0.0:8080");
        assert_eq!(config.influx.url, "http://influxdb:8086");
        assert_eq!(config.influx.token, "secret");
        assert_eq!(config.allowed_origin, "https://feedwatch.example");
    }

    #[test]
    fn fr_fw_6_rejects_missing_and_invalid_settings() {
        assert_eq!(
            Config::from_lookup(env(&[("INFLUX_TOKEN", " ")])),
            Err(ConfigError::Missing("INFLUX_TOKEN"))
        );
        assert!(matches!(
            Config::from_lookup(env(&[("INFLUX_URL", "influxdb:8086")])),
            Err(ConfigError::Invalid {
                name: "INFLUX_URL",
                ..
            })
        ));
        assert!(matches!(
            Config::from_lookup(env(&[("FEEDWATCH_API_ADDR", "nope")])),
            Err(ConfigError::Invalid {
                name: "FEEDWATCH_API_ADDR",
                ..
            })
        ));
        let err = Config::from_lookup(env(&[("FEEDWATCH_ALLOWED_ORIGIN", "*")])).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("FEEDWATCH_ALLOWED_ORIGIN is invalid")
        );
    }
}
