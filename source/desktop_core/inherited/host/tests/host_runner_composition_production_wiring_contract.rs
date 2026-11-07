const OWNER: &str = include_str!("../src/host_runner_composition.rs");
const SHIPPING_HOST: &str = include_str!("../app/src/main.rs");
const GENERATED_STREAM: &str = include_str!("../src/runner/generated_agent_turn_stream.rs");
const TURN_OWNER: &str = include_str!("../src/runner/production_turn_agent_owner.rs");

#[test]
fn host_runner_composition_owns_turn_decoration_order() {
    for needle in [
        "ProductionRunnerCompositionHooks as ProductionTurnCompositionHooks",
        "pub fn compose_production_turn(",
        "create_production_runner_composition_with_hooks(input, hooks)",
    ] {
        assert!(
            OWNER.contains(needle),
            "HostRunnerComposition missing production ownership: {needle}"
        );
    }
}

#[test]
fn shipping_host_has_one_runner_composition_entrypoint() {
    assert!(
        SHIPPING_HOST.contains(
            "let worker_host_runner_composition = Arc::clone(&host_runner_composition);"
        ),
        "shipping Runner worker must use the same HostRunnerComposition owner via Arc::clone",
    );
    assert!(
        SHIPPING_HOST.contains("worker_host_runner_composition.compose_production_turn("),
        "shipping Host must delegate turn assembly to HostRunnerComposition",
    );
    assert!(
        !SHIPPING_HOST.contains("create_production_runner_composition("),
        "shipping Host must not construct a parallel Runner composition",
    );
    for needle in [
        ".with_agent_management_sink(agent_management_sink)",
        ".with_state_writer(state_writer)",
        ".with_routine_auto_review(routine_auto_review)",
        ".with_box_shell_review(box_shell_review)",
        "composition.with_subagent_task_sink(",
        "composition.with_subagent_management(",
        "composition.with_routine_post_write(",
        "composition.with_multitask_todo_state(",
    ] {
        assert!(
            !SHIPPING_HOST.contains(needle),
            "shipping Host still owns Runner decoration outside HostRunnerComposition: {needle}",
        );
    }
}

