use std::sync::{Arc,Mutex};
use mahayana_host_runtime::agents::agent_messaging::AgentMessageImage;
use mahayana_host_runtime::extensions::inference::provider_session::{ProviderSessionError,RoutedToolDefinition};
use mahayana_host_runtime::runner::routed_provider_runtime::RoutedToolBridge;
use mahayana_host_runtime::runner::tools::sand_agent_management_tools::{AgentManagementRecord,AgentManagementSink,AgentManagementToolBridge};
use serde_json::{Value,json};

#[derive(Default)]struct Delegate;
impl RoutedToolBridge for Delegate{
 fn list_tools(&self)->Result<Vec<RoutedToolDefinition>,ProviderSessionError>{Ok(Vec::new())}
 fn call_tool(&self,_:&RoutedToolDefinition,_:Value,_:&str)->Result<Value,ProviderSessionError>{Err(ProviderSessionError::Tool("unexpected delegate".into()))}
}
#[derive(Default)]struct Sink{sends:Mutex<Vec<(String,String,bool,usize)>>,sent_images:Mutex<Vec<Vec<String>>>,resolved_images:Mutex<Vec<(String,String)>>,creates:Mutex<Vec<(String,String)>>,updates:Mutex<Vec<(String,Option<String>,Option<String>)>>}
impl AgentManagementSink for Sink{
 fn self_agent_id(&self)->&str{"self"}
 fn resolve_image_source(&self,image:&AgentMessageImage,tool_call_id:&str)->Result<AgentMessageImage,ProviderSessionError>{self.resolved_images.lock().unwrap().push((tool_call_id.into(),image.url.clone()));Ok(AgentMessageImage{url:format!("resolved:{}",image.url),alt:image.alt.clone()})}
 fn send_to_agent(&self,target:&str,message:&str,images:&[AgentMessageImage],priority:bool)->Result<String,ProviderSessionError>{self.sends.lock().unwrap().push((target.into(),message.into(),priority,images.len()));self.sent_images.lock().unwrap().push(images.iter().map(|image|image.url.clone()).collect());Ok("sent".into())}
 fn create_agent(&self,name:&str,description:&str)->Result<AgentManagementRecord,ProviderSessionError>{self.creates.lock().unwrap().push((name.into(),description.into()));Ok(AgentManagementRecord{id:"new-agent".into(),name:name.into()})}
 fn update_agent(&self,id:&str,name:Option<&str>,description:Option<&str>)->Result<Option<AgentManagementRecord>,ProviderSessionError>{self.updates.lock().unwrap().push((id.into(),name.map(ToOwned::to_owned),description.map(ToOwned::to_owned)));Ok(Some(AgentManagementRecord{id:id.into(),name:name.unwrap_or("Existing").into()}))}
}

#[test]
fn runner_management_tools_are_first_party_and_validate_send_inputs(){
 let sink=Arc::new(Sink::default());let bridge=AgentManagementToolBridge::new(Arc::new(Delegate),sink.clone());let tools=bridge.list_tools().expect("tools");
 assert_eq!(tools.iter().take(3).map(|t|t.name.as_str()).collect::<Vec<_>>(),vec!["SendToAgent","CreateAgent","UpdateAgent"]);
 let send=&tools[0];
 assert_eq!(bridge.call_tool(send,json!({"target_id":"other","message":" hello ","priority":true,"images":[{"url":"https://example.com/a.png","alt":"a"}]}),"c1").expect("send"),Value::String("sent".into()));
 assert_eq!(sink.sends.lock().unwrap().as_slice(),&[("other".into(),"hello".into(),true,1)]);
 assert_eq!(sink.resolved_images.lock().unwrap().as_slice(),&[("c1".into(),"https://example.com/a.png".into())]);
 assert_eq!(sink.sent_images.lock().unwrap().as_slice(),&[vec![String::from("resolved:https://example.com/a.png")]]);
 assert!(bridge.call_tool(send,json!({"target_id":"other","message":"x","images":[{"url":"relative.png"}]}),"c2").is_err());
 assert!(bridge.call_tool(send,json!({"target_id":"self","message":"x"}),"c3").expect("self").as_str().unwrap().contains("can't message yourself"));
}
#[test]
fn create_and_update_tools_delegate_with_merge_semantics(){
 let sink=Arc::new(Sink::default());let bridge=AgentManagementToolBridge::new(Arc::new(Delegate),sink.clone());let tools=bridge.list_tools().expect("tools");
 let created=bridge.call_tool(&tools[1],json!({"name":" Research ","description":" deep work "}),"create").expect("create");assert!(created.as_str().unwrap().contains("new-agent"));
 let updated=bridge.call_tool(&tools[2],json!({"agent_id":"agent-b","description":"new persona"}),"update").expect("update");assert!(updated.as_str().unwrap().contains("agent-b"));
 assert_eq!(sink.updates.lock().unwrap().as_slice(),&[("agent-b".into(),None,Some("new persona".into()))]);
}
