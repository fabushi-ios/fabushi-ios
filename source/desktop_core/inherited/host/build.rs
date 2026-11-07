use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
struct FieldDef {
    number: u64,
    is_map: bool,
    child_symbol: Option<String>,
    child_type: Option<String>,
    map_message: bool,
}

#[derive(Debug, Clone, Default)]
struct MessageDef {
    fields: Vec<FieldDef>,
}

#[derive(Debug, Clone)]
struct BlobField {
    proto_field_name: String,
    field_number: u64,
    blob_reference_type: String,
}

fn quoted_after(input: &str, marker: &str) -> Option<String> {
    let rest = input.get(input.find(marker)? + marker.len()..)?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn number_after(input: &str, marker: &str) -> Option<u64> {
    let rest = input.get(input.find(marker)? + marker.len()..)?.trim_start();
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn identifier_after(input: &str, marker: &str) -> Option<String> {
    let rest = input.get(input.find(marker)? + marker.len()..)?.trim_start();
    let ident: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect();
    (!ident.is_empty()).then_some(ident)
}

fn split_top_level_objects(input: &str) -> Vec<&str> {
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        match ch {
            '{' => {
                if depth == 0 {
                    start = Some(index);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    if let Some(start) = start.take() {
                        objects.push(&input[start..=index]);
                    }
                }
            }
            _ => {}
        }
    }
    objects
}

fn parse_message_definitions(
    content: &str,
    defs: &mut BTreeMap<String, MessageDef>,
    symbol_to_type: &mut BTreeMap<String, String>,
) {
    let type_marker = ".typeName = \"agent.v1.";
    let fields_marker = ").fields = proto3.util.newFieldList(() => [";
    let mut cursor = 0usize;
    while let Some(relative) = content[cursor..].find(type_marker) {
        let type_pos = cursor + relative;
        let line_start = content[..type_pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let line_end = content[type_pos..]
            .find('\n')
            .map(|idx| type_pos + idx)
            .unwrap_or(content.len());
        let line = &content[line_start..line_end];
        let symbol = line
            .trim_start()
            .strip_prefix('(')
            .and_then(|value| value.split(" as MutableMessageType").next())
            .expect("generated message typeName line must expose a symbol")
            .trim()
            .to_string();
        let type_name = quoted_after(line, ".typeName = ")
            .expect("generated message typeName must be quoted");
        if let Some(previous) = symbol_to_type.insert(symbol.clone(), type_name.clone()) {
            assert_eq!(
                previous, type_name,
                "generated protobuf symbol {symbol} mapped to multiple type names"
            );
        }

        let after = &content[line_end..];
        let symbol_marker = format!("({symbol} as MutableMessageType<");
        let symbol_fields = after
            .find(&symbol_marker)
            .expect("generated message fields must follow typeName");
        let fields_start = line_end + symbol_fields;
        let marker_pos = content[fields_start..]
            .find(fields_marker)
            .expect("generated message field list marker");
        let array_start = fields_start + marker_pos + fields_marker.len();
        let array_end = content[array_start..]
            .find("]);")
            .map(|idx| array_start + idx)
            .expect("generated message field list terminator");
        let array = &content[array_start..array_end];

        let mut fields = Vec::new();
        for object in split_top_level_objects(array) {
            let Some(number) = number_after(object, "no:") else {
                continue;
            };
            let kind = quoted_after(object, "kind:").unwrap_or_default();
            if kind == "message" {
                fields.push(FieldDef {
                    number,
                    is_map: false,
                    child_symbol: identifier_after(object, "T:"),
                    child_type: None,
                    map_message: false,
                });
            } else if kind == "map" {
                let map_value = object
                    .find("V:")
                    .and_then(|idx| object.get(idx + 2..))
                    .unwrap_or("");
                let map_message = quoted_after(map_value, "kind:")
                    .is_some_and(|value| value == "message");
                fields.push(FieldDef {
                    number,
                    is_map: true,
                    child_symbol: map_message.then(|| identifier_after(map_value, "T:")).flatten(),
                    child_type: None,
                    map_message,
                });
            } else {
                fields.push(FieldDef {
                    number,
                    is_map: false,
                    child_symbol: None,
                    child_type: None,
                    map_message: false,
                });
            }
        }
        defs.insert(type_name, MessageDef { fields });
        cursor = array_end + 3;
    }
}

fn parse_blob_metadata(content: &str) -> BTreeMap<String, Vec<BlobField>> {
    let section_end = content
        .find("export function getBlobReferenceMessageMetadata")
        .unwrap_or(content.len());
    let mut current_type: Option<String> = None;
    let mut metadata = BTreeMap::<String, Vec<BlobField>>::new();

    for line in content[..section_end].lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("\"agent.v1.") && trimmed.contains("\": {") {
            current_type = trimmed
                .strip_prefix('"')
                .and_then(|value| value.split("\": {").next())
                .map(str::to_string);
        }

        let Some(source_type) = current_type.as_ref() else {
            continue;
        };
        let mut remainder = line;
        while let Some(index) = remainder.find("{ protoFieldName:") {
            remainder = &remainder[index + 2..];
            let object_end = remainder.find('}').unwrap_or(remainder.len());
            let object = &remainder[..object_end];
            let Some(proto_field_name) = quoted_after(object, "protoFieldName:") else {
                break;
            };
            let Some(field_number) = number_after(object, "fieldNumber:") else {
                break;
            };
            let Some(blob_reference_type) = quoted_after(object, "blobReferenceType:") else {
                break;
            };
            metadata
                .entry(source_type.clone())
                .or_default()
                .push(BlobField {
                    proto_field_name,
                    field_number,
                    blob_reference_type,
                });
            remainder = &remainder[object_end..];
        }
    }
    metadata
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}


#[derive(Debug, Clone)]
enum CloudTraceGeneratedMapValueKind {
    Scalar(u64),
    Enum,
    Message {
        child_symbol: String,
        child_type: Option<String>,
    },
}

#[derive(Debug, Clone)]
enum CloudTraceGeneratedFieldKind {
    Scalar(u64),
    Enum,
    Message {
        child_symbol: String,
        child_type: Option<String>,
    },
    Map {
        key_scalar: u64,
        value: CloudTraceGeneratedMapValueKind,
    },
}

#[derive(Debug, Clone)]
struct CloudTraceGeneratedField {
    number: u64,
    proto_name: String,
    json_name: String,
    kind: CloudTraceGeneratedFieldKind,
    repeated: bool,
    optional: bool,
    oneof: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct CloudTraceGeneratedMessage {
    fields: Vec<CloudTraceGeneratedField>,
}

fn snake_to_lower_camel(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut uppercase_next = false;
    for ch in value.chars() {
        if ch == '_' {
            uppercase_next = true;
        } else if uppercase_next {
            output.extend(ch.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(ch);
        }
    }
    output
}

fn collect_generated_proto_ts_files(root: &std::path::Path, files: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(root)
        .unwrap_or_else(|error| panic!("read generated protobuf directory {}: {error}", root.display()))
        .map(|entry| entry.expect("read generated protobuf entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_generated_proto_ts_files(&path, files);
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("_pb.ts"))
        {
            files.push(path);
        }
    }
}

fn parse_cloud_trace_message_definitions(
    content: &str,
    defs: &mut BTreeMap<String, CloudTraceGeneratedMessage>,
    symbol_to_types: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let type_marker = ".typeName = \"";
    let fields_marker = ").fields = proto3.util.newFieldList(() => [";
    let mut cursor = 0usize;
    while let Some(relative) = content[cursor..].find(type_marker) {
        let type_pos = cursor + relative;
        let line_start = content[..type_pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let line_end = content[type_pos..]
            .find('\n')
            .map(|idx| type_pos + idx)
            .unwrap_or(content.len());
        let line = &content[line_start..line_end];
        let Some(symbol) = line
            .trim_start()
            .strip_prefix('(')
            .and_then(|value| value.split(" as MutableMessageType").next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            cursor = line_end;
            continue;
        };
        let Some(type_name) = quoted_after(line, ".typeName = ") else {
            cursor = line_end;
            continue;
        };
        symbol_to_types
            .entry(symbol.clone())
            .or_default()
            .insert(type_name.clone());

        let after = &content[line_end..];
        let symbol_marker = format!("({symbol} as MutableMessageType<");
        let Some(symbol_fields) = after.find(&symbol_marker) else {
            cursor = line_end;
            continue;
        };
        let fields_start = line_end + symbol_fields;
        let Some(marker_pos) = content[fields_start..].find(fields_marker) else {
            cursor = fields_start + symbol_marker.len();
            continue;
        };
        let array_start = fields_start + marker_pos + fields_marker.len();
        let Some(array_end_relative) = content[array_start..].find("]);") else {
            cursor = array_start;
            continue;
        };
        let array_end = array_start + array_end_relative;
        let array = &content[array_start..array_end];

        let mut fields = Vec::new();
        for object in split_top_level_objects(array) {
            let Some(number) = number_after(object, "no:") else {
                continue;
            };
            let Some(proto_name) = quoted_after(object, "name:") else {
                continue;
            };
            let kind_name = quoted_after(object, "kind:").unwrap_or_default();
            let kind = match kind_name.as_str() {
                "scalar" => {
                    let scalar = number_after(object, "T:")
                        .unwrap_or_else(|| panic!("missing scalar type for {type_name}.{proto_name}"));
                    CloudTraceGeneratedFieldKind::Scalar(scalar)
                }
                "enum" => CloudTraceGeneratedFieldKind::Enum,
                "message" => {
                    let child_symbol = identifier_after(object, "T:")
                        .unwrap_or_else(|| panic!("missing message symbol for {type_name}.{proto_name}"));
                    CloudTraceGeneratedFieldKind::Message {
                        child_symbol,
                        child_type: None,
                    }
                }
                "map" => {
                    let key_scalar = number_after(object, "K:")
                        .unwrap_or_else(|| panic!("missing map key type for {type_name}.{proto_name}"));
                    let value_source = object
                        .find("V:")
                        .and_then(|idx| object.get(idx + 2..))
                        .unwrap_or("");
                    let value_kind_name = quoted_after(value_source, "kind:").unwrap_or_default();
                    let value = match value_kind_name.as_str() {
                        "scalar" => CloudTraceGeneratedMapValueKind::Scalar(
                            number_after(value_source, "T:").unwrap_or_else(|| {
                                panic!("missing map scalar value type for {type_name}.{proto_name}")
                            }),
                        ),
                        "enum" => CloudTraceGeneratedMapValueKind::Enum,
                        "message" => CloudTraceGeneratedMapValueKind::Message {
                            child_symbol: identifier_after(value_source, "T:").unwrap_or_else(|| {
                                panic!("missing map message symbol for {type_name}.{proto_name}")
                            }),
                            child_type: None,
                        },
                        other => panic!(
                            "unsupported generated map value kind '{other}' for {type_name}.{proto_name}"
                        ),
                    };
                    CloudTraceGeneratedFieldKind::Map { key_scalar, value }
                }
                other => panic!(
                    "unsupported generated protobuf field kind '{other}' for {type_name}.{proto_name}"
                ),
            };
            fields.push(CloudTraceGeneratedField {
                number,
                json_name: snake_to_lower_camel(&proto_name),
                proto_name,
                kind,
                repeated: object.contains("repeated: true"),
                optional: object.contains("opt: true"),
                oneof: quoted_after(object, "oneof:"),
            });
        }
        defs.insert(type_name, CloudTraceGeneratedMessage { fields });
        cursor = array_end + 3;
    }
}

fn cloud_trace_namespace(type_name: &str) -> String {
    type_name
        .split('.')
        .take(2)
        .collect::<Vec<_>>()
        .join(".")
}

fn resolve_cloud_trace_symbol(
    parent_type: &str,
    symbol: &str,
    symbol_to_types: &BTreeMap<String, BTreeSet<String>>,
) -> Option<String> {
    let candidates = symbol_to_types.get(symbol)?;
    if candidates.len() == 1 {
        return candidates.iter().next().cloned();
    }
    let namespace = cloud_trace_namespace(parent_type);
    let prefix = format!("{namespace}.");
    let scoped = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    (scoped.len() == 1).then(|| scoped[0].clone())
}

fn cloud_trace_field_is_required(type_name: &str, field: &CloudTraceGeneratedField) -> bool {
    match type_name {
        "aiserver.v1.ConversationMessage" => matches!(field.number, 1 | 2 | 18 | 45),
        "aiserver.v1.ConversationMessage.ToolResult" => {
            matches!(field.number, 1 | 2 | 4 | 5 | 7 | 8 | 13 | 14)
        }
        "aiserver.v1.ConversationMessage.Thinking" => field.number == 1,
        "aiserver.v1.ClientSideToolV2Result" => field.oneof.as_deref() == Some("result"),
        _ => true,
    }
}

fn add_cloud_trace_well_known_types(
    defs: &mut BTreeMap<String, CloudTraceGeneratedMessage>,
    symbol_to_types: &mut BTreeMap<String, BTreeSet<String>>,
) {
    // @bufbuild/protobuf supplies these canonical proto3 well-known messages
    // outside the recovered *_pb.ts tree. Keep their standard protobuf schemas
    // explicit so generated aiserver.v1 fields such as CallMcpToolResult.result
    // remain fully decoded rather than becoming opaque fallbacks.
    let scalar = |number, proto_name: &str, scalar, oneof: Option<&str>| CloudTraceGeneratedField {
        number,
        proto_name: proto_name.to_string(),
        json_name: snake_to_lower_camel(proto_name),
        kind: CloudTraceGeneratedFieldKind::Scalar(scalar),
        repeated: false,
        optional: false,
        oneof: oneof.map(str::to_string),
    };
    let enum_field = |number, proto_name: &str, oneof: Option<&str>| CloudTraceGeneratedField {
        number,
        proto_name: proto_name.to_string(),
        json_name: snake_to_lower_camel(proto_name),
        kind: CloudTraceGeneratedFieldKind::Enum,
        repeated: false,
        optional: false,
        oneof: oneof.map(str::to_string),
    };
    let message = |number, proto_name: &str, child_symbol: &str, child_type: &str, repeated| CloudTraceGeneratedField {
        number,
        proto_name: proto_name.to_string(),
        json_name: snake_to_lower_camel(proto_name),
        kind: CloudTraceGeneratedFieldKind::Message {
            child_symbol: child_symbol.to_string(),
            child_type: Some(child_type.to_string()),
        },
        repeated,
        optional: false,
        oneof: None,
    };

    defs.insert(
        "google.protobuf.Struct".to_string(),
        CloudTraceGeneratedMessage {
            fields: vec![CloudTraceGeneratedField {
                number: 1,
                proto_name: "fields".to_string(),
                json_name: "fields".to_string(),
                kind: CloudTraceGeneratedFieldKind::Map {
                    key_scalar: 9,
                    value: CloudTraceGeneratedMapValueKind::Message {
                        child_symbol: "Value".to_string(),
                        child_type: Some("google.protobuf.Value".to_string()),
                    },
                },
                repeated: false,
                optional: false,
                oneof: None,
            }],
        },
    );
    defs.insert(
        "google.protobuf.Value".to_string(),
        CloudTraceGeneratedMessage {
            fields: vec![
                enum_field(1, "null_value", Some("kind")),
                scalar(2, "number_value", 1, Some("kind")),
                scalar(3, "string_value", 9, Some("kind")),
                scalar(4, "bool_value", 8, Some("kind")),
                CloudTraceGeneratedField {
                    number: 5,
                    proto_name: "struct_value".to_string(),
                    json_name: "structValue".to_string(),
                    kind: CloudTraceGeneratedFieldKind::Message {
                        child_symbol: "Struct".to_string(),
                        child_type: Some("google.protobuf.Struct".to_string()),
                    },
                    repeated: false,
                    optional: false,
                    oneof: Some("kind".to_string()),
                },
                CloudTraceGeneratedField {
                    number: 6,
                    proto_name: "list_value".to_string(),
                    json_name: "listValue".to_string(),
                    kind: CloudTraceGeneratedFieldKind::Message {
                        child_symbol: "ListValue".to_string(),
                        child_type: Some("google.protobuf.ListValue".to_string()),
                    },
                    repeated: false,
                    optional: false,
                    oneof: Some("kind".to_string()),
                },
            ],
        },
    );
    defs.insert(
        "google.protobuf.ListValue".to_string(),
        CloudTraceGeneratedMessage {
            fields: vec![message(
                1,
                "values",
                "Value",
                "google.protobuf.Value",
                true,
            )],
        },
    );
    defs.insert(
        "google.protobuf.Timestamp".to_string(),
        CloudTraceGeneratedMessage {
            fields: vec![
                scalar(1, "seconds", 3, None),
                scalar(2, "nanos", 5, None),
            ],
        },
    );

    for (symbol, type_name) in [
        ("Struct", "google.protobuf.Struct"),
        ("Value", "google.protobuf.Value"),
        ("ListValue", "google.protobuf.ListValue"),
        ("Timestamp", "google.protobuf.Timestamp"),
    ] {
        symbol_to_types
            .entry(symbol.to_string())
            .or_default()
            .insert(type_name.to_string());
    }
}

fn generate_cloud_agent_trace_schema(manifest_dir: &std::path::Path) {
    let proto_root = manifest_dir.join("../packages/proto/generated");
    println!("cargo:rerun-if-changed={}", proto_root.display());

    let mut files = Vec::new();
    collect_generated_proto_ts_files(&proto_root, &mut files);

    let mut defs = BTreeMap::<String, CloudTraceGeneratedMessage>::new();
    let mut symbol_to_types = BTreeMap::<String, BTreeSet<String>>::new();
    for path in &files {
        let content = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read generated protobuf TypeScript {}: {error}", path.display()));
        parse_cloud_trace_message_definitions(&content, &mut defs, &mut symbol_to_types);
    }
    add_cloud_trace_well_known_types(&mut defs, &mut symbol_to_types);

    let parent_names = defs.keys().cloned().collect::<Vec<_>>();
    for parent_type in parent_names {
        let Some(message) = defs.get_mut(&parent_type) else {
            continue;
        };
        for field in &mut message.fields {
            match &mut field.kind {
                CloudTraceGeneratedFieldKind::Message {
                    child_symbol,
                    child_type,
                } => {
                    *child_type =
                        resolve_cloud_trace_symbol(&parent_type, child_symbol, &symbol_to_types);
                }
                CloudTraceGeneratedFieldKind::Map {
                    value:
                        CloudTraceGeneratedMapValueKind::Message {
                            child_symbol,
                            child_type,
                        },
                    ..
                } => {
                    *child_type =
                        resolve_cloud_trace_symbol(&parent_type, child_symbol, &symbol_to_types);
                }
                _ => {}
            }
        }
    }

    let roots = [
        "aiserver.v1.ConversationMessage",
        "aiserver.v1.ConversationMessage.ToolResult",
        "aiserver.v1.ConversationMessage.Thinking",
        "aiserver.v1.ClientSideToolV2Result",
    ];
    for root in roots {
        assert!(
            defs.contains_key(root),
            "canonical generated protobuf root {root} is missing"
        );
    }

    let mut reachable = BTreeSet::<String>::new();
    let mut queue = VecDeque::<String>::from(
        roots.into_iter().map(str::to_string).collect::<Vec<_>>(),
    );
    while let Some(type_name) = queue.pop_front() {
        if !reachable.insert(type_name.clone()) {
            continue;
        }
        let message = defs
            .get(&type_name)
            .unwrap_or_else(|| panic!("generated protobuf descriptor missing for {type_name}"));
        for field in message
            .fields
            .iter()
            .filter(|field| cloud_trace_field_is_required(&type_name, field))
        {
            match &field.kind {
                CloudTraceGeneratedFieldKind::Message {
                    child_symbol,
                    child_type,
                } => {
                    let child_type = child_type.as_ref().unwrap_or_else(|| {
                        panic!(
                            "could not resolve generated protobuf message symbol {child_symbol} for {type_name}.{}",
                            field.proto_name
                        )
                    });
                    if !reachable.contains(child_type) {
                        queue.push_back(child_type.clone());
                    }
                }
                CloudTraceGeneratedFieldKind::Map {
                    value:
                        CloudTraceGeneratedMapValueKind::Message {
                            child_symbol,
                            child_type,
                        },
                    ..
                } => {
                    let child_type = child_type.as_ref().unwrap_or_else(|| {
                        panic!(
                            "could not resolve generated protobuf map value symbol {child_symbol} for {type_name}.{}",
                            field.proto_name
                        )
                    });
                    if !reachable.contains(child_type) {
                        queue.push_back(child_type.clone());
                    }
                }
                _ => {}
            }
        }
    }

    let mut output = String::new();
    output.push_str("// @generated by source/host/build.rs from frozen generated protobuf TypeScript.\n");
    output.push_str("// CloudAgent transcript conversion must consume this schema; do not hand-edit.\n\n");
    output.push_str(
        "fn cloud_agent_proto_message_descriptor(type_name: &str) -> Option<CloudAgentProtoMessageDescriptor> {\n    match type_name {\n",
    );
    for type_name in &reachable {
        let message = defs
            .get(type_name)
            .unwrap_or_else(|| panic!("reachable generated protobuf descriptor missing for {type_name}"));
        output.push_str("        ");
        output.push_str(&rust_string(type_name));
        output.push_str(" => Some(CloudAgentProtoMessageDescriptor { fields: &[\n");
        for field in message
            .fields
            .iter()
            .filter(|field| cloud_trace_field_is_required(type_name, field))
        {
            output.push_str("            CloudAgentProtoFieldDescriptor { number: ");
            output.push_str(&field.number.to_string());
            output.push_str(", proto_name: ");
            output.push_str(&rust_string(&field.proto_name));
            output.push_str(", json_name: ");
            output.push_str(&rust_string(&field.json_name));
            output.push_str(", kind: ");
            match &field.kind {
                CloudTraceGeneratedFieldKind::Scalar(scalar) => {
                    output.push_str("CloudAgentProtoFieldKind::Scalar(");
                    output.push_str(&scalar.to_string());
                    output.push(')');
                }
                CloudTraceGeneratedFieldKind::Enum => {
                    output.push_str("CloudAgentProtoFieldKind::Enum");
                }
                CloudTraceGeneratedFieldKind::Message {
                    child_type: Some(child_type),
                    ..
                } => {
                    output.push_str("CloudAgentProtoFieldKind::Message(");
                    output.push_str(&rust_string(child_type));
                    output.push(')');
                }
                CloudTraceGeneratedFieldKind::Message { .. } => {
                    panic!(
                        "unresolved generated protobuf message for {type_name}.{}",
                        field.proto_name
                    );
                }
                CloudTraceGeneratedFieldKind::Map { key_scalar, value } => {
                    output.push_str("CloudAgentProtoFieldKind::Map { key_scalar: ");
                    output.push_str(&key_scalar.to_string());
                    output.push_str(", value: ");
                    match value {
                        CloudTraceGeneratedMapValueKind::Scalar(scalar) => {
                            output.push_str("CloudAgentProtoMapValueKind::Scalar(");
                            output.push_str(&scalar.to_string());
                            output.push(')');
                        }
                        CloudTraceGeneratedMapValueKind::Enum => {
                            output.push_str("CloudAgentProtoMapValueKind::Enum");
                        }
                        CloudTraceGeneratedMapValueKind::Message {
                            child_type: Some(child_type),
                            ..
                        } => {
                            output.push_str("CloudAgentProtoMapValueKind::Message(");
                            output.push_str(&rust_string(child_type));
                            output.push(')');
                        }
                        CloudTraceGeneratedMapValueKind::Message { .. } => {
                            panic!(
                                "unresolved generated protobuf map message for {type_name}.{}",
                                field.proto_name
                            );
                        }
                    }
                    output.push_str(" }");
                }
            }
            output.push_str(", repeated: ");
            output.push_str(if field.repeated { "true" } else { "false" });
            output.push_str(", optional: ");
            output.push_str(if field.optional { "true" } else { "false" });
            output.push_str(", oneof: ");
            match field.oneof.as_deref() {
                Some(oneof) => {
                    output.push_str("Some(");
                    output.push_str(&rust_string(oneof));
                    output.push(')');
                }
                None => output.push_str("None"),
            }
            output.push_str(" },\n");
        }
        output.push_str("        ] }),\n");
    }
    output.push_str("        _ => None,\n    }\n}\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("cloud_agent_trace_schema.rs"), output)
        .expect("write generated CloudAgent trace schema");
}



fn generate_client_side_tool_v2_schema(manifest_dir: &std::path::Path) {
    let proto_root = manifest_dir.join("../packages/proto/generated");
    println!("cargo:rerun-if-changed={}", proto_root.display());

    let mut files = Vec::new();
    collect_generated_proto_ts_files(&proto_root, &mut files);

    let mut defs = BTreeMap::<String, CloudTraceGeneratedMessage>::new();
    let mut symbol_to_types = BTreeMap::<String, BTreeSet<String>>::new();
    for path in &files {
        let content = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read generated protobuf TypeScript {}: {error}", path.display()));
        parse_cloud_trace_message_definitions(&content, &mut defs, &mut symbol_to_types);
    }
    add_cloud_trace_well_known_types(&mut defs, &mut symbol_to_types);

    let parent_names = defs.keys().cloned().collect::<Vec<_>>();
    for parent_type in parent_names {
        let Some(message) = defs.get_mut(&parent_type) else { continue };
        for field in &mut message.fields {
            match &mut field.kind {
                CloudTraceGeneratedFieldKind::Message { child_symbol, child_type } => {
                    *child_type = resolve_cloud_trace_symbol(&parent_type, child_symbol, &symbol_to_types);
                }
                CloudTraceGeneratedFieldKind::Map {
                    value: CloudTraceGeneratedMapValueKind::Message { child_symbol, child_type },
                    ..
                } => {
                    *child_type = resolve_cloud_trace_symbol(&parent_type, child_symbol, &symbol_to_types);
                }
                _ => {}
            }
        }
    }

    let roots = [
        "aiserver.v1.ClientSideToolV2Call",
        "aiserver.v1.ClientSideToolV2Result",
    ];
    for root in roots {
        assert!(defs.contains_key(root), "canonical generated protobuf root {root} is missing");
    }

    let mut reachable = BTreeSet::<String>::new();
    let mut queue = VecDeque::<String>::from(
        roots.into_iter().map(str::to_string).collect::<Vec<_>>(),
    );
    while let Some(type_name) = queue.pop_front() {
        if !reachable.insert(type_name.clone()) { continue; }
        let message = defs.get(&type_name)
            .unwrap_or_else(|| panic!("generated protobuf descriptor missing for {type_name}"));
        for field in &message.fields {
            match &field.kind {
                CloudTraceGeneratedFieldKind::Message { child_symbol, child_type } => {
                    let child_type = child_type.as_ref().unwrap_or_else(|| {
                        panic!("could not resolve generated protobuf message symbol {child_symbol} for {type_name}.{}", field.proto_name)
                    });
                    if !reachable.contains(child_type) { queue.push_back(child_type.clone()); }
                }
                CloudTraceGeneratedFieldKind::Map {
                    value: CloudTraceGeneratedMapValueKind::Message { child_symbol, child_type },
                    ..
                } => {
                    let child_type = child_type.as_ref().unwrap_or_else(|| {
                        panic!("could not resolve generated protobuf map value symbol {child_symbol} for {type_name}.{}", field.proto_name)
                    });
                    if !reachable.contains(child_type) { queue.push_back(child_type.clone()); }
                }
                _ => {}
            }
        }
    }

    let mut output = String::new();
    output.push_str("// @generated by source/host/build.rs from frozen generated protobuf TypeScript.\n");
    output.push_str("// Client-side Tool V2 projection consumes this schema; do not hand-edit.\n\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("enum ClientToolProtoMapValueKind { Scalar(u64), Enum, Message(&'static str) }\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("enum ClientToolProtoFieldKind { Scalar(u64), Enum, Message(&'static str), Map { key_scalar: u64, value: ClientToolProtoMapValueKind } }\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct ClientToolProtoFieldDescriptor { number: u64, json_name: &'static str, kind: ClientToolProtoFieldKind, repeated: bool, optional: bool, oneof: Option<&'static str> }\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct ClientToolProtoMessageDescriptor { fields: &'static [ClientToolProtoFieldDescriptor] }\n\n");
    output.push_str("fn client_side_tool_proto_message_descriptor(type_name: &str) -> Option<ClientToolProtoMessageDescriptor> {\n    match type_name {\n");

    for type_name in &reachable {
        let message = defs.get(type_name)
            .unwrap_or_else(|| panic!("reachable generated protobuf descriptor missing for {type_name}"));
        output.push_str("        ");
        output.push_str(&rust_string(type_name));
        output.push_str(" => Some(ClientToolProtoMessageDescriptor { fields: &[\n");
        for field in &message.fields {
            output.push_str("            ClientToolProtoFieldDescriptor { number: ");
            output.push_str(&field.number.to_string());
            output.push_str(", json_name: ");
            output.push_str(&rust_string(&field.json_name));
            output.push_str(", kind: ");
            match &field.kind {
                CloudTraceGeneratedFieldKind::Scalar(scalar) => {
                    output.push_str("ClientToolProtoFieldKind::Scalar(");
                    output.push_str(&scalar.to_string());
                    output.push(')');
                }
                CloudTraceGeneratedFieldKind::Enum => output.push_str("ClientToolProtoFieldKind::Enum"),
                CloudTraceGeneratedFieldKind::Message { child_type: Some(child_type), .. } => {
                    output.push_str("ClientToolProtoFieldKind::Message(");
                    output.push_str(&rust_string(child_type));
                    output.push(')');
                }
                CloudTraceGeneratedFieldKind::Message { .. } => {
                    panic!("unresolved generated protobuf message for {type_name}.{}", field.proto_name)
                }
                CloudTraceGeneratedFieldKind::Map { key_scalar, value } => {
                    output.push_str("ClientToolProtoFieldKind::Map { key_scalar: ");
                    output.push_str(&key_scalar.to_string());
                    output.push_str(", value: ");
                    match value {
                        CloudTraceGeneratedMapValueKind::Scalar(scalar) => {
                            output.push_str("ClientToolProtoMapValueKind::Scalar(");
                            output.push_str(&scalar.to_string());
                            output.push(')');
                        }
                        CloudTraceGeneratedMapValueKind::Enum => output.push_str("ClientToolProtoMapValueKind::Enum"),
                        CloudTraceGeneratedMapValueKind::Message { child_type: Some(child_type), .. } => {
                            output.push_str("ClientToolProtoMapValueKind::Message(");
                            output.push_str(&rust_string(child_type));
                            output.push(')');
                        }
                        CloudTraceGeneratedMapValueKind::Message { .. } => {
                            panic!("unresolved generated protobuf map message for {type_name}.{}", field.proto_name)
                        }
                    }
                    output.push_str(" }");
                }
            }
            output.push_str(", repeated: ");
            output.push_str(if field.repeated { "true" } else { "false" });
            output.push_str(", optional: ");
            output.push_str(if field.optional { "true" } else { "false" });
            output.push_str(", oneof: ");
            match field.oneof.as_deref() {
                Some(oneof) => {
                    output.push_str("Some(");
                    output.push_str(&rust_string(oneof));
                    output.push(')');
                }
                None => output.push_str("None"),
            }
            output.push_str(" },\n");
        }
        output.push_str("        ] }),\n");
    }
    output.push_str("        _ => None,\n    }\n}\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("client_side_tool_v2_schema.rs"), output)
        .expect("write generated ClientSideToolV2 schema");
}

fn verify_local_exec_generated_source(path: &std::path::Path, markers: &[&str]) {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read frozen local-exec generated source {}: {error}", path.display()));
    for marker in markers {
        assert!(
            content.contains(marker),
            "frozen local-exec generated source {} is missing provenance/schema marker {marker:?}",
            path.display()
        );
    }
}

fn generate_agent_v1_local_exec_bindings(manifest_dir: &std::path::Path) {
    let generated_root = manifest_dir.join("../packages/proto/generated/agent/v1");
    let exec_source = generated_root.join("exec_pb.ts");
    let shell_source = generated_root.join("shell_exec_pb.ts");
    let read_source = generated_root.join("read_exec_pb.ts");

    for source in [&exec_source, &shell_source, &read_source] {
        println!("cargo:rerun-if-changed={}", source.display());
    }

    verify_local_exec_generated_source(
        &exec_source,
        &[
            "Region SHA-256: fd81fa7df18ed8c8bcd43cadaad2e893a6901b64ebcfd2f95551a582a543722a",
            "typeName = \"agent.v1.ExecClientControlMessage\"",
            "typeName = \"agent.v1.ExecClientMessage\"",
            "name: \"adopt_result\"",
            "typeName = \"agent.v1.ExecServerMessage\"",
            "name: \"adopt_args\"",
        ],
    );
    verify_local_exec_generated_source(
        &shell_source,
        &[
            "Region SHA-256: 7967a913a5fd54239f9de4f4294f8901b37b973393a73f559443bc04f418f5aa",
            "typeName = \"agent.v1.ShellArgs\"",
            "name: \"admin_command_denylist\"",
            "typeName = \"agent.v1.ShellStream\"",
            "name: \"sandbox_unsupported\"",
        ],
    );
    verify_local_exec_generated_source(
        &read_source,
        &[
            "Region SHA-256: 02569af827c77335892b43c8295700a6f03fc6dd3a920c329c157ae4e487a1f9",
            "typeName = \"agent.v1.ReadArgs\"",
            "typeName = \"agent.v1.ReadResult\"",
            "name: \"invalid_file\"",
        ],
    );

    let proto_root = manifest_dir.join("proto");
    let proto_file = proto_root.join("agent/v1/local_exec.proto");
    println!("cargo:rerun-if-changed={}", proto_file.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let descriptor_path = out_dir.join("agent_v1_local_exec_descriptor.bin");
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    let protoc_include = protoc_bin_vendored::include_path().expect("vendored protoc includes");

    let mut config = prost_build::Config::new();
    config
        .protoc_executable(protoc)
        .file_descriptor_set_path(&descriptor_path)
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types");
    config
        .compile_protos(&[proto_file], &[proto_root, protoc_include])
        .expect("generate canonical agent.v1 local-exec prost bindings");

    let descriptor_set =
        fs::read(&descriptor_path).expect("read canonical agent.v1 local-exec descriptor set");
    let mut json = pbjson_build::Builder::new();
    json.register_descriptors(&descriptor_set)
        .expect("register canonical agent.v1 local-exec descriptors");
    json.ignore_unknown_fields();
    json.build(&[".agent.v1"])
        .expect("generate protobuf-JSON serde for canonical agent.v1 local-exec bindings");
}


fn signed_number_after(input: &str, marker: &str) -> Option<i64> {
    let rest = input.get(input.find(marker)? + marker.len()..)?.trim_start();
    let value: String = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
        .collect();
    (!value.is_empty()).then(|| value.parse().ok()).flatten()
}

fn parse_generated_enum_definitions(
    content: &str,
    symbol_to_types: &mut BTreeMap<String, BTreeSet<String>>,
    values_by_type: &mut BTreeMap<String, Vec<(i64, String)>>,
) {
    let marker = "proto3.util.setEnumType(";
    let mut cursor = 0usize;
    while let Some(relative) = content[cursor..].find(marker) {
        let start = cursor + relative + marker.len();
        let Some(comma_relative) = content[start..].find(',') else { break };
        let comma = start + comma_relative;
        let symbol = content[start..comma].trim();
        let rest = &content[comma + 1..];
        let Some(type_name) = quoted_after(rest, "") else {
            cursor = comma + 1;
            continue;
        };
        let Some(array_start_relative) = rest.find('[') else {
            cursor = comma + 1;
            continue;
        };
        let array_start = comma + 1 + array_start_relative + 1;
        let Some(array_end_relative) = content[array_start..].find("]);") else {
            cursor = array_start;
            continue;
        };
        let array_end = array_start + array_end_relative;
        let mut values = Vec::new();
        for object in split_top_level_objects(&content[array_start..array_end]) {
            let Some(number) = signed_number_after(object, "no:") else { continue };
            let Some(name) = quoted_after(object, "name:") else { continue };
            values.push((number, name));
        }
        symbol_to_types
            .entry(symbol.to_string())
            .or_default()
            .insert(type_name.clone());
        values_by_type.insert(type_name, values);
        cursor = array_end + 3;
    }
}

fn parse_generated_enum_field_symbols(
    content: &str,
    output: &mut BTreeMap<(String, u64), String>,
) {
    let type_marker = ".typeName = \"";
    let fields_marker = ").fields = proto3.util.newFieldList(() => [";
    let mut cursor = 0usize;
    while let Some(relative) = content[cursor..].find(type_marker) {
        let type_pos = cursor + relative;
        let line_start = content[..type_pos].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
        let line_end = content[type_pos..]
            .find('\n')
            .map(|idx| type_pos + idx)
            .unwrap_or(content.len());
        let line = &content[line_start..line_end];
        let Some(symbol) = line
            .trim_start()
            .strip_prefix('(')
            .and_then(|value| value.split(" as MutableMessageType").next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            cursor = line_end;
            continue;
        };
        let Some(type_name) = quoted_after(line, ".typeName = ") else {
            cursor = line_end;
            continue;
        };
        let after = &content[line_end..];
        let symbol_marker = format!("({symbol} as MutableMessageType<");
        let Some(symbol_fields) = after.find(&symbol_marker) else {
            cursor = line_end;
            continue;
        };
        let fields_start = line_end + symbol_fields;
        let Some(marker_pos) = content[fields_start..].find(fields_marker) else {
            cursor = fields_start + symbol_marker.len();
            continue;
        };
        let array_start = fields_start + marker_pos + fields_marker.len();
        let Some(array_end_relative) = content[array_start..].find("]);") else {
            cursor = array_start;
            continue;
        };
        let array_end = array_start + array_end_relative;
        for object in split_top_level_objects(&content[array_start..array_end]) {
            let Some(number) = number_after(object, "no:") else { continue };
            let kind = quoted_after(object, "kind:").unwrap_or_default();
            let enum_source = if kind == "enum" {
                Some(object)
            } else if kind == "map" {
                object
                    .find("V:")
                    .and_then(|index| object.get(index + 2..))
                    .filter(|value| quoted_after(value, "kind:").as_deref() == Some("enum"))
            } else {
                None
            };
            let Some(enum_source) = enum_source else { continue };
            let Some(enum_marker) = enum_source.find("getEnumType(") else { continue };
            let enum_rest = &enum_source[enum_marker + "getEnumType(".len()..];
            let enum_symbol: String = enum_rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            if !enum_symbol.is_empty() {
                output.insert((type_name.clone(), number), enum_symbol);
            }
        }
        cursor = array_end + 3;
    }
}

fn generate_agent_tool_json_schema(manifest_dir: &std::path::Path) {
    let proto_root = manifest_dir.join("../packages/proto/generated");
    println!("cargo:rerun-if-changed={}", proto_root.display());
    let mut files = Vec::new();
    collect_generated_proto_ts_files(&proto_root, &mut files);

    let mut defs = BTreeMap::<String, CloudTraceGeneratedMessage>::new();
    let mut symbol_to_types = BTreeMap::<String, BTreeSet<String>>::new();
    let mut enum_values = BTreeMap::<String, Vec<(i64, String)>>::new();
    let mut enum_field_symbols = BTreeMap::<(String, u64), String>::new();
    for path in &files {
        let content = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read generated protobuf TypeScript {}: {error}", path.display()));
        parse_cloud_trace_message_definitions(&content, &mut defs, &mut symbol_to_types);
        parse_generated_enum_definitions(&content, &mut symbol_to_types, &mut enum_values);
        parse_generated_enum_field_symbols(&content, &mut enum_field_symbols);
    }
    add_cloud_trace_well_known_types(&mut defs, &mut symbol_to_types);
    enum_values.entry("google.protobuf.NullValue".to_string())
        .or_insert_with(|| vec![(0, "NULL_VALUE".to_string())]);
    symbol_to_types.entry("NullValue".to_string()).or_default()
        .insert("google.protobuf.NullValue".to_string());

    let parent_names = defs.keys().cloned().collect::<Vec<_>>();
    for parent_type in parent_names {
        let Some(message) = defs.get_mut(&parent_type) else { continue };
        for field in &mut message.fields {
            match &mut field.kind {
                CloudTraceGeneratedFieldKind::Message { child_symbol, child_type } => {
                    *child_type = resolve_cloud_trace_symbol(&parent_type, child_symbol, &symbol_to_types);
                }
                CloudTraceGeneratedFieldKind::Map {
                    value: CloudTraceGeneratedMapValueKind::Message { child_symbol, child_type },
                    ..
                } => {
                    *child_type = resolve_cloud_trace_symbol(&parent_type, child_symbol, &symbol_to_types);
                }
                _ => {}
            }
        }
    }

    let mut enum_field_types = BTreeMap::<(String, u64), String>::new();
    for ((parent_type, field_number), symbol) in enum_field_symbols {
        let enum_type = resolve_cloud_trace_symbol(&parent_type, &symbol, &symbol_to_types)
            .unwrap_or_else(|| panic!("could not resolve generated enum symbol {symbol} for {parent_type}.{field_number}"));
        enum_field_types.insert((parent_type, field_number), enum_type);
    }

    enum_field_types.insert(
        ("google.protobuf.Value".to_string(), 1),
        "google.protobuf.NullValue".to_string(),
    );

    let root = defs.get("agent.v1.ToolCall")
        .expect("canonical generated agent.v1.ToolCall is missing");
    let mut tool_calls = Vec::<(u64, String, String, Option<String>, Option<String>)>::new();
    let mut roots = Vec::<String>::new();
    for field in root.fields.iter().filter(|field| field.oneof.as_deref() == Some("tool")) {
        let CloudTraceGeneratedFieldKind::Message { child_type: Some(tool_type), .. } = &field.kind else {
            panic!("agent.v1.ToolCall field {} must resolve to a message", field.number);
        };
        let tool_message = defs.get(tool_type)
            .unwrap_or_else(|| panic!("generated tool call message missing: {tool_type}"));
        let args_type = tool_message.fields.iter()
            .find(|candidate| candidate.proto_name == "args")
            .and_then(|candidate| match &candidate.kind {
                CloudTraceGeneratedFieldKind::Message { child_type, .. } => child_type.clone(),
                _ => None,
            });
        let result_type = tool_message.fields.iter()
            .find(|candidate| candidate.proto_name == "result")
            .and_then(|candidate| match &candidate.kind {
                CloudTraceGeneratedFieldKind::Message { child_type, .. } => child_type.clone(),
                _ => None,
            });
        if let Some(value) = args_type.as_ref() { roots.push(value.clone()); }
        if let Some(value) = result_type.as_ref() { roots.push(value.clone()); }
        let static_name = field.proto_name
            .strip_suffix("_tool_call")
            .unwrap_or(&field.proto_name)
            .to_string();
        tool_calls.push((field.number, static_name, tool_type.clone(), args_type, result_type));
    }

    let mut reachable = BTreeSet::<String>::new();
    let mut queue = VecDeque::<String>::from(roots);
    while let Some(type_name) = queue.pop_front() {
        if !reachable.insert(type_name.clone()) { continue; }
        let message = defs.get(&type_name)
            .unwrap_or_else(|| panic!("generated agent tool JSON descriptor missing for {type_name}"));
        for field in &message.fields {
            match &field.kind {
                CloudTraceGeneratedFieldKind::Message { child_type: Some(child_type), .. } => {
                    if !reachable.contains(child_type) { queue.push_back(child_type.clone()); }
                }
                CloudTraceGeneratedFieldKind::Message { child_symbol, child_type: None } => {
                    panic!("could not resolve generated message symbol {child_symbol} for {type_name}.{}", field.proto_name);
                }
                CloudTraceGeneratedFieldKind::Map {
                    value: CloudTraceGeneratedMapValueKind::Message { child_type: Some(child_type), .. },
                    ..
                } => {
                    if !reachable.contains(child_type) { queue.push_back(child_type.clone()); }
                }
                CloudTraceGeneratedFieldKind::Map {
                    value: CloudTraceGeneratedMapValueKind::Message { child_symbol, child_type: None },
                    ..
                } => {
                    panic!("could not resolve generated map message symbol {child_symbol} for {type_name}.{}", field.proto_name);
                }
                _ => {}
            }
        }
    }

    let mut output = String::new();
    output.push_str("// @generated by source/host/build.rs from frozen generated protobuf TypeScript.\n");
    output.push_str("// Transcript occurrence tool JSON projection consumes this schema; do not hand-edit.\n\n");
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str("enum AgentToolJsonMapValueKind { Scalar(u64), Enum(&'static [(i64, &'static str)]), Message(&'static str) }\n");
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str("enum AgentToolJsonFieldKind { Scalar(u64), Enum(&'static [(i64, &'static str)]), Message(&'static str), Map { key_scalar: u64, value: AgentToolJsonMapValueKind } }\n");
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct AgentToolJsonFieldDescriptor { number: u64, json_name: &'static str, kind: AgentToolJsonFieldKind, repeated: bool, optional: bool, oneof: Option<&'static str> }\n");
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct AgentToolJsonMessageDescriptor { fields: &'static [AgentToolJsonFieldDescriptor] }\n");
    output.push_str("#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct AgentToolCallDescriptor { static_name: &'static str, args_type: Option<&'static str>, result_type: Option<&'static str> }\n\n");

    output.push_str("fn generated_agent_tool_call_descriptor(field_number: u64) -> Option<AgentToolCallDescriptor> {\n    match field_number {\n");
    for (number, static_name, _tool_type, args_type, result_type) in &tool_calls {
        output.push_str("        ");
        output.push_str(&number.to_string());
        output.push_str(" => Some(AgentToolCallDescriptor { static_name: ");
        output.push_str(&rust_string(static_name));
        output.push_str(", args_type: ");
        match args_type {
            Some(value) => {
                output.push_str("Some(");
                output.push_str(&rust_string(value));
                output.push(')');
            }
            None => output.push_str("None"),
        }
        output.push_str(", result_type: ");
        match result_type {
            Some(value) => {
                output.push_str("Some(");
                output.push_str(&rust_string(value));
                output.push(')');
            }
            None => output.push_str("None"),
        }
        output.push_str(" }),\n");
    }
    output.push_str("        _ => None,\n    }\n}\n\n");

    output.push_str("fn generated_agent_tool_json_message_descriptor(type_name: &str) -> Option<AgentToolJsonMessageDescriptor> {\n    match type_name {\n");
    for type_name in &reachable {
        let message = defs.get(type_name)
            .unwrap_or_else(|| panic!("reachable agent tool JSON descriptor missing for {type_name}"));
        output.push_str("        ");
        output.push_str(&rust_string(type_name));
        output.push_str(" => Some(AgentToolJsonMessageDescriptor { fields: &[\n");
        for field in &message.fields {
            output.push_str("            AgentToolJsonFieldDescriptor { number: ");
            output.push_str(&field.number.to_string());
            output.push_str(", json_name: ");
            output.push_str(&rust_string(&field.json_name));
            output.push_str(", kind: ");
            match &field.kind {
                CloudTraceGeneratedFieldKind::Scalar(scalar) => {
                    output.push_str("AgentToolJsonFieldKind::Scalar(");
                    output.push_str(&scalar.to_string());
                    output.push(')');
                }
                CloudTraceGeneratedFieldKind::Enum => {
                    let enum_type = enum_field_types.get(&(type_name.clone(), field.number))
                        .unwrap_or_else(|| panic!("generated enum type missing for {type_name}.{}", field.proto_name));
                    let values = enum_values.get(enum_type)
                        .unwrap_or_else(|| panic!("generated enum values missing for {enum_type}"));
                    output.push_str("AgentToolJsonFieldKind::Enum(&[");
                    for (number, name) in values {
                        output.push('(');
                        output.push_str(&number.to_string());
                        output.push_str(", ");
                        output.push_str(&rust_string(name));
                        output.push_str("),");
                    }
                    output.push_str("])");
                }
                CloudTraceGeneratedFieldKind::Message { child_type: Some(child_type), .. } => {
                    output.push_str("AgentToolJsonFieldKind::Message(");
                    output.push_str(&rust_string(child_type));
                    output.push(')');
                }
                CloudTraceGeneratedFieldKind::Message { .. } => {
                    panic!("unresolved generated message for {type_name}.{}", field.proto_name);
                }
                CloudTraceGeneratedFieldKind::Map { key_scalar, value } => {
                    output.push_str("AgentToolJsonFieldKind::Map { key_scalar: ");
                    output.push_str(&key_scalar.to_string());
                    output.push_str(", value: ");
                    match value {
                        CloudTraceGeneratedMapValueKind::Scalar(scalar) => {
                            output.push_str("AgentToolJsonMapValueKind::Scalar(");
                            output.push_str(&scalar.to_string());
                            output.push(')');
                        }
                        CloudTraceGeneratedMapValueKind::Enum => {
                            let enum_type = enum_field_types.get(&(type_name.clone(), field.number))
                                .unwrap_or_else(|| panic!("generated map enum type missing for {type_name}.{}", field.proto_name));
                            let values = enum_values.get(enum_type)
                                .unwrap_or_else(|| panic!("generated enum values missing for {enum_type}"));
                            output.push_str("AgentToolJsonMapValueKind::Enum(&[");
                            for (number, name) in values {
                                output.push('(');
                                output.push_str(&number.to_string());
                                output.push_str(", ");
                                output.push_str(&rust_string(name));
                                output.push_str("),");
                            }
                            output.push_str("])");
                        }
                        CloudTraceGeneratedMapValueKind::Message { child_type: Some(child_type), .. } => {
                            output.push_str("AgentToolJsonMapValueKind::Message(");
                            output.push_str(&rust_string(child_type));
                            output.push(')');
                        }
                        CloudTraceGeneratedMapValueKind::Message { .. } => {
                            panic!("unresolved generated map message for {type_name}.{}", field.proto_name);
                        }
                    }
                    output.push_str(" }");
                }
            }
            output.push_str(", repeated: ");
            output.push_str(if field.repeated { "true" } else { "false" });
            output.push_str(", optional: ");
            output.push_str(if field.optional { "true" } else { "false" });
            output.push_str(", oneof: ");
            match field.oneof.as_deref() {
                Some(oneof) => {
                    output.push_str("Some(");
                    output.push_str(&rust_string(oneof));
                    output.push(')');
                }
                None => output.push_str("None"),
            }
            output.push_str(" },\n");
        }
        output.push_str("        ] }),\n");
    }
    output.push_str("        _ => None,\n    }\n}\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("agent_tool_json_schema.rs"), output)
        .expect("write generated Agent tool JSON schema");
}

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let proto_dir = manifest_dir.join("../packages/proto/generated/agent/v1");
    println!("cargo:rerun-if-changed={}", proto_dir.display());

    let mut defs = BTreeMap::<String, MessageDef>::new();
    let mut symbol_to_type = BTreeMap::<String, String>::new();

    let mut files = fs::read_dir(&proto_dir)
        .expect("read generated protobuf directory")
        .map(|entry| entry.expect("read generated protobuf entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_pb.ts"))
        })
        .collect::<Vec<_>>();
    files.sort();

    for path in &files {
        let content = fs::read_to_string(path).expect("read generated protobuf TypeScript");
        parse_message_definitions(&content, &mut defs, &mut symbol_to_type);
    }

    for def in defs.values_mut() {
        for field in &mut def.fields {
            field.child_type = field
                .child_symbol
                .as_ref()
                .and_then(|symbol| symbol_to_type.get(symbol))
                .cloned();
        }
    }

    let metadata_path = proto_dir.join("blob-reference-metadata.ts");
    let metadata_content =
        fs::read_to_string(&metadata_path).expect("read blob-reference-metadata.ts");
    let metadata = parse_blob_metadata(&metadata_content);

    let mut proto_blob_types = BTreeSet::<String>::new();
    for fields in metadata.values() {
        for field in fields {
            if field.blob_reference_type.starts_with("agent.v1.") {
                proto_blob_types.insert(field.blob_reference_type.clone());
            }
        }
    }
    for type_name in &proto_blob_types {
        assert!(
            defs.contains_key(type_name),
            "blob-reference metadata target {type_name} has no generated message definition"
        );
    }

    let mut reachable = BTreeSet::<String>::new();
    let mut queue = VecDeque::<String>::new();
    queue.push_back("agent.v1.ConversationStateStructure".to_string());
    for type_name in &proto_blob_types {
        queue.push_back(type_name.clone());
    }
    while let Some(type_name) = queue.pop_front() {
        if !reachable.insert(type_name.clone()) {
            continue;
        }
        let Some(def) = defs.get(&type_name) else {
            continue;
        };
        for field in &def.fields {
            if let Some(child_type) = field.child_type.as_ref() {
                if child_type.starts_with("agent.v1.") && !reachable.contains(child_type) {
                    queue.push_back(child_type.clone());
                }
            }
        }
    }

    let mut output = String::new();
    output.push_str("// @generated by source/host/build.rs from frozen Grok protobuf metadata.\n");
    output.push_str("// Do not edit by hand; change the generated TypeScript metadata/schema inputs instead.\n\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct BlobReferenceFieldDescriptor { field_number: u64, blob_reference_type: &'static str, map_value: bool, skip: bool }\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct NestedMessageFieldDescriptor { field_number: u64, child_type: &'static str, map_value: bool }\n");
    output.push_str("#[allow(dead_code)]\n#[derive(Debug, Clone, Copy)]\n");
    output.push_str("struct MessageDescriptor { blob_fields: &'static [BlobReferenceFieldDescriptor], nested_fields: &'static [NestedMessageFieldDescriptor] }\n\n");

    output.push_str("fn is_proto_blob_reference_type(type_name: &str) -> bool {\n    matches!(type_name,\n");
    for type_name in &proto_blob_types {
        output.push_str("        ");
        output.push_str(&rust_string(type_name));
        output.push_str(" |\n");
    }
    output.push_str("        \"__never__\"\n    )\n}\n\n");

    output.push_str("fn message_descriptor(type_name: &str) -> Option<MessageDescriptor> {\n    match type_name {\n");
    for type_name in &reachable {
        let Some(def) = defs.get(type_name) else {
            continue;
        };
        output.push_str("        ");
        output.push_str(&rust_string(type_name));
        output.push_str(" => Some(MessageDescriptor {\n            blob_fields: &[\n");
        if let Some(blob_fields) = metadata.get(type_name) {
            for blob in blob_fields {
                let map_value = def
                    .fields
                    .iter()
                    .find(|field| field.number == blob.field_number)
                    .is_some_and(|field| field.is_map);
                let edge_name = format!("{type_name}.{}", blob.proto_field_name);
                let skip = matches!(
                    edge_name.as_str(),
                    "agent.v1.UserMessage.conversation_state_blob_id"
                        | "agent.v1.ConversationStateStructure.summary_archive"
                        | "agent.v1.ConversationStateStructure.summary_archives"
                );
                output.push_str("                BlobReferenceFieldDescriptor { field_number: ");
                output.push_str(&blob.field_number.to_string());
                output.push_str(", blob_reference_type: ");
                output.push_str(&rust_string(&blob.blob_reference_type));
                output.push_str(", map_value: ");
                output.push_str(if map_value { "true" } else { "false" });
                output.push_str(", skip: ");
                output.push_str(if skip { "true" } else { "false" });
                output.push_str(" },\n");
            }
        }
        output.push_str("            ],\n            nested_fields: &[\n");
        for field in &def.fields {
            if !field.map_message && field.child_symbol.is_none() {
                continue;
            }
            let Some(child_type) = field.child_type.as_ref() else {
                continue;
            };
            if !reachable.contains(child_type) {
                continue;
            }
            output.push_str("                NestedMessageFieldDescriptor { field_number: ");
            output.push_str(&field.number.to_string());
            output.push_str(", child_type: ");
            output.push_str(&rust_string(child_type));
            output.push_str(", map_value: ");
            output.push_str(if field.is_map { "true" } else { "false" });
            output.push_str(" },\n");
        }
        output.push_str("            ],\n        }),\n");
    }
    output.push_str("        _ => None,\n    }\n}\n");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("conversation_blob_gc_metadata.rs"), output)
        .expect("write conversation blob GC metadata");

    generate_cloud_agent_trace_schema(&manifest_dir);
    generate_client_side_tool_v2_schema(&manifest_dir);
    generate_agent_tool_json_schema(&manifest_dir);
    generate_agent_v1_local_exec_bindings(&manifest_dir);
}
