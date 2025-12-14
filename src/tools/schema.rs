//! JSON schema helpers for tool definitions

use serde_json::{json, Value};

/// Build a JSON schema object
pub fn object_schema() -> SchemaBuilder {
    SchemaBuilder::new("object")
}

/// Build a string property
pub fn string_prop(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description
    })
}

/// Build an integer property
pub fn integer_prop(description: &str) -> Value {
    json!({
        "type": "integer",
        "description": description
    })
}

/// Build an array property
pub fn array_prop(description: &str, items: Value) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": items
    })
}

/// Schema builder for tool definitions
pub struct SchemaBuilder {
    schema_type: String,
    properties: serde_json::Map<String, Value>,
    required: Vec<String>,
}

impl SchemaBuilder {
    pub fn new(schema_type: &str) -> Self {
        Self {
            schema_type: schema_type.to_string(),
            properties: serde_json::Map::new(),
            required: Vec::new(),
        }
    }

    /// Add a property to the schema
    pub fn property(mut self, name: &str, schema: Value, required: bool) -> Self {
        self.properties.insert(name.to_string(), schema);
        if required {
            self.required.push(name.to_string());
        }
        self
    }

    /// Build the final schema
    pub fn build(self) -> Value {
        json!({
            "type": self.schema_type,
            "properties": self.properties,
            "required": self.required
        })
    }
}
