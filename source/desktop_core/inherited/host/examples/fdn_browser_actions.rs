#[cfg(unix)]
mod actions {
// Nonshipping adapter for real browser resource execution in disposable Actions.
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::net::TcpListener;

use base64::Engine;
use mahayana_host_runtime::extensions::inference::provider_session::{ProviderSessionError, RoutedToolDefinition};
use mahayana_host_runtime::runner::box_tool_access::{RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxShellRequest, RunnerBoxWriteRequest};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_browser_tools::{browser_tool_specs, ProductionBrowserToolExecutor, SandBrowserToolBridge};
use mahayana_host_runtime::r#box::box_remote_accessor::BoxEndpoint;
use mahayana_host_runtime::r#box::box_file_transfer::{FileTransferAccessor, WriteExecResult};
use mahayana_host_runtime::r#box::box_shell_command::{build_host_shell_args, HostShellArgsInput};
use mahayana_host_runtime::r#box::exec_daemon_process::{BoxExecDaemonProcessOptions, start_box_exec_daemon_process};
use mahayana_host_runtime::r#box::generated_production::{create_production_box_resource_accessor, ProductionBoxResourceAccessor, ProductionBoxTransport, ProductionReadArgs, ProductionReadOutput, ProductionReadResult, ProductionShellResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

struct ActionsResources {
    accessor: Mutex<ProductionBoxResourceAccessor>,
    shells: AtomicUsize,
    writes: AtomicUsize,
    reads: AtomicUsize,
}
fn failure(message: &str) -> ProviderSessionError { ProviderSessionError::Tool(message.into()) }
fn driver_path(path: &str) -> Result<(), ProviderSessionError> {
    let path=Path::new(path);
    if !path.starts_with("/workspace/.sand-browser") || path.components().any(|part| matches!(part,std::path::Component::ParentDir)) {
        return Err(failure("Actions browser resource path denied"));
    }
    Ok(())
}
impl RunnerBoxResourcePort for ActionsResources {
    fn execute_shell(&self, request: RunnerBoxShellRequest) -> Result<Value, ProviderSessionError> {
        if request.working_directory!="/workspace" || request.is_background {return Err(failure("Actions browser shell scope denied"));}
        self.shells.fetch_add(1,Ordering::SeqCst);
        let args=build_host_shell_args(HostShellArgsInput {command:request.command,name:"g11-browser-resource".into(),working_directory:request.working_directory,tool_call_id:request.tool_call_id});
        let result=self.accessor.lock().map_err(|_|failure("resource lock failed"))?.execute_shell_result(&(),args).map_err(|_|failure("real Box shell RPC failed"))?;
        match result {
            ProductionShellResult::Success{exit_code,..}=>Ok(json!({"kind":"success","exitCode":exit_code})),
            _=>Err(failure("real Box shell execution failed")),
        }
    }
    fn execute_write(&self, request: RunnerBoxWriteRequest) -> Result<(), ProviderSessionError> {
        driver_path(&request.path)?;
        if request.data.len()>2*1024*1024 {return Err(failure("Actions write limit"));}
        let result=self.accessor.lock().map_err(|_|failure("resource lock failed"))?.execute_write(&(),&request.path,&request.data,&request.tool_call_id).map_err(|_|failure("real Box write RPC failed"))?;
        if result!=WriteExecResult::Success {return Err(failure("real Box write failed"));}
        self.writes.fetch_add(1,Ordering::SeqCst);Ok(())
    }
    fn execute_read(&self, request: RunnerBoxReadRequest) -> Result<Value, ProviderSessionError> {
        driver_path(&request.path)?;
        let result=self.accessor.lock().map_err(|_|failure("resource lock failed"))?.execute_read(&(),ProductionReadArgs {path:request.path,tool_call_id:request.tool_call_id,offset:request.offset,limit:request.limit,encoding_hint:request.encoding_hint}).map_err(|_|failure("real Box read RPC failed"))?;
        self.reads.fetch_add(1,Ordering::SeqCst);
        match result {
            ProductionReadResult::Success{output,truncated:false,..}=>match output {
                ProductionReadOutput::Content(content)=>Ok(json!({"kind":"success","output":{"content":content}})),
                ProductionReadOutput::Data(data)=>Ok(json!({"kind":"success","output":{"data":data}})),
                _=>Err(failure("real Box returned empty resource")),
            },
            _=>Err(failure("real Box resource read failed")),
        }
    }
    fn browser_window_index(&self)->Result<u32,ProviderSessionError> {Ok(77)}
}
struct NoDelegate;
impl RoutedToolBridge for NoDelegate {
    fn list_tools(&self)->Result<Vec<RoutedToolDefinition>,ProviderSessionError>{Ok(Vec::new())}
    fn call_tool(&self,_tool:&RoutedToolDefinition,_args:Value,_id:&str)->Result<Value,ProviderSessionError>{Err(failure("Actions unknown tool"))}
}
pub fn run()->Result<(),Box<dyn std::error::Error>> {
    if std::env::var("GITHUB_ACTIONS").as_deref()!=Ok("true") {return Err("Actions only".into());}
    let encoded=std::env::args().nth(1).ok_or("missing synthetic request")?;
    if encoded.len()>128*1024 {return Err("request limit".into());}
    let bytes=base64::engine::general_purpose::STANDARD.decode(encoded)?;
    let mut args:Value=serde_json::from_slice(&bytes)?;
    let operation=args.get("op").and_then(Value::as_str).ok_or("missing operation")?.to_owned();
    let spec=browser_tool_specs().into_iter().find(|spec|spec.op==operation).ok_or("unknown operation")?;
    args.as_object_mut().ok_or("request object required")?.remove("op");
    let executable=std::env::var_os("G11_BOX_DAEMON_BIN").ok_or("missing exact Box daemon")?;
    let listener=TcpListener::bind(("127.0.0.1",0))?;
    let daemon_port=listener.local_addr()?.port();drop(listener);
    let token=uuid::Uuid::new_v4().to_string();
    let terminals=Path::new("/workspace/.sand-browser").join(format!("daemon-terminals-{}",uuid::Uuid::new_v4()));
    let mut daemon=start_box_exec_daemon_process(BoxExecDaemonProcessOptions {executable_path:executable.into(),host:"127.0.0.1".into(),port:daemon_port,auth_token:token.clone(),workspace_root:"/workspace".into(),terminals_directory:terminals.clone(),start_timeout_ms:5000,stop_timeout_ms:2000}).map_err(|_|"real Box daemon startup/authentication failed")?;
    if daemon.pid==0 {return Err("missing real daemon PID".into());}
    let transport=ProductionBoxTransport::from_endpoint(&BoxEndpoint::new("127.0.0.1",daemon_port,token));
    let port=Arc::new(ActionsResources {accessor:Mutex::new(create_production_box_resource_accessor(&transport)),shells:AtomicUsize::new(0),writes:AtomicUsize::new(0),reads:AtomicUsize::new(0)});
    let safety=Arc::new(AtomicUsize::new(0));let observed_safety=safety.clone();
    let executor=ProductionBrowserToolExecutor::new(port.clone(),"primary")
        .with_shell_safety_callback(Arc::new(move |_request| {observed_safety.fetch_add(1,Ordering::SeqCst);Ok(())}));
    let bridge=SandBrowserToolBridge::new(Arc::new(NoDelegate),Arc::new(executor));
    let tool=bridge.list_tools()?.into_iter().find(|tool|tool.name==spec.name).ok_or("missing routed tool")?;
    let mut result=bridge.call_tool(&tool,args,"g11-actions")?;
    let stdout=fs::read_to_string("/workspace/.sand-browser/result-g11-actions.txt")?;
    let line=stdout.lines().rev().find_map(|line|line.strip_prefix("__SAND_BROWSER_RESULT__")).ok_or("missing actual driver result")?;
    let raw:Value=serde_json::from_str(line)?;
    let image=result.as_object_mut().ok_or("bridge result object")?.remove("imageB64");
    let image=image.as_ref().and_then(Value::as_str).map(|text|base64::engine::general_purpose::STANDARD.decode(text)).transpose()?;
    daemon.close().map_err(|_|"real Box daemon shutdown failed")?;
    let _=fs::remove_dir_all(terminals);
    println!("{}",json!({"boxRpc":"authenticated-connect-protobuf","bridge":result,"driver":raw,"imageBytes":image.as_ref().map(Vec::len).unwrap_or(0),
        "imageSha256":image.as_ref().map(|bytes|format!("{:x}",Sha256::digest(bytes))),
        "io":{"shell":port.shells.load(Ordering::SeqCst),"write":port.writes.load(Ordering::SeqCst),"read":port.reads.load(Ordering::SeqCst),"safety":safety.load(Ordering::SeqCst)}}));
    Ok(())
}

}

#[cfg(unix)]
fn main() -> Result<(), Box<dyn std::error::Error>> { actions::run() }

#[cfg(not(unix))]
fn main() { panic!("managed Linux Actions environment required"); }
