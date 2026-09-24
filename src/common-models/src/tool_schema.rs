use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};

pub trait ToolSchema {
    fn schema() -> Value;
}

macro_rules! primitive_schema {
    ($kind:literal: $($ty:ty),+ $(,)?) => {
        $(impl ToolSchema for $ty {
            fn schema() -> Value {
                json!({"type": $kind})
            }
        })+
    };
}

primitive_schema!("string": String, str);
primitive_schema!("boolean": bool);
primitive_schema!("integer": u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize);
primitive_schema!("number": f32, f64);

impl ToolSchema for Value {
    fn schema() -> Value {
        json!({})
    }
}

impl<T: ToolSchema> ToolSchema for Option<T> {
    fn schema() -> Value {
        nullable(T::schema())
    }
}

impl<T: ToolSchema> ToolSchema for Vec<T> {
    fn schema() -> Value {
        json!({"type": "array", "items": T::schema()})
    }
}

impl<T: ToolSchema> ToolSchema for BTreeMap<String, T> {
    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": T::schema()})
    }
}

impl<T: ToolSchema, S> ToolSchema for HashMap<String, T, S> {
    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": T::schema()})
    }
}

impl ToolSchema for serde_json::Map<String, Value> {
    fn schema() -> Value {
        json!({"type": "object", "additionalProperties": true})
    }
}

pub fn nullable(mut schema: Value) -> Value {
    match schema.get_mut("type") {
        Some(kind @ Value::String(_)) => *kind = json!([kind.clone(), "null"]),
        Some(Value::Array(kinds)) if !kinds.contains(&json!("null")) => kinds.push(json!("null")),
        _ => (),
    }
    match schema.get_mut("enum") {
        Some(Value::Array(values)) if !values.contains(&Value::Null) => values.push(Value::Null),
        _ => (),
    }
    match schema.get_mut("oneOf") {
        Some(Value::Array(variants)) if !variants.contains(&json!({"type": "null"})) => {
            variants.push(json!({"type": "null"}))
        }
        _ => (),
    }
    schema
}

pub fn tagged(mut schema: Value, tag: &str, name: &str, description: &str) -> Value {
    schema["properties"][tag] = json!({"type": "string", "enum": [name]});
    if !description.is_empty() {
        schema["properties"][tag]["description"] = json!(description);
    }
    schema["required"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!(tag));
    schema
}

pub fn input_schema(schema: Value, variants: &[&str], fields: &[&str]) -> Value {
    let mut schema = match schema.get("oneOf").and_then(Value::as_array) {
        Some(choices) => {
            let mut properties = serde_json::Map::new();
            let mut required: Option<Vec<Value>> = None;
            for choice in choices.iter().filter(|choice| {
                variants.is_empty()
                    || choice["required"][0].as_str().is_some_and(|tag| {
                        choice["properties"][tag]["enum"][0]
                            .as_str()
                            .is_some_and(|name| variants.contains(&name))
                    })
            }) {
                let names = choice["required"].as_array().cloned().unwrap_or_default();
                required = Some(match required {
                    None => names,
                    Some(previous) => previous
                        .into_iter()
                        .filter(|name| names.contains(name))
                        .collect(),
                });
                for (name, property) in choice["properties"].as_object().unwrap() {
                    match properties.get_mut(name) {
                        Some(existing) => *existing = merged(existing.clone(), property.clone()),
                        None => {
                            properties.insert(name.clone(), property.clone());
                        }
                    }
                }
            }
            json!({"type": "object", "properties": properties, "required": required.unwrap_or_default()})
        }
        None => schema,
    };
    if !fields.is_empty() {
        schema["properties"]
            .as_object_mut()
            .unwrap()
            .retain(|name, _| fields.contains(&name.as_str()));
        schema["required"]
            .as_array_mut()
            .unwrap()
            .retain(|name| name.as_str().is_some_and(|name| fields.contains(&name)));
    }
    schema
}

fn merged(left: Value, right: Value) -> Value {
    let mut left_shape = left.clone();
    let mut right_shape = right.clone();
    let left_values = left_shape
        .as_object_mut()
        .and_then(|shape| shape.remove("enum"));
    let right_values = right_shape
        .as_object_mut()
        .and_then(|shape| shape.remove("enum"));
    match (left_values, right_values) {
        _ if left == right => left,
        (Some(Value::Array(mut values)), Some(Value::Array(additions)))
            if left_shape == right_shape =>
        {
            for value in additions {
                if !values.contains(&value) {
                    values.push(value);
                }
            }
            left_shape["enum"] = Value::Array(values);
            left_shape
        }
        _ => json!({"anyOf": [left, right]}),
    }
}

#[cfg(test)]
#[path = "../tests/unit/tool_schema.rs"]
mod tests;
