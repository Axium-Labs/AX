//! Chat Completions reasoning fields, including opaque gateway state.
use crate::ModelError;
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
pub(super) struct Reasoning {
    #[serde(rename = "reasoning_content")]
    pub content: Option<String>,
    #[serde(rename = "reasoning")]
    pub text: Option<String>,
    #[serde(rename = "reasoning_details")]
    pub details: Option<Vec<Value>>,
}

impl Reasoning {
    pub fn display_text(&self) -> Option<String> {
        self.content
            .clone()
            .or_else(|| self.text.clone())
            .or_else(|| {
                self.details
                    .as_ref()
                    .map(|details| {
                        details
                            .iter()
                            .filter_map(|detail| {
                                detail
                                    .get("text")
                                    .or_else(|| detail.get("summary"))
                                    .and_then(Value::as_str)
                            })
                            .collect::<String>()
                    })
                    .filter(|text| !text.is_empty())
            })
    }

    pub fn append(&mut self, delta: Self) -> Result<(), ModelError> {
        for (target, text) in [
            (&mut self.content, delta.content),
            (&mut self.text, delta.text),
        ] {
            if let Some(text) = text {
                target.get_or_insert_default().push_str(&text);
            }
        }
        if let Some(details) = delta.details {
            let saved = self.details.get_or_insert_default();
            for detail in details {
                merge_detail(saved, detail)?;
            }
        }
        Ok(())
    }
}

fn merge_detail(saved: &mut Vec<Value>, delta: Value) -> Result<(), ModelError> {
    let incoming = delta
        .as_object()
        .ok_or_else(|| ModelError::InvalidResponse("reasoning detail is not an object".into()))?;
    let existing = saved.iter_mut().find(|item| {
        delta
            .get("index")
            .and_then(Value::as_u64)
            .is_some_and(|index| item["index"] == index)
            || delta
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| item["id"] == id)
    });
    let Some(existing) = existing else {
        saved.push(delta);
        return Ok(());
    };
    for (field, value) in incoming {
        if value.is_null() {
            continue;
        }
        if matches!(field.as_str(), "text" | "summary" | "data" | "signature") {
            let text = value.as_str().ok_or_else(|| {
                ModelError::InvalidResponse(format!("reasoning detail {field} is not text"))
            })?;
            let previous = existing[field].as_str().unwrap_or_default();
            existing[field] = Value::String(format!("{previous}{text}"));
        } else if existing.get(field).is_none_or(Value::is_null) {
            existing[field] = value.clone();
        } else if existing[field] != *value {
            return Err(ModelError::InvalidResponse(format!(
                "reasoning detail changed identity field {field}"
            )));
        }
    }
    Ok(())
}
