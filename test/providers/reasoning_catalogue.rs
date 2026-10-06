use model::{ModelInfo, ReasoningEffort};

#[test]
fn provider_efforts_keep_their_wire_values_through_catalogue_and_cli_parse() {
    for value in [
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
    ] {
        let effort = ReasoningEffort::parse(value).unwrap();
        assert_eq!(effort.to_string(), value);
        assert_eq!(serde_json::to_value(effort).unwrap(), value);
        assert_eq!(
            serde_json::from_str::<ReasoningEffort>(&format!("\"{value}\"")).unwrap(),
            effort
        );
    }
    assert!(ReasoningEffort::parse("fast").is_none());
}

#[test]
fn catalogue_does_not_expand_a_models_advertised_choices() {
    let model: ModelInfo = serde_json::from_value(serde_json::json!({
        "id":"vendor-model", "display_name":"Vendor", "provider":"openai",
        "context_window":32768,"supports_tools":true,
        "reasoning_efforts":["minimal","high","ultra"],"default_reasoning_effort":"high"
    }))
    .unwrap();
    let catalogue = serde_json::to_value(model).unwrap();
    assert_eq!(
        catalogue["reasoning_efforts"],
        serde_json::json!(["minimal", "high", "ultra"])
    );
    assert_eq!(catalogue["default_reasoning_effort"], "high");
}
