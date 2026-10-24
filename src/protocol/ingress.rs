//! Software admission policies, not verified robot or firmware maxima.

use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use std::fmt;

pub(crate) const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_BINARY_BYTES: usize = 1152 * 1024;
pub(crate) const MAX_BINARY_HEADER_BYTES: usize = 64 * 1024;
pub(crate) const MAX_BINARY_EXPANSION_BYTES: usize = 1024 * 1024;
const MAX_JSON_NODES: usize = 65_536;
const MAX_JSON_DEPTH: usize = 64;

/// Build an owned tree in one pass, checking each node/key before allocation.
/// A later parse or budget error can discard an already-built, bounded partial tree.
pub(crate) fn parse_json(data: &[u8]) -> Result<Value, &'static str> {
    if data.len() > MAX_TEXT_BYTES {
        return Err("Ingress JSON exceeds 1 MiB");
    }
    let mut remaining_nodes = MAX_JSON_NODES;
    let mut deserializer = serde_json::Deserializer::from_slice(data);
    JsonAdmission {
        remaining_nodes: &mut remaining_nodes,
        depth: 0,
    }
    .deserialize(&mut deserializer)
    .and_then(|value| deserializer.end().map(|()| value))
    .map_err(|_| "Invalid ingress JSON or resource budget exceeded")
}

struct JsonAdmission<'a> {
    remaining_nodes: &'a mut usize,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for JsonAdmission<'_> {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if *self.remaining_nodes == 0 {
            return Err(serde::de::Error::custom("JSON node budget exceeded"));
        }
        *self.remaining_nodes -= 1;
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for JsonAdmission<'_> {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }
    fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E>(self, value: f64) -> Result<Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .ok_or_else(|| E::custom("Nonfinite JSON number"))
    }
    fn visit_str<E>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }
    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        if self.depth >= MAX_JSON_DEPTH {
            return Err(serde::de::Error::custom("JSON depth budget exceeded"));
        }
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element_seed(JsonAdmission {
            remaining_nodes: self.remaining_nodes,
            depth: self.depth + 1,
        })? {
            values.push(value);
        }
        Ok(Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        if self.depth >= MAX_JSON_DEPTH {
            return Err(serde::de::Error::custom("JSON depth budget exceeded"));
        }
        let mut values = Map::new();
        while let Some(key) = map.next_key_seed(JsonKey {
            remaining_nodes: self.remaining_nodes,
        })? {
            let value = map.next_value_seed(JsonAdmission {
                remaining_nodes: self.remaining_nodes,
                depth: self.depth + 1,
            })?;
            values.insert(key, value);
        }
        Ok(Value::Object(values))
    }
}

