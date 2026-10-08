use sea_orm::{DatabaseConnection, TransactionTrait};
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use super::RelayError;
use crate::db::service::app_metadata_service::{get_value, upsert_value};

pub const AGENT_PATH: &str = "/__codeg_relay/v1/agent";
const FIELDS: [&str; 5] = ["enabled", "url", "key", "fingerprint", "public_url"];
const ENV: [&str; 5] = [
    "CODEG_RELAY_ENABLED",
    "CODEG_RELAY_URL",
    "CODEG_RELAY_KEY",
    "CODEG_RELAY_FINGERPRINT",
    "CODEG_RELAY_PUBLIC_URL",
];

// Deliberately no Debug/Serialize: the key must never escape through diagnostics.
#[derive(Clone, Default)]
pub struct RelayConfig {
    pub enabled: bool,
    pub url: String,
    pub key: String,
    pub fingerprint: Option<String>,
    pub public_url: Option<String>,
    pub from_env: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayConfigView {
    pub enabled: bool,
    pub url: String,
    pub key_set: bool,
    pub fingerprint: Option<String>,
    pub public_url: Option<String>,
    pub effective_public_url: Option<String>,
    pub source: &'static str,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayConfigUpdate {
    pub enabled: Option<bool>,
    pub url: Option<String>,
    pub key: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    pub fingerprint: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub public_url: Option<Option<String>>,
}

fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

impl RelayConfig {
    pub fn view(&self) -> RelayConfigView {
        RelayConfigView {
            enabled: self.enabled,
            url: self.url.clone(),
            key_set: !self.key.is_empty(),
            fingerprint: self.fingerprint.clone(),
            public_url: self.public_url.clone(),
            effective_public_url: self
                .public_url
                .clone()
                .or_else(|| (!self.url.is_empty()).then(|| self.url.clone())),
            source: if self.from_env { "env" } else { "settings" },
        }
    }

    pub fn agent_url(&self) -> Result<Url, RelayError> {
        let mut url = Url::parse(&self.url).map_err(|_| invalid("Invalid relay URL"))?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| invalid("Invalid relay URL scheme"))?;
        url.set_path(AGENT_PATH);
        Ok(url)
    }

    pub fn validate(&mut self) -> Result<(), RelayError> {
        if !self.url.trim().is_empty() {
            self.url = normalize_url(&self.url)?;
        } else {
            self.url.clear();
        }
        if !self.key.is_empty() {
            validate_key(&self.key)?;
        }
        self.fingerprint = self
            .fingerprint
            .as_deref()
            .map(normalize_fingerprint)
            .transpose()?;
        self.public_url = self
            .public_url
            .as_deref()
            .map(normalize_public_url)
            .transpose()?;
        if self.enabled && (self.url.is_empty() || self.key.is_empty()) {
            return Err(invalid("Enabled relay requires a URL and key"));
        }
        if self.fingerprint.is_some() && self.url.starts_with("http:") {
            return Err(invalid("A certificate fingerprint requires HTTPS"));
        }
        Ok(())
    }

    pub fn apply(&mut self, update: RelayConfigUpdate) -> Result<(), RelayError> {
        if self.from_env {
            return Err(RelayError::new(
                "relay_config_from_env",
                "Relay configuration is controlled by environment variables",
            ));
        }
        if let Some(v) = update.enabled {
            self.enabled = v;
        }
        if let Some(v) = update.url {
            self.url = v;
        }
        if let Some(v) = update.key {
            validate_key(&v)?;
            self.key = v;
        }
        if let Some(v) = update.fingerprint {
            self.fingerprint = v;
        }
        if let Some(v) = update.public_url {
            self.public_url = v;
        }
        self.validate()
    }
}

pub fn invalid(message: &'static str) -> RelayError {
    RelayError::new("invalid_input", message)
}

pub fn validate_key(key: &str) -> Result<(), RelayError> {
    if key.chars().count() < 16 || http::HeaderValue::from_str(&format!("Bearer {key}")).is_err() {
        return Err(invalid(
            "Relay key must contain at least 16 characters and be a valid bearer value",
        ));
    }
    Ok(())
}

