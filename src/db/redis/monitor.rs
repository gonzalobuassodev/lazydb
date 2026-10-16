use std::collections::{BTreeMap, HashMap};

use crate::{db::monitor::MonitorSnapshot, model::dashboard::MetricKey};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RedisMonitorDetails {
    pub fields: BTreeMap<String, BTreeMap<String, String>>,
    pub keyspace: BTreeMap<u32, RedisKeyspaceDetails>,
    pub redis_version: Option<String>,
    pub redis_mode: Option<String>,
    pub os: Option<String>,
    pub arch_bits: Option<String>,
    pub process_id: Option<u32>,
    pub tcp_port: Option<u16>,
    pub uptime_in_seconds: Option<u64>,
    pub used_memory: Option<u64>,
    pub used_memory_peak: Option<u64>,
    pub used_memory_rss: Option<u64>,
    pub connected_clients: Option<u64>,
    pub max_clients: Option<u64>,
    pub instantaneous_ops_per_sec: Option<u64>,
    pub total_commands_processed: Option<u64>,
    pub total_net_input_bytes: Option<u64>,
    pub total_net_output_bytes: Option<u64>,
    pub evicted_keys: Option<u64>,
    pub expired_keys: Option<u64>,
    pub keyspace_hits: Option<u64>,
    pub keyspace_misses: Option<u64>,
    pub mem_fragmentation_ratio: Option<f64>,
    pub aof_enabled: Option<bool>,
    pub aof_rewrite_in_progress: Option<bool>,
    pub rdb_changes_since_last_save: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RedisKeyspaceDetails {
    pub keys: Option<u64>,
    pub expires: Option<u64>,
    pub avg_ttl: Option<u64>,
}

pub fn parse_info(raw: &str, at_millis: u64) -> (MonitorSnapshot, RedisMonitorDetails) {
    let mut details = RedisMonitorDetails::default();
    let mut section = String::from("default");
    let mut counters = HashMap::<String, f64>::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if let Some(name) = line.strip_prefix("# ") {
            section = name.trim().to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_owned();
        let value = value.trim().to_owned();
        details
            .fields
            .entry(section.clone())
            .or_default()
            .insert(key.clone(), value.clone());
        if let Some(database) = key.strip_prefix("db").and_then(|value| value.parse().ok()) {
            details.keyspace.insert(database, parse_keyspace(&value));
            continue;
        }
        if let Ok(number) = value.parse::<f64>()
            && number.is_finite()
        {
            counters.insert(key.clone(), number);
        }
        match key.as_str() {
            "redis_version" => details.redis_version = Some(value),
            "redis_mode" => details.redis_mode = Some(value),
            "os" => details.os = Some(value),
            "arch_bits" => details.arch_bits = Some(value),
            "process_id" => details.process_id = value.parse().ok(),
            "tcp_port" => details.tcp_port = value.parse().ok(),
            "uptime_in_seconds" => details.uptime_in_seconds = value.parse().ok(),
            "used_memory" => details.used_memory = value.parse().ok(),
            "used_memory_peak" => details.used_memory_peak = value.parse().ok(),
            "used_memory_rss" => details.used_memory_rss = value.parse().ok(),
            "connected_clients" => details.connected_clients = value.parse().ok(),
            "maxclients" => details.max_clients = value.parse().ok(),
            "instantaneous_ops_per_sec" => details.instantaneous_ops_per_sec = value.parse().ok(),
            "total_commands_processed" => details.total_commands_processed = value.parse().ok(),
            "total_net_input_bytes" => details.total_net_input_bytes = value.parse().ok(),
            "total_net_output_bytes" => details.total_net_output_bytes = value.parse().ok(),
            "evicted_keys" => details.evicted_keys = value.parse().ok(),
            "expired_keys" => details.expired_keys = value.parse().ok(),
            "keyspace_hits" => details.keyspace_hits = value.parse().ok(),
            "keyspace_misses" => details.keyspace_misses = value.parse().ok(),
            "mem_fragmentation_ratio" => details.mem_fragmentation_ratio = value.parse().ok(),
            "aof_enabled" => details.aof_enabled = Some(value == "1"),
            "aof_rewrite_in_progress" => details.aof_rewrite_in_progress = Some(value == "1"),
            "rdb_changes_since_last_save" => {
                details.rdb_changes_since_last_save = value.parse().ok()
            }
            _ => {}
        }
    }
    let mut values = BTreeMap::new();
    for (field, metric) in [
        ("instantaneous_ops_per_sec", MetricKey::RedisCommands),
        ("connected_clients", MetricKey::Connections),
        ("used_memory", MetricKey::RedisMemory),
        ("total_net_input_bytes", MetricKey::BytesRead),
        ("total_net_output_bytes", MetricKey::BytesWritten),
        ("evicted_keys", MetricKey::RedisEvictedKeys),
        ("expired_keys", MetricKey::RedisExpiredKeys),
        ("keyspace_hits", MetricKey::RedisKeyspaceHits),
        ("keyspace_misses", MetricKey::RedisKeyspaceMisses),
        ("uptime_in_seconds", MetricKey::ServerUptime),
    ] {
        if let Some(value) = counters.get(field) {
            values.insert(metric, *value);
        }
    }
    let keys = details
        .keyspace
        .values()
        .filter_map(|value| value.keys)
        .sum::<u64>();
    if !details.keyspace.is_empty() {
        values.insert(MetricKey::RedisKeys, keys as f64);
    }
    (
        MonitorSnapshot {
            server_time_millis: at_millis,
            server_generation: 1,
            values,
            redis_details: None,
            visibility: crate::db::monitor::MonitorVisibility::Unknown,
        },
        details,
    )
}

fn parse_keyspace(value: &str) -> RedisKeyspaceDetails {
    let mut result = RedisKeyspaceDetails::default();
    for item in value.split(',') {
        let Some((key, value)) = item.split_once('=') else {
            continue;
        };
        match key {
            "keys" => result.keys = value.parse().ok(),
            "expires" => result.expires = value.parse().ok(),
            "avg_ttl" => result.avg_ttl = value.parse().ok(),
            _ => {}
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::parse_info;
    use crate::model::dashboard::MetricKey;

    #[test]
    fn parses_server_stats_and_keyspace_without_losing_unknown_fields() {
        let (snapshot, details) = parse_info(
            "# Server\r\nredis_version:7.4.2\r\nuptime_in_seconds:42\r\n# Stats\r\ninstantaneous_ops_per_sec:12\r\ntotal_net_input_bytes:100\r\n# Keyspace\r\ndb2:keys=3,expires=1,avg_ttl=99\r\ncustom:value:with:colon\r\n",
            1000,
        );
        assert_eq!(details.redis_version.as_deref(), Some("7.4.2"));
        assert_eq!(details.keyspace[&2].keys, Some(3));
        assert_eq!(snapshot.values[&MetricKey::RedisCommands], 12.0);
        assert_eq!(details.fields["keyspace"]["custom"], "value:with:colon");
    }

    #[test]
    fn distinguishes_missing_values_from_real_zeroes_and_invalid_numbers() {
        let (snapshot, details) = parse_info(
            "# Stats\ninstantaneous_ops_per_sec:0\nconnected_clients:not-a-number\n# Keyspace\ndb0:keys=0,expires=0,avg_ttl=0\n",
            1000,
        );
        assert_eq!(snapshot.values[&MetricKey::RedisCommands], 0.0);
        assert!(!snapshot.values.contains_key(&MetricKey::Connections));
        assert_eq!(details.keyspace[&0].keys, Some(0));
        assert_eq!(details.keyspace[&0].expires, Some(0));
    }

    #[test]
    fn preserves_unknown_sections_and_ignores_malformed_lines() {
        let (_, details) = parse_info(
            "not a field\n# Custom\nunknown:value\nmalformed\n# Server\nredis_version:\n",
            1000,
        );
        assert_eq!(details.fields["custom"]["unknown"], "value");
        assert_eq!(details.redis_version.as_deref(), Some(""));
        assert!(!details.fields["custom"].contains_key("malformed"));
    }
}
