use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn is_object(value: &Value) -> bool {
    value.is_object()
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn is_object_returns_true_for_objects() {
        assert!(is_object(&json!({"key": "value"})));
        assert!(is_object(&json!({})));
    }

    #[test]
    fn is_object_returns_false_for_non_objects() {
        assert!(!is_object(&json!("string")));
        assert!(!is_object(&json!(42)));
        assert!(!is_object(&json!([1, 2])));
        assert!(!is_object(&json!(null)));
        assert!(!is_object(&json!(true)));
    }
}