struct JsonKey<'a> {
    remaining_nodes: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for JsonKey<'_> {
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> Result<String, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        if *self.remaining_nodes == 0 {
            return Err(serde::de::Error::custom("JSON node budget exceeded"));
        }
        *self.remaining_nodes -= 1;
        serde::Deserialize::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_values_match_literal_semantics_including_escaped_duplicate_keys() {
        for (input, expected) in [
            ("null", serde_json::json!(null)),
            ("true", serde_json::json!(true)),
            ("false", serde_json::json!(false)),
            (r#""plain""#, serde_json::json!("plain")),
            ("[]", serde_json::json!([])),
            ("{}", serde_json::json!({})),
            (
                r#" {"a\"key":[true,null,-1,"\uB85C\uBD07","\uD83D\uDE80","\\\n\t"],"k":0,"\u006b":{"last":[]}} "#,
                serde_json::json!({
                    "a\"key": [true, null, -1, "로봇", "🚀", "\\\n\t"],
                    "k": {"last": []},
                }),
            ),
        ] {
            assert_eq!(parse_json(input.as_bytes()).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn json_numbers_preserve_integer_precision_and_float_representation() {
        for (input, expected) in [
            ("-9223372036854775808", Value::from(i64::MIN)),
            ("9223372036854775807", Value::from(i64::MAX)),
            (
                "9223372036854775808",
                Value::from(9_223_372_036_854_775_808u64),
            ),
            ("18446744073709551615", Value::from(u64::MAX)),
            ("9007199254740993", Value::from(9_007_199_254_740_993u64)),
            ("0", Value::from(0u64)),
            ("1.25", Value::from(1.25)),
            ("1e2", Value::from(100.0)),
        ] {
            let parsed = parse_json(input.as_bytes()).unwrap();
            assert_eq!(parsed, expected, "{input}");
            assert_eq!(parsed.is_f64(), expected.is_f64(), "{input}");
        }
        for input in ["-0", "-0.0", "-0e0"] {
            let parsed = parse_json(input.as_bytes()).unwrap();
            assert!(parsed.is_f64(), "{input}");
            assert_eq!(parsed.as_f64().unwrap().to_bits(), (-0.0f64).to_bits());
        }
    }

    #[test]
    fn json_finite_numeric_fallback_matches_canonical_serde() {
        // Out-of-integer-range lexemes retain Serde's finite-f64 fallback.
        for input in [
            "18446744073709551616",
            "-9223372036854775809",
            "1.7976931348623157e308",
            "2.2250738585072014e-308",
            "5e-324",
            "1e-400",
            "1.2345678901234567890123456789",
        ] {
            let expected: Value = serde_json::from_str(input).unwrap();
            let parsed = parse_json(input.as_bytes()).unwrap();
            assert!(parsed.is_f64(), "{input}");
            assert_eq!(
                parsed.as_f64().unwrap().to_bits(),
                expected.as_f64().unwrap().to_bits(),
                "{input}",
            );
        }
    }

    #[test]
    fn json_rejects_nonfinite_and_invalid_numeric_lexemes() {
        for input in ["1e400", "-1e400", "NaN", "Infinity", "01", "+1", "1e", "1."] {
            assert!(parse_json(input.as_bytes()).is_err(), "{input}");
        }
    }

    #[test]
    fn json_output_owns_strings_and_keys_after_input_is_dropped() {
        let parsed = {
            let input = String::from(r#"{"plain":"text","\u006b":"\uB85C\uBD07"}"#);
            parse_json(input.as_bytes()).unwrap()
        };
        assert_eq!(parsed, serde_json::json!({"plain": "text", "k": "로봇"}));
    }

    #[test]
    fn json_duplicate_entries_count_discarded_keys_and_values_at_node_limit() {
        // Root + 32,766 key/value pairs + final key/array/element = 65,536 visits.
        let at_limit = format!("{{{}\"key\":[0]}}", "\"key\":0,".repeat(32_766));
        assert_eq!(
            parse_json(at_limit.as_bytes()).unwrap(),
            serde_json::json!({"key": [0]}),
        );
        let over_limit = format!("{{{}\"key\":[0,1]}}", "\"key\":0,".repeat(32_766));
        assert!(parse_json(over_limit.as_bytes()).is_err());
    }

    #[test]
    fn json_object_keys_consume_budget_before_their_values() {
        // Root array + 65,532 scalars + object/key/value = 65,536 visits.
        let at_limit = format!("[{}{{\"key\":0}}]", "0,".repeat(65_532));
        let parsed = parse_json(at_limit.as_bytes()).unwrap();
        let values = parsed.as_array().unwrap();
        assert_eq!(values.len(), 65_533);
        assert_eq!(values.last().unwrap(), &serde_json::json!({"key": 0}));
        let over_limit = format!("[{}{{\"key\":0}}]", "0,".repeat(65_533));
        assert!(parse_json(over_limit.as_bytes()).is_err());
    }

    #[test]
    fn json_depth_limit_counts_empty_arrays_and_objects_but_not_scalar_leaves() {
        for leaf in ["[]", "{}"] {
            let at_limit = format!("{}{}{}", "{\"k\":".repeat(63), leaf, "}".repeat(63));
            assert!(parse_json(at_limit.as_bytes()).is_ok(), "{leaf}");
            let over_limit = format!("[{at_limit}]");
            assert!(parse_json(over_limit.as_bytes()).is_err(), "{leaf}");
        }
    }

    #[test]
    fn json_rejects_malformed_or_trailing_data_without_echoing_peer_input() {
        for input in [
            b"".as_slice(),
            b" \n\t",
            b"null true",
            b"{}sentinel-private-key",
            br#"{"sentinel-private-key":"\uD800"}"#,
            br#"{"sentinel-private-key":"\x00"}"#,
            br#"{"sentinel-private-key":0,}"#,
            br#"{"sentinel-private-key":0"#,
            b"[0,]",
            b"[0",
            b"{0:1}",
            b"\"\xff\"",
        ] {
            let error = parse_json(input).unwrap_err();
            assert_eq!(error, "Invalid ingress JSON or resource budget exceeded");
            assert!(!error.contains("sentinel-private-key"));
        }
    }

    #[test]
    fn boundary_json_admission_preserves_escaped_values_and_exact_integers() {
        let parsed =
            parse_json(br#"{"a\"key":[true,null,-1,18446744073709551615,"\uB85C\uBD07"]}"#)
                .unwrap();
        assert_eq!(parsed["a\"key"][3].as_u64(), Some(u64::MAX));
        assert_eq!(parsed["a\"key"][4], "로봇");
    }

    #[test]
    fn boundary_json_admission_checks_exact_byte_limit() {
        let at_limit = format!("\"{}\"", "x".repeat(1024 * 1024 - 2));
        assert_eq!(
            parse_json(at_limit.as_bytes())
                .unwrap()
                .as_str()
                .unwrap()
                .len(),
            1024 * 1024 - 2
        );
        let over_limit = format!("{at_limit} ");
        assert!(parse_json(over_limit.as_bytes()).is_err());
    }

    #[test]
    fn boundary_json_admission_checks_exact_node_limit() {
        let at_limit = format!("[{}0]", "0,".repeat(65_534));
        assert_eq!(
            parse_json(at_limit.as_bytes())
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            65_535
        );
        let over_limit = format!("[{}0]", "0,".repeat(65_535));
        assert!(parse_json(over_limit.as_bytes()).is_err());
        // Duplicate object keys still consume parsing work, even if Value keeps only the last.
        let repeated_keys = format!("{{{}\"key\":0}}", "\"key\":0,".repeat(32_767));
        assert!(parse_json(repeated_keys.as_bytes()).is_err());
    }

    #[test]
    fn boundary_json_admission_checks_depth_and_rejects_malformed_trailing_input() {
        let at_limit = format!("{}null{}", "[".repeat(64), "]".repeat(64));
        assert!(parse_json(at_limit.as_bytes()).is_ok());
        let over_limit = format!("[{at_limit}]");
        assert!(parse_json(over_limit.as_bytes()).is_err());
        for input in [
            b"{} {}".as_slice(),
            br#"{"x":"sentinel-private-key",}"#,
            b"[1,]",
            b"\xff",
        ] {
            let error = parse_json(input).unwrap_err();
            assert!(!error.contains("sentinel-private-key"));
        }
    }
}
