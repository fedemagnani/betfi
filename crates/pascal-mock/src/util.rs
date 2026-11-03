//! Small JSON helpers.

use serde_json::Value;

/// Wall clock, Unix ms (live mode's clock).
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Removes `null` members from every object, recursively. Pascal omits absent optional fields
/// rather than sending `null`; the `pascal-rs` models serialise `None` as `null`, so typed
/// responses go through this before they are served.
pub fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(strip_nulls).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_nested_nulls_only_from_objects() {
        let v = json!({"a": null, "b": {"c": null, "d": 1}, "e": [null, {"f": null}]});
        assert_eq!(strip_nulls(v), json!({"b": {"d": 1}, "e": [null, {}]}));
    }
}