#[test]
fn host_runner_composition_owns_transcript_checkpoint_wiring() {
    for needle in [
        "pub fn compose_production_checkpoint_sink(",
        "sessions.open_agent_store_owner(agent_id)?",
        "sessions.create_agent_blob_store(agent_id)?",
        "transcript_providers: Mutex<HashMap<PathBuf, Arc<CanonicalTranscriptMirrorProvider>>>",
        ".entry(transcripts_dir.clone())",
        "ProductionTranscriptMirrorProvider::with_reporter(",
        "GeneratedTranscriptOccurrenceCodec::new(",
        "CanonicalGeneratedToolJsonProjection",
        "Arc::clone(&report_outcome)",
        "transcript_provider.route_for_session(",
        "ProductionAgentStateCheckpointSink::new(",
        "if group_member_turn",
        "return Ok(None)",
    ] {
        assert!(
            OWNER.contains(needle),
            "HostRunnerComposition missing checkpoint composition responsibility: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains(".compose_production_checkpoint_sink("),
        "shipping Host must delegate transcript/checkpoint composition",
    );
    for needle in [
        "ProductionTranscriptMirrorProvider::with_reporter(",
        "GeneratedTranscriptOccurrenceCodec::new(",
        "ProductionAgentStateCheckpointSink::new(",
        ".open_agent_store_owner(&agent_id)",
        ".create_agent_blob_store(&agent_id)",
    ] {
        assert!(
            !SHIPPING_HOST.contains(needle),
            "shipping Host still owns transcript/checkpoint wiring: {needle}",
        );
    }
}

#[test]
fn host_runner_composition_owns_turn_state_surface_wiring() {
    for needle in [
        "pub struct ProductionTurnStateSurfaces",
        "pub fn compose_turn_state_surfaces(",
        ".memory_service()",
        ".agents_root_dir()",
        "group_member_turn: bool",
        "state_writer: None",
        "SandAgentState::new(sand_root, agent_id.to_string())",
        "avatar_box_file_reader: Option<AvatarBoxFileReader>",
        "state = state.with_box_file_reader(reader)",
        "state = state.with_agent_memory_remover(Arc::new(move |memory_id|",
        "memory_sessions.remove_agent_memory(&memory_agent_id, memory_id)",
        "state = state.with_agent_profile_writer(Arc::new(move |name, description|",
        "profile_sessions.update_agent_profile_fields(",
        "ProductionMultitaskTodoState::new(",
        "Arc::clone(&sessions),",
    ] {
        assert!(
            OWNER.contains(needle),
            "HostRunnerComposition missing state-surface responsibility: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains(".compose_turn_state_surfaces(")
            && SHIPPING_HOST.contains("Arc::clone(&session_workers),")
            && SHIPPING_HOST.contains("&agent_id,")
            && SHIPPING_HOST.contains("is_group_member_turn,")
            && SHIPPING_HOST.contains("multitask_enabled,")
            && SHIPPING_HOST.contains("let avatar_box_file_reader: AvatarBoxFileReader")
            && SHIPPING_HOST.contains("forever_box")
            && SHIPPING_HOST.contains(".download_file(&(), &avatar_agent_id, path)")
            && SHIPPING_HOST.contains("Some(avatar_box_file_reader),"),
        "shipping Host must consume HostRunnerComposition state surfaces",
    );
    for needle in [
        "SandAgentState::new(state_sand_root",
        "could not open production multitask todo state for {agent_id}",
        "production memory agents root has no sand root parent",
    ] {
        assert!(
            !SHIPPING_HOST.contains(needle),
            "shipping Host still owns turn state-surface composition: {needle}",
        );
    }
}

#[test]
fn host_runner_composition_owns_shipping_runner_construction() {
    for needle in [
        "pub fn compose_production_runner(",
        "create_production_runner(",
        "composition,",
        "checkpoint_sink,",
        "upgrade_quiesce_signal,",
        "generated_agent_runtime,",
    ] {
        assert!(
            OWNER.contains(needle),
            "HostRunnerComposition missing Runner construction responsibility: {needle}"
        );
    }
    assert!(
        SHIPPING_HOST.contains("worker_host_runner_composition.compose_production_runner("),
        "shipping Host must delegate Runner facade construction",
    );
    for needle in [
        "ProductionTurnAgentOwner::new(composition)",
        "SandAgentRunner::new(owner)",
    ] {
        assert!(
            !SHIPPING_HOST.contains(needle),
            "shipping Host still constructs the Runner owner path directly: {needle}",
        );
    }
}

#[test]
fn host_runner_composition_owns_computer_use_session_lifecycle() {
    for needle in [
        "pub fn begin_computer_use_preparation(",
        "owner.acquire_control_lease(agent_id)",
        "owner.begin_preparation(agent_id)",
        "ComputerUsePrewarmStage::Box",
        "pub fn mark_computer_use_preparation_ready(",
        "pub fn mark_computer_use_preparation_failed(",
        "pub fn owns_computer_control_lease(",
        "pub fn record_computer_audit_intent(",
        "pub fn capture_computer_navigation_baseline(",
        "pub fn request_computer_navigation_probe(",
        "pub fn computer_navigation_probe_wait_ms(",
        "pub fn complete_computer_navigation_probe(",
        "pub fn finish_computer_use_turn(",
        "owner.record_model_id(model_id)",
        "owner.record_turn_ended(usage)",
        "owner.release_control_lease(lease)",
        "owner.free_window(agent_id)",
    ] {
        assert!(
            OWNER.contains(needle),
            "HostRunnerComposition missing computer-use lifecycle ownership: {needle}"
        );
    }
    for needle in [
        ".acquire_control_lease(&agent_id)",
        ".begin_preparation(&agent_id)",
        ".record_turn_ended(usage)",
        ".release_control_lease(lease)",
        ".free_window(&agent_id)",
    ] {
        assert!(
            !SHIPPING_HOST.contains(needle),
            "shipping Host still owns computer-use session composition: {needle}"
        );
    }
    assert_eq!(
        SHIPPING_HOST.matches("computer_use_coordination()").count(),
        1,
        "shipping Host may only read the canonical HostRunnerComposition computer-use owner to inject that same service into SandAgentRunner",
    );
    assert!(
        SHIPPING_HOST.contains(
            "Some(worker_host_runner_composition.computer_use_coordination()),"
        ),
        "shipping Runner must receive the canonical HostRunnerComposition computer-use coordination rather than a second owner",
    );
    assert!(
        SHIPPING_HOST.contains("host_runner_composition.begin_computer_use_preparation(&agent_id)"),
        "shipping Host must delegate computer-use preparation",
    );
    assert!(
        SHIPPING_HOST.contains("worker_host_runner_composition.finish_computer_use_turn("),
        "shipping Host must delegate computer-use turn settlement",
    );
    for needle in [
        "navigation_probe_command(display_number)",
        "worker_host_runner_composition.capture_computer_navigation_baseline(stdout)",
        ".with_post_action_callback(",
        "computer_navigation_owner.request_computer_navigation_probe()",
        "navigation_owner.complete_computer_navigation_probe(",
    ] {
        assert!(
            SHIPPING_HOST.contains(needle),
            "shipping Host missing frozen computer-use navigation-probe wiring: {needle}",
        );
    }
    let normalized_shipping_host = SHIPPING_HOST
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        normalized_shipping_host
            .contains("remote_computer_audit_owner .record_computer_audit_intent(action_case);"),
        "shipping Host must delegate computer audit intent to HostRunnerComposition",
    );
}


#[test]
fn group_member_turn_keeps_generated_lifecycle_without_private_state() {
    let group_flag = SHIPPING_HOST
        .find("let is_group_member_turn = args")
        .expect("shipping Host must resolve group-member identity");
    let private_memory = SHIPPING_HOST
        .find("let memory_store = if is_group_member_turn")
        .expect("private memory must be explicitly gated for group-member turns");
    assert!(
        group_flag < private_memory,
        "group-member identity must be known before private memory is resolved",
    );
    assert!(
        OWNER.contains("return Ok(None)")
            && OWNER.contains("state_writer: None")
            && OWNER.contains("group_member_turn: bool"),
        "HostRunnerComposition must omit private checkpoint/state surfaces for group-member turns",
    );
    assert!(
        SHIPPING_HOST.contains("if !is_group_member_turn")
            && SHIPPING_HOST.contains("with_persist_image_callback(browser_persist_image)")
            && SHIPPING_HOST.contains("with_persist_image_callback(computer_persist_image)"),
        "group-member turns must not bind private image persistence callbacks",
    );
    assert!(
        GENERATED_STREAM.contains(
            "checkpoint_sink: Option<Arc<dyn AgentStateCheckpointSink>>"
        ) && GENERATED_STREAM.contains(
            "pub checkpoint: Option<TextTurnCheckpointArtifacts>"
        ),
        "generated Agent lifecycle must model private checkpointing as optional",
    );
    assert!(
        TURN_OWNER.contains("run_production_generated_agent_stream(")
            && !TURN_OWNER.contains("if let Some(checkpoint_sink) = checkpoint_sink"),
        "absence of a private checkpoint must not downgrade the turn to a raw provider path",
    );
}


#[test]
fn host_runner_composition_settles_only_its_owned_shutdown_surfaces() {
    assert!(
        OWNER.contains("pub fn dispose(&self)")
            && OWNER.contains(".clear();"),
        "HostRunnerComposition must dispose its live permission subscriptions",
    );
    let cancel = SHIPPING_HOST
        .find("runner_registry.cancel_all(\"Mahayana Host shutting down\")")
        .expect("shipping shutdown must interrupt canonical Runner registry");
    let dispose = SHIPPING_HOST
        .find("host_runner_composition.dispose();")
        .expect("shipping shutdown must settle HostRunnerComposition");
    assert!(
        cancel < dispose,
        "RunnerRegistry must interrupt active turns before composition-owned surfaces are dropped",
    );
    assert!(
        !OWNER.contains("runner_registry.cancel_all"),
        "HostRunnerComposition must not duplicate TranscriptRunnerRegistry cancellation ownership",
    );
}


#[test]
fn shell_terminal_watch_shipping_path_uses_canonical_permission_and_generated_watermark() {
    let host = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("app/src/main.rs"),
    )
    .expect("shipping host");
    let composition = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/host_runner_composition.rs"),
    )
    .expect("host runner composition");
    let forever_box = std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/extensions/forever_box/runner_tools.rs"),
    )
    .expect("forever box runner tools");

    assert!(host.contains(".with_background_read_permission(Arc::clone(&shell_watch_local_tool_permission))"));
    assert!(forever_box.contains("SandLocalToolScope {"));
    assert!(forever_box.contains("agent_id: self.agent_id.clone()"));
    assert!(forever_box.contains("SandLocalToolRequest::simple(\"read-file\", output_path.clone())"));
    assert!(host.contains("confirmed_user_turn_watermark("));
    assert!(composition.contains("latest_checkpoint_bytes()"));
    assert!(composition.contains("create_agent_blob_store(agent_id)"));
    assert!(composition.contains("decode_conversation_turn_structure_fields"));
    assert!(composition.contains("decode_user_message_identity_fields"));
    assert!(composition.contains("find_confirmed_user_turn_watermark(&turns, None)"));
    assert!(composition.contains("event.request.session_key"));
    assert!(composition.contains("append_agent_transcript_entries_if_session_admitted"));
    assert!(composition.contains("update_agent_transcript_entry_if_session_admitted"));
}


