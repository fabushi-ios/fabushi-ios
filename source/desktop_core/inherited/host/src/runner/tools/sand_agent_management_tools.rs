use std::sync::Arc;
use serde_json::{Value, json};
use crate::agents::agent_messaging::{AgentMessageImage,SAND_CREATE_AGENT_TOOL_NAME,SAND_SEND_TO_AGENT_TOOL_NAME,SAND_UPDATE_AGENT_TOOL_NAME};
use crate::extensions::inference::provider_session::{ProviderSessionError,RoutedToolDefinition};
use crate::runner::routed_provider_runtime::RoutedToolBridge;
use super::send_message_schema::is_valid_attachment_url;

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct AgentManagementRecord{pub id:String,pub name:String}

pub trait AgentManagementSink:Send+Sync{
    fn self_agent_id(&self)->&str;
    fn resolve_image_source(&self,image:&AgentMessageImage,_tool_call_id:&str)->Result<AgentMessageImage,ProviderSessionError>{Ok(image.clone())}
    fn send_to_agent(&self,target_id:&str,message:&str,images:&[AgentMessageImage],priority:bool)->Result<String,ProviderSessionError>;
    fn send_to_agent_with_reply(&self,target_id:&str,message:&str,images:&[AgentMessageImage],priority:bool,reply_to_id:Option<&str>)->Result<String,ProviderSessionError>{
        let _ = reply_to_id;
        self.send_to_agent(target_id,message,images,priority)
    }
    fn current_reply_to_id(&self)->Option<&str>{None}
    fn create_agent(&self,name:&str,description:&str)->Result<AgentManagementRecord,ProviderSessionError>;
    fn update_agent(&self,agent_id:&str,name:Option<&str>,description:Option<&str>)->Result<Option<AgentManagementRecord>,ProviderSessionError>;
}

pub struct AgentManagementToolBridge{delegate:Arc<dyn RoutedToolBridge>,sink:Arc<dyn AgentManagementSink>}
impl AgentManagementToolBridge{pub fn new(delegate:Arc<dyn RoutedToolBridge>,sink:Arc<dyn AgentManagementSink>)->Self{Self{delegate,sink}}}

fn send_to_agent_definition()->RoutedToolDefinition{RoutedToolDefinition{
    name:SAND_SEND_TO_AGENT_TOOL_NAME.into(),provider_identifier:"fabushi-runner".into(),tool_name:SAND_SEND_TO_AGENT_TOOL_NAME.into(),
    description:Some("Send a fire-and-forget asynchronous message to another agent or a group by id. Use priority=true only for a 1:1 STOP/supersede instruction; group posts ignore priority. Replies arrive later on a fresh turn.".into()),
    input_schema:json!({"type":"object","required":["target_id","message"],"additionalProperties":false,"properties":{
        "target_id":{"type":"string","minLength":1},"message":{"type":"string","minLength":1},
        "images":{"type":"array","items":{"type":"object","required":["url"],"additionalProperties":false,"properties":{"url":{"type":"string","minLength":1},"alt":{"type":"string"}}}},
        "priority":{"type":"boolean"}}}),
}}
fn create_agent_definition()->RoutedToolDefinition{RoutedToolDefinition{
    name:SAND_CREATE_AGENT_TOOL_NAME.into(),provider_identifier:"fabushi-runner".into(),tool_name:SAND_CREATE_AGENT_TOOL_NAME.into(),
    description:Some("Create a new teammate agent with a name and optional persona/description. Returns its id so it can be messaged with SendToAgent.".into()),
    input_schema:json!({"type":"object","required":["name"],"additionalProperties":false,"properties":{"name":{"type":"string","minLength":1},"description":{"type":"string"}}}),
}}
fn update_agent_definition()->RoutedToolDefinition{RoutedToolDefinition{
    name:SAND_UPDATE_AGENT_TOOL_NAME.into(),provider_identifier:"fabushi-runner".into(),tool_name:SAND_UPDATE_AGENT_TOOL_NAME.into(),
    description:Some("Edit another agent's name and/or description. Omitted fields remain unchanged.".into()),
    input_schema:json!({"type":"object","required":["agent_id"],"additionalProperties":false,"properties":{"agent_id":{"type":"string","minLength":1},"name":{"type":"string"},"description":{"type":"string"}}}),
}}