pub fn normalize_fingerprint(input: &str) -> Result<String, RelayError> {
    let input = input.trim().to_ascii_lowercase();
    let input = input
        .strip_prefix("sha256/")
        .or_else(|| input.strip_prefix("sha256:"))
        .unwrap_or(&input);
    let canonical: String = input
        .chars()
        .filter(|c| *c != ':' && !c.is_ascii_whitespace())
        .collect();
    if canonical.len() != 64 || !canonical.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(invalid(
            "Certificate fingerprint must be 64 hexadecimal characters",
        ));
    }
    Ok(canonical)
}

fn parse_url(input: &str) -> Result<Url, RelayError> {
    let input = input.trim();
    let value = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let mut url = Url::parse(&value).map_err(|_| invalid("Invalid relay URL"))?;
    let scheme = match url.scheme() {
        "wss" | "https" => "https",
        "ws" | "http" => "http",
        _ => return Err(invalid("Relay URL must use HTTPS")),
    };
    url.set_scheme(scheme)
        .map_err(|_| invalid("Invalid relay scheme"))?;
    if url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err(invalid(
            "Relay URL must have a host and no credentials, query or fragment",
        ));
    }
    if scheme == "http"
        && !match url.host() {
            Some(url::Host::Domain("localhost")) => true,
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => {
                ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v| v.is_loopback())
            }
            _ => false,
        }
    {
        return Err(invalid("Plaintext relay URLs are allowed only on loopback"));
    }
    Ok(url)
}

pub fn normalize_url(input: &str) -> Result<String, RelayError> {
    let mut url = parse_url(input)?;
    if url.path() != "/" && url.path() != AGENT_PATH {
        return Err(invalid(
            "Relay URL path must be empty or the agent endpoint",
        ));
    }
    url.set_path("");
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn normalize_public_url(input: &str) -> Result<String, RelayError> {
    Ok(parse_url(input)?.as_str().trim_end_matches('/').to_string())
}

#[derive(Default)]
pub struct RelayEnv([Option<String>; 5]);

impl RelayEnv {
    pub fn is_present(&self) -> bool {
        self.0.iter().any(Option::is_some)
    }
    pub fn capture() -> Self {
        Self(ENV.map(|key| std::env::var(key).ok()))
    }
}

pub async fn load(conn: &DatabaseConnection) -> Result<RelayConfig, RelayError> {
    load_with_env(conn, &RelayEnv::capture()).await
}

pub async fn load_with_env(
    conn: &DatabaseConnection,
    env: &RelayEnv,
) -> Result<RelayConfig, RelayError> {
    let mut values = Vec::with_capacity(5);
    for (i, field) in FIELDS.iter().enumerate() {
        values.push(match &env.0[i] {
            Some(v) => Some(v.clone()),
            None => get_value(conn, &format!("web_relay_{field}"))
                .await
                .map_err(RelayError::database)?,
        });
    }
    let from_env = env.0.iter().any(Option::is_some);
    let value = |i: usize| values[i].clone().filter(|v| !v.is_empty());
    let enabled = if let Some(raw) =
        env.0[0]
            .as_ref()
            .or_else(|| if from_env { None } else { values[0].as_ref() })
    {
        match raw.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => true,
            "false" | "0" => false,
            _ => return Err(invalid("Relay enabled must be true or false")),
        }
    } else {
        from_env && value(1).is_some() && value(2).is_some()
    };
    let mut config = RelayConfig {
        enabled,
        url: value(1).unwrap_or_default(),
        key: value(2).unwrap_or_default(),
        fingerprint: value(3),
        public_url: value(4),
        from_env,
    };
    config.validate()?;
    Ok(config)
}

