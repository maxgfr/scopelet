//! Pretty JSON that keeps the key order of the text it replaces.
//!
//! `serde_json::Value` sorts an object's keys. That is right for artifacts,
//! whose identity must not depend on how a source spelled its objects, and
//! wrong for a settings file a person wrote: installing a hook must not
//! reorder everything else in it.
use serde::de::{Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;
use std::collections::BTreeSet;

/// The shape of a JSON text: the order of each object's keys.
enum Order {
    Object(Vec<(String, Order)>),
    Array(Vec<Order>),
    Scalar,
}

impl<'de> Deserialize<'de> for Order {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Shape;
        impl<'de> Visitor<'de> for Shape {
            type Value = Order;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Order, A::Error> {
                let mut keys = Vec::new();
                while let Some(key) = map.next_key::<String>()? {
                    keys.push((key, map.next_value()?));
                }
                Ok(Order::Object(keys))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Order, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Order::Array(items))
            }
            fn visit_bool<E>(self, _: bool) -> Result<Order, E> {
                Ok(Order::Scalar)
            }
            fn visit_i64<E>(self, _: i64) -> Result<Order, E> {
                Ok(Order::Scalar)
            }
            fn visit_u64<E>(self, _: u64) -> Result<Order, E> {
                Ok(Order::Scalar)
            }
            fn visit_f64<E>(self, _: f64) -> Result<Order, E> {
                Ok(Order::Scalar)
            }
            fn visit_str<E>(self, _: &str) -> Result<Order, E> {
                Ok(Order::Scalar)
            }
            fn visit_unit<E>(self) -> Result<Order, E> {
                Ok(Order::Scalar)
            }
        }
        deserializer.deserialize_any(Shape)
    }
}

/// `value` as `serde_json::to_vec_pretty` writes it, except that each
/// object's keys follow the order they have in `original` (those it lacks
/// come after, sorted) and that a final newline of `original` is kept. An
/// `original` that is absent or not JSON gives the plain pretty form.
pub(crate) fn pretty_like(value: &Value, original: Option<&[u8]>) -> Vec<u8> {
    let order = original.and_then(|bytes| serde_json::from_slice::<Order>(bytes).ok());
    let mut out = String::new();
    write(&mut out, value, order.as_ref(), 0);
    if original.is_some_and(|bytes| bytes.ends_with(b"\n")) {
        out.push('\n');
    }
    out.into_bytes()
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write(out: &mut String, value: &Value, like: Option<&Order>, depth: usize) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            let known: &[(String, Order)] = match like {
                Some(Order::Object(keys)) => keys,
                _ => &[],
            };
            // The original's order first (a key it repeats counts once),
            // then the keys it did not have.
            let mut seen = BTreeSet::new();
            let mut keys: Vec<(&String, Option<&Order>)> = Vec::with_capacity(map.len());
            for (key, order) in known {
                if let Some((key, _)) = map.get_key_value(key)
                    && seen.insert(key)
                {
                    keys.push((key, Some(order)));
                }
            }
            keys.extend(
                map.keys()
                    .filter(|key| !seen.contains(key))
                    .map(|key| (key, None)),
            );
            out.push('{');
            for (i, (key, order)) in keys.into_iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                indent(out, depth + 1);
                out.push_str(&Value::String(key.clone()).to_string());
                out.push_str(": ");
                write(out, &map[key], order, depth + 1);
            }
            out.push('\n');
            indent(out, depth);
            out.push('}');
        }
        Value::Array(items) if !items.is_empty() => {
            let known: &[Order] = match like {
                Some(Order::Array(items)) => items,
                _ => &[],
            };
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                out.push_str(if i == 0 { "\n" } else { ",\n" });
                indent(out, depth + 1);
                write(out, item, known.get(i), depth + 1);
            }
            out.push('\n');
            indent(out, depth);
            out.push(']');
        }
        // Scalars, `{}` and `[]`: as serde_json writes them.
        scalar => out.push_str(&scalar.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn without_an_original_it_is_the_plain_pretty_form() {
        let value = json!({"b": [1, {"z": null, "a": 1.50}, []], "a": {}, "c": "é\n\"q\""});
        for original in [None, Some(&b"not json"[..])] {
            assert_eq!(
                pretty_like(&value, original),
                serde_json::to_vec_pretty(&value).unwrap()
            );
        }
    }

    #[test]
    fn keys_follow_the_original_at_every_depth_and_new_ones_come_last() {
        let original = br#"{"z": 1, "m": {"y": 1, "b": 2}, "a": [{"k": 1, "c": 2}], "z": 3}
"#;
        let value = json!({"a": [{"c": 2, "k": 1, "new": 0}, {"q": 1, "b": 2}], "added": true,
            "m": {"b": 2, "y": 1, "early": 0}, "z": 1});
        let text = String::from_utf8(pretty_like(&value, Some(original))).unwrap();
        assert_eq!(
            text,
            "{\n  \"z\": 1,\n  \"m\": {\n    \"y\": 1,\n    \"b\": 2,\n    \"early\": 0\n  },\n  \"a\": [\n    {\n      \"k\": 1,\n      \"c\": 2,\n      \"new\": 0\n    },\n    {\n      \"b\": 2,\n      \"q\": 1\n    }\n  ],\n  \"added\": true\n}\n"
        );
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
    }
}
