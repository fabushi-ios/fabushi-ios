use mahayana_host_runtime::extensions::cloud_agents::model_catalog_fetch::{
    AI_AVAILABLE_MODELS_PATH, SandModelCatalogParameterType, decode_available_models_response,
    encode_available_models_request,
};

fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 { byte |= 0x80; }
        out.push(byte);
        if value == 0 { return out; }
    }
}

fn key(field: u64, wire: u8) -> Vec<u8> {
    varint((field << 3) | u64::from(wire))
}

fn string_field(field: u64, value: &str) -> Vec<u8> {
    let mut out = key(field, 2);
    out.extend(varint(value.len() as u64));
    out.extend_from_slice(value.as_bytes());
    out
}

fn message_field(field: u64, value: Vec<u8>) -> Vec<u8> {
    let mut out = key(field, 2);
    out.extend(varint(value.len() as u64));
    out.extend(value);
    out
}

#[test]
fn request_is_the_frozen_user_available_model_catalog_request() {
    assert_eq!(AI_AVAILABLE_MODELS_PATH, "/aiserver.v1.AiService/AvailableModels");
    assert_eq!(
        encode_available_models_request(),
        vec![0x28, 0x01, 0x38, 0x01, 0x50, 0x01]
    );
}

#[test]
fn response_maps_names_aliases_parameter_definitions_and_variants() {
    let mut bool_value = Vec::new();
    bool_value.extend(string_field(1, "true"));
    bool_value.extend(string_field(2, "Enabled"));
    let boolean_definition = message_field(1, bool_value);

    let mut enum_value = Vec::new();
    enum_value.extend(string_field(1, "high"));
    enum_value.extend(string_field(2, "High"));
    let enum_definition = message_field(1, enum_value);

    let mut bool_type = Vec::new();
    bool_type.extend(message_field(1, boolean_definition));
    let mut enum_type = Vec::new();
    enum_type.extend(message_field(2, enum_definition));

    let mut p1 = Vec::new();
    p1.extend(string_field(1, "fast"));
    p1.extend(string_field(2, "Fast"));
    p1.extend(message_field(4, bool_type));

    let mut p2 = Vec::new();
    p2.extend(string_field(1, "thinking"));
    p2.extend(string_field(2, " Thinking "));
    p2.extend(message_field(4, enum_type));

    let mut variant_param = Vec::new();
    variant_param.extend(string_field(1, "thinking"));
    variant_param.extend(string_field(2, "high"));
    let variant = message_field(1, variant_param);

    let mut model = Vec::new();
    model.extend(string_field(1, "model-x"));
    model.extend(string_field(17, " Model X "));
    model.extend(message_field(29, p1));
    model.extend(message_field(29, p2));
    model.extend(message_field(30, variant));
    model.extend(string_field(37, "model-x-latest"));

    let response = message_field(2, model);
    let catalog = decode_available_models_response(&response).unwrap();

    assert_eq!(catalog.len(), 1);
    let entry = &catalog[0];
    assert_eq!(entry.id, "model-x");
    assert_eq!(entry.display_name.as_deref(), Some("Model X"));
    assert_eq!(entry.aliases, vec!["model-x-latest"]);
    assert_eq!(entry.params.len(), 2);
    assert_eq!(entry.params[0].id, "fast");
    assert_eq!(entry.params[0].parameter_type, SandModelCatalogParameterType::Boolean);
    assert_eq!(entry.params[0].values[0].value, "true");
    assert_eq!(entry.params[0].values[0].display_name.as_deref(), Some("Enabled"));
    assert_eq!(entry.params[1].parameter_type, SandModelCatalogParameterType::Enum);
    assert_eq!(entry.params[1].name.as_deref(), Some("Thinking"));
    assert_eq!(entry.variants[0][0].id, "thinking");
    assert_eq!(entry.variants[0][0].value, "high");
}

#[test]
fn response_filters_empty_model_names_and_parameters_without_values() {
    let mut empty_parameter = Vec::new();
    empty_parameter.extend(string_field(1, "empty"));
    empty_parameter.extend(message_field(4, message_field(2, Vec::new())));

    let mut empty_model = Vec::new();
    empty_model.extend(string_field(1, "   "));

    let mut valid_model = Vec::new();
    valid_model.extend(string_field(1, "usable"));
    valid_model.extend(message_field(29, empty_parameter));

    let mut response = Vec::new();
    response.extend(message_field(2, empty_model));
    response.extend(message_field(2, valid_model));

    let catalog = decode_available_models_response(&response).unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].id, "usable");
    assert!(catalog[0].params.is_empty());
}