#[test]
fn generated_agent_stream_owns_canonical_stream_attempt_runtime() {
    const GENERATED_STREAM: &str =
        include_str!("../src/runner/generated_agent_turn_stream.rs");
    const TURN_COMPOSITION: &str =
        include_str!("../src/runner/turn_agent_composition.rs");
    const ROUTED_PROVIDER: &str =
        include_str!("../src/runner/routed_provider_runtime.rs");
    const ATTEMPT_ADAPTER: &str =
        include_str!("../src/runner/production_turn_run_shell_adapter.rs");

    assert!(GENERATED_STREAM.contains(
        "Arc::new(StreamAttemptRuntime::new("
    ));
    assert!(GENERATED_STREAM.contains(
        ".with_stream_attempt_runtime(stream_attempt_runtime)"
    ));
    assert!(TURN_COMPOSITION.contains(
        "stream_attempt_runtime: self.stream_attempt_runtime.clone()"
    ));
    assert!(ROUTED_PROVIDER.contains(
        "run_with_attempt_runtime_reporting("
    ));
    assert!(ATTEMPT_ADAPTER.contains(
        "let generation = runtime.begin_attempt();"
    ));
    assert!(ATTEMPT_ADAPTER.contains(
        "runtime_for_delta.mark_stream_output(generation)"
    ));
    assert!(ATTEMPT_ADAPTER.contains(
        "runtime_for_checkpoint.is_current(generation)"
    ));
}