impl RoutedToolBridge for AgentManagementToolBridge{
    fn list_tools(&self)->Result<Vec<RoutedToolDefinition>,ProviderSessionError>{
        let mut tools=self.delegate.list_tools()?;
        tools.retain(|t|!matches!(t.name.as_str(),"SendToAgent"|"CreateAgent"|"UpdateAgent")&&!matches!(t.tool_name.as_str(),"SendToAgent"|"CreateAgent"|"UpdateAgent"));
        tools.insert(0,update_agent_definition());tools.insert(0,create_agent_definition());tools.insert(0,send_to_agent_definition());Ok(tools)
    }
    fn call_tool(&self,tool:&RoutedToolDefinition,args:Value,tool_call_id:&str)->Result<Value,ProviderSessionError>{
        let name=if matches!(tool.name.as_str(),"SendToAgent"|"CreateAgent"|"UpdateAgent"){tool.name.as_str()}else{tool.tool_name.as_str()};
        match name{
            "SendToAgent"=>{
                let o=args.as_object().ok_or_else(||ProviderSessionError::Tool("SendToAgent arguments must be an object".into()))?;
                let target=required_trimmed(o,"target_id",SAND_SEND_TO_AGENT_TOOL_NAME)?;
                let message=required_trimmed(o,"message",SAND_SEND_TO_AGENT_TOOL_NAME)?;
                if target==self.sink.self_agent_id(){return Ok(Value::String("You can't message yourself with SendToAgent. Use SendMessage to talk to the user, or pick a different target id.".into()));}
                let priority=o.get("priority").and_then(Value::as_bool).unwrap_or(false);
                let images=parse_images(o.get("images"))?
                    .into_iter()
                    .map(|image|self.sink.resolve_image_source(&image,tool_call_id))
                    .collect::<Result<Vec<_>,_>>()?;
                Ok(Value::String(self.sink.send_to_agent_with_reply(target,message,&images,priority,self.sink.current_reply_to_id())?))
            }
            "CreateAgent"=>{
                let o=args.as_object().ok_or_else(||ProviderSessionError::Tool("CreateAgent arguments must be an object".into()))?;
                let name=required_trimmed(o,"name",SAND_CREATE_AGENT_TOOL_NAME)?;
                let description=o.get("description").and_then(Value::as_str).map(str::trim).unwrap_or_default();
                let created=self.sink.create_agent(name,description)?;
                Ok(Value::String(format!("Created agent \"{}\" (id: {}). Message it with SendToAgent using that id.",created.name,created.id)))
            }
            "UpdateAgent"=>{
                let o=args.as_object().ok_or_else(||ProviderSessionError::Tool("UpdateAgent arguments must be an object".into()))?;
                let id=required_trimmed(o,"agent_id",SAND_UPDATE_AGENT_TOOL_NAME)?;
                let name=optional_trimmed(o.get("name"));let description=optional_trimmed(o.get("description"));
                if name.is_none()&&description.is_none(){return Ok(Value::String("Nothing to update: provide a new name and/or description.".into()));}
                Ok(Value::String(match self.sink.update_agent(id,name,description)?{
                    Some(updated)=>format!("Updated agent \"{}\" (id: {}).",updated.name,updated.id),
                    None=>format!("No agent found with id {id}."),
                }))
            }
            _=>self.delegate.call_tool(tool,args,tool_call_id),
        }
    }
}

fn required_trimmed<'a>(o:&'a serde_json::Map<String,Value>,field:&str,tool:&str)->Result<&'a str,ProviderSessionError>{
    o.get(field).and_then(Value::as_str).map(str::trim).filter(|v|!v.is_empty()).ok_or_else(||ProviderSessionError::Tool(format!("{field} is required for {tool}")))
}
fn optional_trimmed(value:Option<&Value>)->Option<&str>{value.and_then(Value::as_str).map(str::trim).filter(|v|!v.is_empty())}
fn parse_images(value:Option<&Value>)->Result<Vec<AgentMessageImage>,ProviderSessionError>{
    let Some(value)=value else{return Ok(Vec::new());};let rows=value.as_array().ok_or_else(||ProviderSessionError::Tool("images must be an array".into()))?;
    let mut images=Vec::with_capacity(rows.len());
    for(index,row)in rows.iter().enumerate(){
        let o=row.as_object().ok_or_else(||ProviderSessionError::Tool(format!("images[{index}] must be an object")))?;
        let url=o.get("url").and_then(Value::as_str).map(str::trim).filter(|v|!v.is_empty()).ok_or_else(||ProviderSessionError::Tool(format!("images[{index}].url is required")))?;
        if !is_valid_attachment_url(url){return Err(ProviderSessionError::Tool(format!("images[{index}].url must include a file:// or https:// scheme")));}
        images.push(AgentMessageImage{url:url.into(),alt:optional_trimmed(o.get("alt")).map(ToOwned::to_owned)});
    }Ok(images)
}
