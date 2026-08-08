//! Small JSON helpers for reading Pandora's responses.
//!
//! This lives here rather than in [`crate::demo`] because it is production code: the client
//! depends on it. `demo` is example-support and reads credentials from the environment, which
//! nothing shipped should ever pull in.

use serde_json::Value;

/// Find the first value under `key`, at any depth.
///
/// Pandora nests the same field at different depths across endpoints — `stations` is top-level on
/// one response and under `result` on another — and it changes without notice. Searching by key
/// rather than by path means a response reshuffle doesn't break us for no reason.
///
/// Returns the shallowest match, preferring a direct hit on the current object.
pub fn find_key<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    match value {
        Value::Object(map) => map
            .get(key)
            .or_else(|| map.values().find_map(|v| find_key(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_at_any_depth() {
        let value = json!({ "result": { "deep": { "stations": [1, 2] } } });
        assert_eq!(find_key(&value, "stations").unwrap().as_array().unwrap().len(), 2);
    }

    /// A direct hit must win over a deeper one, or a nested echo of the same key could shadow
    /// the value the caller actually meant.
    #[test]
    fn prefers_the_shallowest_match() {
        let value = json!({ "name": "outer", "inner": { "name": "inner" } });
        assert_eq!(find_key(&value, "name").unwrap(), "outer");
    }

    #[test]
    fn searches_through_arrays() {
        let value = json!([{ "a": 1 }, { "wanted": "yes" }]);
        assert_eq!(find_key(&value, "wanted").unwrap(), "yes");
    }

    #[test]
    fn missing_key_is_none() {
        assert!(find_key(&json!({ "a": 1 }), "b").is_none());
        assert!(find_key(&json!(null), "b").is_none());
    }
}