pub async fn save(conn: &DatabaseConnection, config: &RelayConfig) -> Result<(), RelayError> {
    let txn = conn.begin().await.map_err(|_| RelayError::database(()))?;
    let values = [
        config.enabled.to_string(),
        config.url.clone(),
        config.key.clone(),
        config.fingerprint.clone().unwrap_or_default(),
        config.public_url.clone().unwrap_or_default(),
    ];
    for (field, value) in FIELDS.iter().zip(values) {
        upsert_value(&txn, &format!("web_relay_{field}"), &value)
            .await
            .map_err(RelayError::database)?;
    }
    txn.commit().await.map_err(|_| RelayError::database(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_fingerprint_accepts_human_format_and_rejects_invalid() {
        let human = format!("SHA256: {}", ["AB"; 32].join(":"));
        assert_eq!(normalize_fingerprint(&human).unwrap(), "ab".repeat(32));
        assert_eq!(
            normalize_fingerprint(&format!("sha256/{}", "CD".repeat(32))).unwrap(),
            "cd".repeat(32)
        );
        for invalid in ["", "aa", &"gg".repeat(32), &"ab".repeat(33)] {
            assert!(normalize_fingerprint(invalid).is_err());
        }
    }

    #[test]
    fn urls_normalize_and_reject_unsafe_or_ambiguous_inputs() {
        for input in [
            "relay.test:18443",
            "https://relay.test:18443/",
            "wss://relay.test:18443/__codeg_relay/v1/agent",
        ] {
            assert_eq!(normalize_url(input).unwrap(), "https://relay.test:18443");
        }
        assert_eq!(
            normalize_url("ws://[::1]:18443").unwrap(),
            "http://[::1]:18443"
        );
        for input in [
            "http://example.com",
            "ws://192.168.1.1",
            "https://a:b@relay.test",
            "https://relay.test/?key=x",
            "https://relay.test/#x",
            "https://relay.test/other",
            "ftp://relay.test",
            "https://relay.test:0",
        ] {
            assert!(normalize_url(input).is_err(), "{input}");
        }
    }

    #[tokio::test]
    async fn null_clears_omitted_preserves_and_persistence_is_atomic() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        let mut config = RelayConfig {
            fingerprint: Some("ab".repeat(32)),
            public_url: Some("https://public.test".into()),
            ..Default::default()
        };
        save(&db.conn, &config).await.unwrap();
        config = load_with_env(&db.conn, &RelayEnv::default()).await.unwrap();
        config.apply(serde_json::from_str("{}").unwrap()).unwrap();
        assert_eq!(config.fingerprint, Some("ab".repeat(32)));
        config
            .apply(serde_json::from_str(r#"{"fingerprint":null,"publicUrl":null}"#).unwrap())
            .unwrap();
        save(&db.conn, &config).await.unwrap();
        let loaded = load_with_env(&db.conn, &RelayEnv::default()).await.unwrap();
        assert!(loaded.fingerprint.is_none());
        assert!(loaded.public_url.is_none());
    }

    #[tokio::test]
    async fn env_overrides_settings_and_refuses_updates() {
        let db = crate::db::test_helpers::fresh_in_memory_db().await;
        save(
            &db.conn,
            &RelayConfig {
                url: "https://saved.test".into(),
                key: "saved-key-12345678".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let env = RelayEnv([
            None,
            Some("wss://env.test".into()),
            Some("env-key-123456789".into()),
            None,
            None,
        ]);
        let mut config = load_with_env(&db.conn, &env).await.unwrap();
        assert!(config.enabled);
        assert_eq!(config.view().source, "env");
        assert_eq!(config.url, "https://env.test");
        assert_eq!(config.key, "env-key-123456789");
        assert_eq!(
            config.apply(RelayConfigUpdate::default()).unwrap_err().code,
            "relay_config_from_env"
        );
        let encoded = serde_json::to_string(&config.view()).unwrap();
        assert!(!encoded.contains("env-key"));
        let disabled = RelayEnv([Some("false".into()), None, None, None, None]);
        assert!(!load_with_env(&db.conn, &disabled).await.unwrap().enabled);
    }
}
