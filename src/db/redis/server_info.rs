#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RedisServerIdentity {
    pub server_name: Option<String>,
    pub version: Option<String>,
    pub mode: Option<String>,
    pub is_valkey: bool,
}

impl RedisServerIdentity {
    pub fn display_name(&self) -> &'static str {
        if self.is_valkey { "Valkey" } else { "Redis" }
    }

    pub fn display_name_or(&self, fallback: &'static str) -> &'static str {
        if self.server_name.is_none() && self.version.is_none() {
            fallback
        } else {
            self.display_name()
        }
    }
}

pub fn parse(raw: &str) -> RedisServerIdentity {
    let mut identity = RedisServerIdentity::default();
    let mut valkey_version = None;
    let mut redis_version = None;
    let mut server_mode = None;
    let mut redis_mode = None;

    for line in raw.lines().map(str::trim) {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "server_name" => identity.server_name = Some(value.to_ascii_lowercase()),
            "valkey_version" => valkey_version = Some(value.to_owned()),
            "redis_version" => redis_version = Some(value.to_owned()),
            "server_mode" => server_mode = Some(value.to_owned()),
            "redis_mode" => redis_mode = Some(value.to_owned()),
            _ => {}
        }
    }

    let is_valkey = identity.server_name.as_deref() == Some("valkey") || valkey_version.is_some();
    identity.is_valkey = is_valkey;
    identity.version = if is_valkey {
        valkey_version
    } else {
        redis_version
    };
    identity.mode = server_mode.or(redis_mode);
    identity
}

#[cfg(test)]
mod tests {
    use super::{RedisServerIdentity, parse};

    #[test]
    fn parses_valkey_identity_without_confusing_redis_compatibility_version() {
        let identity = parse(
            "# Server\r\nredis_version:7.2.4\r\nserver_name:valkey\r\nvalkey_version:8.1.3\r\nredis_mode:standalone\r\nserver_mode:cluster\r\n",
        );
        assert_eq!(identity.display_name(), "Valkey");
        assert_eq!(identity.version.as_deref(), Some("8.1.3"));
        assert_eq!(identity.mode.as_deref(), Some("cluster"));
    }

    #[test]
    fn valkey_without_its_version_does_not_report_redis_compatibility_version() {
        let identity = parse("redis_version:7.2.4\nserver_name:valkey\n");
        assert_eq!(identity.display_name(), "Valkey");
        assert_eq!(identity.version, None);
    }

    #[test]
    fn valkey_version_identifies_valkey_when_server_name_is_missing() {
        let identity = parse("redis_version:7.2.4\nvalkey_version:8.1.3\n");
        assert_eq!(identity.display_name(), "Valkey");
        assert_eq!(identity.version.as_deref(), Some("8.1.3"));
    }

    #[test]
    fn redis_uses_redis_version_and_mode_and_missing_info_is_empty() {
        let redis = parse("redis_version:7.4.2\nredis_mode:standalone\n");
        assert_eq!(
            redis,
            RedisServerIdentity {
                server_name: None,
                version: Some("7.4.2".into()),
                mode: Some("standalone".into()),
                is_valkey: false,
            }
        );
        assert_eq!(parse("").version, None);
    }
}
