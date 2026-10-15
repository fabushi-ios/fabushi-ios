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

pub fn generate_cloud_agent_trace_schema(manifest_dir: &std::path::Path) {
    let proto_root = manifest_dir.join("../../proto/generated");
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