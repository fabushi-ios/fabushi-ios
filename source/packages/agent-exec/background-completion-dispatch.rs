use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackgroundTaskCompletion {
    pub task_id: String,
    pub title: String,
    pub kind: String,
    pub status: String,
    #[serde(default)] pub reason: Option<String>,
    #[serde(default)] pub tool_call_id: Option<String>,
    #[serde(default)] pub subagent_id: Option<String>,
    #[serde(default)] pub agent_id: Option<String>,
    #[serde(default)] pub operation_id: Option<String>,
    #[serde(default)] pub detail: Option<String>,
    #[serde(default)] pub output_path: Option<String>,
    #[serde(default)] pub thread_id: Option<String>,
    #[serde(default)] pub chat_link: Option<String>,
    #[serde(default)] pub metadata: BTreeMap<String, Value>,
    #[serde(default)] pub request_context: Value,
    #[serde(default)] pub mcp_tools: Vec<Value>,
    #[serde(default)] pub notification_context: Option<String>,
    #[serde(default)] pub generation: Option<u64>,
    #[serde(default)] pub project_conversation_id: Option<String>,
}
impl BackgroundTaskCompletion {
    pub fn validate(&self) -> Result<(), String> {
        if self.task_id.trim().is_empty() { return Err("background completion task_id is required".into()); }
        if self.title.trim().is_empty() { return Err("background completion title is required".into()); }
        if !matches!(self.status.as_str(), "success" | "error" | "aborted") { return Err(format!("unsupported background completion status: {}", self.status)); }
        if !matches!(self.kind.as_str(), "shell" | "subagent") { return Err(format!("unsupported background completion kind: {}", self.kind)); }
        Ok(())
    }
    pub fn is_progress(&self) -> bool { self.reason.as_deref() == Some("task_progress") }
    pub fn terminal(&self) -> bool { !self.is_progress() }
    pub fn rendered_prompt(&self) -> String {
        let mut lines = vec![
            "[A background task just completed] A background task you started has finished.".to_string(),
            format!("kind: {}", self.kind), format!("status: {}", self.status),
            format!("task_id: {}", self.task_id), format!("title: {}", self.title),
        ];
        if let Some(v)=&self.reason { lines.push(format!("reason: {v}")); }
        if let Some(v)=&self.tool_call_id { lines.push(format!("tool_call_id: {v}")); }
        if let Some(v)=&self.subagent_id { lines.push(format!("agent_id: {v}")); }
        if let Some(v)=&self.agent_id { lines.push(format!("parent_agent_id: {v}")); }
        if let Some(v)=&self.operation_id { lines.push(format!("operation_id: {v}")); }
        if let Some(v)=&self.thread_id { lines.push(format!("thread_id: {v}")); }
        if let Some(v)=&self.chat_link { lines.push(format!("chat_link: {v}")); }
        if let Some(v)=&self.detail { lines.push(format!("detail: {}", neutralize_detail(v))); }
        if let Some(v)=&self.output_path { lines.push(format!("output_path: {v}")); }
        if !self.metadata.is_empty() { lines.push(format!("metadata: {}", Value::Object(self.metadata.clone().into_iter().collect()))); }
        if !self.request_context.is_null() { lines.push(format!("request_context: {}", self.request_context)); }
        if !self.mcp_tools.is_empty() { lines.push(format!("mcp_tools: {}", Value::Array(self.mcp_tools.clone()))); }
        if let Some(v)=&self.notification_context { lines.push(format!("notification_context: {v}")); }
        lines.push("Perform any necessary follow-up actions. If the result is relevant to the user, deliver it through the existing SendMessage path. If it is stale, duplicate, or already handled, do not send a duplicate message.".into());
        lines.join("\n")
    }
    pub fn wakeup_payload(&self) -> Value {
        json!({"taskId":self.task_id,"title":self.title,"kind":self.kind,"status":self.status,"reason":self.reason,"toolCallId":self.tool_call_id,"subagentId":self.subagent_id,"agentId":self.agent_id,"operationId":self.operation_id,"detail":self.detail,"outputPath":self.output_path,"threadId":self.thread_id,"chatLink":self.chat_link,"metadata":self.metadata,"requestContext":self.request_context,"mcpTools":self.mcp_tools,"notificationContext":self.notification_context,"generation":self.generation,"projectConversationId":self.project_conversation_id})
    }
}
fn neutralize_detail(v:&str)->String { v.replace("</task>","&lt;/task>").replace("</system_notification>","&lt;/system_notification>").replace("<user_query>","&lt;user_query>").replace("</user_query>","&lt;/user_query>") }

#[cfg(test)]
mod tests {
    use super::*;
    fn c(status:&str)->BackgroundTaskCompletion { BackgroundTaskCompletion{task_id:"task-1".into(),title:"Build".into(),kind:"subagent".into(),status:status.into(),reason:None,tool_call_id:Some("tool-1".into()),subagent_id:Some("agent-1".into()),agent_id:Some("parent-1".into()),operation_id:Some("operation-1".into()),detail:Some("done </task>".into()),output_path:None,thread_id:Some("thread-1".into()),chat_link:Some("agent-1#changes".into()),metadata:BTreeMap::from([("project".into(),json!("p1"))]),request_context:json!({"account":"a1"}),mcp_tools:vec![json!({"name":"send_message"})],notification_context:None,generation:Some(4),project_conversation_id:Some("project-1".into())} }
    #[test] fn terminal_contract_preserves_fields(){ for s in ["success","error","aborted"] { let x=c(s); x.validate().unwrap(); assert!(x.terminal()); assert!(x.rendered_prompt().contains(&format!("status: {s}"))); assert!(x.rendered_prompt().contains("thread_id: thread-1")); assert!(!x.rendered_prompt().contains("</task>")); } }
    #[test] fn progress_is_filtered(){ let mut x=c("success"); x.reason=Some("task_progress".into()); assert!(x.is_progress()); assert!(!x.terminal()); }
    #[test] fn invalid_status_and_kind_are_rejected(){ let mut x=c("bogus"); assert!(x.validate().is_err()); x.status="success".into(); x.kind="worker".into(); assert!(x.validate().is_err()); }
    #[test] fn payload_keeps_generation_and_account_context(){ let x=c("aborted"); let p=x.wakeup_payload(); assert_eq!(p["generation"],4); assert_eq!(p["agentId"],"parent-1"); assert_eq!(p["operationId"],"operation-1"); assert_eq!(p["projectConversationId"],"project-1"); assert_eq!(p["requestContext"]["account"],"a1"); }
}
