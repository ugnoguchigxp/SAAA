//! Reject ambiguous action envelopes instead of accepting the last duplicate key.
use serde::de::{Error, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value};

struct Action(Value);
impl<'de> Deserialize<'de> for Action {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Action;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("one JSON action object with unique keys")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut access: M) -> Result<Action, M::Error> {
                let mut object = Map::new();
                while let Some(key) = access.next_key::<String>()? {
                    if object.contains_key(&key) {
                        return Err(M::Error::custom("duplicate action field"));
                    }
                    object.insert(key, access.next_value::<Value>()?);
                }
                Ok(Action(Value::Object(object)))
            }
        }
        deserializer.deserialize_map(Object)
    }
}

pub(super) fn parse(body: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str::<Action>(body.trim()).map(|action| action.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_duplicate_actions_content_non_objects_and_trailing_objects() {
        for raw in [
            r#"{"action":"answer","content":"ok","action":"web_search"}"#,
            r#"{"action":"answer","content":"first","content":"second"}"#,
            "[]",
            r#"{"action":"answer"}{"action":"web_search"}"#,
        ] {
            assert!(parse(raw).is_err(), "{raw}");
        }
        assert_eq!(
            parse(r#"{"sources":[],"content":"ok","action":"answer"}"#).unwrap()["content"],
            "ok"
        );
    }
}
