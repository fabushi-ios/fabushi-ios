use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mahayana_host_runtime::extensions::inference::provider_session::{
    ProviderSessionError, RoutedToolDefinition,
};
use mahayana_host_runtime::extensions::session::production::ProductionSessionWorkers;
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_multitask_todo_tool::{
    MultitaskTodoItem, MultitaskTodoState, MultitaskTodoStatus,
    ProductionMultitaskTodoState,
    SAND_MULTITASK_TODO_TOOL_NAME, SAND_MULTITASK_TODOS_KV,
    SandMultitaskTodoToolBridge,
};
use mahayana_host_runtime::runner::tools::turn_toolset::{
    TurnToolsetDependencies, build_turn_toolset,
};
use mahayana_host_runtime::sand_multitask::SAND_MULTITASK_TODO_DESCRIPTION;
use serde_json::{Value, json};

#[derive(Default)]
struct EmptyBridge;

impl RoutedToolBridge for EmptyBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        Ok(Vec::new())
    }

    fn call_tool(
        &self,
        _tool: &RoutedToolDefinition,
        _args: Value,
        _tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        Err(ProviderSessionError::Tool("unexpected delegated tool".into()))
    }
}

#[derive(Default)]
struct MemoryTodoState {
    todos: Mutex<Vec<MultitaskTodoItem>>,
}

impl MultitaskTodoState for MemoryTodoState {
    fn load_todos(&self) -> Result<Vec<MultitaskTodoItem>, String> {
        Ok(self.todos.lock().unwrap().clone())
    }

    fn store_todos(&self, todos: &[MultitaskTodoItem]) -> Result<(), String> {
        *self.todos.lock().unwrap() = todos.to_vec();
        Ok(())
    }
}

fn todo_tool(bridge: &Arc<dyn RoutedToolBridge>) -> RoutedToolDefinition {
    bridge
        .list_tools()
        .unwrap()
        .into_iter()
        .find(|tool| tool.name == SAND_MULTITASK_TODO_TOOL_NAME)
        .expect("TodoWrite")
}

fn root(label: &str) -> std::path::PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "fabushi-multitask-todo-{label}-{}-{nonce}",
        std::process::id()
    ))
}

#[test]
fn turn_toolset_exposes_exact_multitask_description_only_when_enabled() {
    let state: Arc<dyn MultitaskTodoState> = Arc::new(MemoryTodoState::default());
    let disabled = build_turn_toolset(
        Arc::new(EmptyBridge),
        TurnToolsetDependencies {
            multitask_enabled: false,
            multitask_todo_state: Some(Arc::clone(&state)),
            ..TurnToolsetDependencies::default()
        },
    );
    assert!(
        disabled
            .list_tools()
            .unwrap()
            .iter()
            .all(|tool| tool.name != SAND_MULTITASK_TODO_TOOL_NAME)
    );

    let enabled = build_turn_toolset(
        Arc::new(EmptyBridge),
        TurnToolsetDependencies {
            multitask_enabled: true,
            multitask_todo_state: Some(state),
            ..TurnToolsetDependencies::default()
        },
    );
    let definition = todo_tool(&enabled);
    assert_eq!(
        definition.description.as_deref(),
        Some(SAND_MULTITASK_TODO_DESCRIPTION)
    );
    assert_eq!(
        definition.input_schema["properties"]["todos"]["minItems"],
        json!(2)
    );
}

#[test]
fn merge_and_replace_follow_frozen_todo_semantics() {
    let state: Arc<dyn MultitaskTodoState> = Arc::new(MemoryTodoState::default());
    let bridge: Arc<dyn RoutedToolBridge> = Arc::new(SandMultitaskTodoToolBridge::new(
        Arc::new(EmptyBridge),
        Arc::clone(&state),
    ));
    let tool = todo_tool(&bridge);

    bridge
        .call_tool(
            &tool,
            json!({
                "merge": false,
                "todos": [
                    {"id":"a","content":"First","status":"in_progress"},
                    {"id":"b","content":"Second","status":"pending"}
                ]
            }),
            "call-1",
        )
        .unwrap();

    let rendered = bridge
        .call_tool(
            &tool,
            json!({
                "merge": true,
                "todos": [
                    {"id":"a","status":"completed"},
                    {"id":"c","content":"Third","status":"in_progress"}
                ]
            }),
            "call-2",
        )
        .unwrap();
    let rendered = rendered.as_str().unwrap();
    assert!(rendered.contains("**COMPLETED**: First (id: a)"));
    assert!(rendered.contains("**PENDING**: Second (id: b)"));
    assert!(rendered.contains("**IN_PROGRESS**: Third (id: c)"));

    let todos = state.load_todos().unwrap();
    assert_eq!(todos.len(), 3);
    assert_eq!(todos[0].status, MultitaskTodoStatus::Completed);

    let error = bridge
        .call_tool(
            &tool,
            json!({"todos":[{"id":"new","status":"pending"}]}),
            "call-3",
        )
        .unwrap_err();
    assert!(error.to_string().contains("must provide 'content'"));
}

#[test]
fn production_agent_db_adapter_keeps_todos_across_tool_bridge_rebuilds() {
    let root = root("sqlite");
    let workers = Arc::new(ProductionSessionWorkers::with_agents_root(&root, 500));
    let session = workers
        .materialize_session_with_active(None, "user", None, None)
        .expect("session");
    let state: Arc<dyn MultitaskTodoState> = Arc::new(ProductionMultitaskTodoState::new(
        Arc::clone(&workers),
        &session.record.id,
    ));
    let first: Arc<dyn RoutedToolBridge> = Arc::new(SandMultitaskTodoToolBridge::new(
        Arc::new(EmptyBridge),
        Arc::clone(&state),
    ));
    let tool = todo_tool(&first);
    first
        .call_tool(
            &tool,
            json!({
                "merge": false,
                "todos": [
                    {"id":"a","content":"Persist me","status":"in_progress"},
                    {"id":"b","content":"Also persist","status":"pending"}
                ]
            }),
            "call-1",
        )
        .unwrap();

    assert!(
        session
            .db
            .read_kv(SAND_MULTITASK_TODOS_KV)
            .unwrap()
            .is_some()
    );

    let rebuilt: Arc<dyn RoutedToolBridge> = Arc::new(SandMultitaskTodoToolBridge::new(
        Arc::new(EmptyBridge),
        state,
    ));
    let persisted = rebuilt
        .call_tool(
            &todo_tool(&rebuilt),
            json!({"merge":true,"todos":[{"id":"a","status":"completed"}]}),
            "call-2",
        )
        .unwrap();
    assert!(persisted.as_str().unwrap().contains("**COMPLETED**: Persist me"));

    workers.shutdown();
    let _ = fs::remove_dir_all(root);
}
