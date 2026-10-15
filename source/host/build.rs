mod build_cloud_agent_trace;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn require_markers(path: &Path, markers: &[&str]) {
    let content = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read canonical schema {}: {error}", path.display()));
    for marker in markers {
        assert!(
            content.contains(marker),
            "canonical schema {} is missing required marker {marker:?}",
            path.display()
        );
    }
}

fn main() {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
    );
    let host_root = manifest_dir.join("../../../host");
    let proto_root = host_root.join("proto");
    let proto_file = proto_root.join("agent/v1/local_exec.proto");

    println!("cargo:rerun-if-changed={}", proto_file.display());
    require_markers(
        &proto_file,
        &[
            "message ExecClientControlMessage",
            "enum TimeoutBehavior",
            "TIMEOUT_BEHAVIOR_CANCEL",
            "TIMEOUT_BEHAVIOR_BACKGROUND",
            "enum ShellBackgroundReason",
            "enum ShellAbortReason",
            "message ShellStreamHookContext",
            "message ShellSandboxUnsupported",
            "message ReadSuccess",
            "bytes output_blob_id = 7;",
            "bool truncated = 6;",
            "message ExecClientMessage",
            "google.protobuf.Struct write_shell_stdin_result = 23;",
            "google.protobuf.Struct adopt_result = 56;",
            "message ExecServerMessage",
            "google.protobuf.Struct write_shell_stdin_args = 23;",
            "google.protobuf.Struct adopt_args = 56;",
            "optional bool accept_hook_additional_contexts = 55;",
        ],
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let descriptor_path = out_dir.join("agent_v1_local_exec_descriptor.bin");
    let protoc = protoc_bin_vendored::protoc_bin_path()
        .expect("vendored protoc");
    let protoc_include = protoc_bin_vendored::include_path()
        .expect("vendored protoc includes");

    let mut config = prost_build::Config::new();
    config
        .protoc_executable(protoc)
        .file_descriptor_set_path(&descriptor_path)
        .compile_well_known_types()
        .extern_path(".google.protobuf", "::pbjson_types");
    config
        .compile_protos(&[proto_file], &[proto_root, protoc_include])
        .expect("generate canonical agent.v1 local-exec prost bindings");

    let descriptor_set = fs::read(&descriptor_path)
        .expect("read canonical agent.v1 local-exec descriptor set");
    let mut json = pbjson_build::Builder::new();
    json.register_descriptors(&descriptor_set)
        .expect("register canonical agent.v1 local-exec descriptors");
    json.ignore_unknown_fields();
    json.build(&[".agent.v1"])
        .expect("generate canonical agent.v1 local-exec protobuf JSON");

    build_cloud_agent_trace::generate_cloud_agent_trace_schema(&manifest_dir);
}
