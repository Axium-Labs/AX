use super::*;

#[test]
fn removed_providers_cannot_be_constructed_even_with_explicit_selection() {
    let root = std::env::temp_dir().join(format!("ax-removed-selection-{}", uuid::Uuid::new_v4()));
    let path = root.join("auth.json");
    let auth = model::AuthStorage::new(&path);
    for id in ["openai", "google-vertex", "amazon-bedrock", "openai-codex"] {
        auth.disable_provider(id).unwrap();
        let info = ModelInfo {
            id: "test-model".into(),
            display_name: "Test".into(),
            provider: id.into(),
            context_window: 128_000,
            max_output_tokens: None,
            reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
            supports_tools: true,
            endpoint: None,
        };
        let selection = selection_from_info(&info).unwrap();
        let Err(error) = crate::runtime::build_provider(&selection, &path) else {
            panic!("removed provider must not be constructed");
        };
        assert!(error.to_string().contains("was removed"));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn native_ids_select_the_native_adapter_and_keep_catalog_limits() {
    for id in [
        "anthropic",
        "google",
        "google-vertex",
        "amazon-bedrock",
        "azure-openai-responses",
        "radius",
    ] {
        let info = ModelInfo {
            id: "test-model".into(),
            display_name: "Test".into(),
            provider: id.into(),
            context_window: 200_000,
            max_output_tokens: Some(16_000),
            reasoning_efforts: Vec::new(),
            default_reasoning_effort: None,
            supports_tools: true,
            endpoint: None,
        };
        let selection = selection_from_info(&info).unwrap();
        assert!(matches!(selection.provider, ProviderKind::Native), "{id}");
        assert_eq!(selection.max_output_tokens, Some(16_000));
        assert_eq!(selection.provider_id, id);
    }
    for id in ["cloudflare-ai-gateway", "cloudflare-workers-ai"] {
        assert!(matches!(
            provider_kind_for(id).unwrap(),
            ProviderKind::Compatible
        ));
    }
}

#[test]
fn primary_factory_constructs_native_models_from_saved_keys_without_network() {
    let root = std::env::temp_dir().join(format!("ax-native-selection-{}", uuid::Uuid::new_v4()));
    let path = root.join("auth.json");
    let auth = model::AuthStorage::new(&path);
    for id in ["anthropic", "google", "radius"] {
        auth.store_api_key(id, "test-key").unwrap();
        let selection = selection_for_provider_arg(id, Some("test-model".into()), None).unwrap();
        let provider = crate::runtime::build_provider(&selection, &path).unwrap();
        assert_eq!(provider.name(), id);
        assert_eq!(provider.model_id(), "test-model");
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
}
