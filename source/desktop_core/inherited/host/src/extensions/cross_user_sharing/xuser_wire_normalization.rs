use serde_json::Value;

pub const MAX_INLINE_IMAGES_PER_ENTRY:usize=4;

#[derive(Debug,Clone,PartialEq,Eq)]
pub struct InlineImage{pub base64:String,pub media_type:String,pub alt:Option<String>}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct TurnMessage{pub speaker_kind:String,pub speaker_name:String,pub is_self:bool,pub text:String}
#[derive(Debug,Clone,PartialEq,Eq)]
pub enum MirrorEntry{
 HumanMessage{entry_id:String,author_auth_id:String,author_name:String,author_avatar_url:Option<String>,text:String,images:Vec<InlineImage>,client_nonce:Option<String>,timestamp_ms:u64},
 AgentMessage{entry_id:String,agent_owner_auth_id:String,agent_id:String,author_name:String,text:String,images:Vec<InlineImage>,timestamp_ms:u64},
}
fn clamp_guest_name(value:&str)->String{
 let one=value.replace(['\r','\n']," ").trim().chars().take(120).collect::<String>();
 if one.is_empty(){"Someone".into()}else{one}
}
pub fn normalize_inline_images(raw:&Value)->Vec<InlineImage>{
 let Some(items)=raw.as_array()else{return vec![]};let mut out=Vec::new();
 for value in items.iter().take(MAX_INLINE_IMAGES_PER_ENTRY){
  let Some(obj)=value.as_object()else{continue};let Some(base64)=obj.get("base64").and_then(Value::as_str).filter(|v|!v.is_empty())else{continue};
  let media=obj.get("mediaType").and_then(Value::as_str).filter(|v|v.starts_with("image/")).unwrap_or("image/png");
  let alt=obj.get("alt").and_then(Value::as_str).filter(|v|!v.is_empty()).map(str::to_string);
  out.push(InlineImage{base64:base64.into(),media_type:media.into(),alt});
 }out
}
pub fn normalize_mirror_entry(value:&Value,now_ms:u64)->Option<MirrorEntry>{
 let obj=value.as_object()?;let entry_id=obj.get("entryId")?.as_str()?.to_string();if entry_id.is_empty(){return None}
 let text=obj.get("text").and_then(Value::as_str).unwrap_or("").to_string();
 let images=normalize_inline_images(obj.get("images").unwrap_or(&Value::Null));
 if text.is_empty()&&images.is_empty(){return None}
 let timestamp_ms=obj.get("timestampMs").and_then(Value::as_u64).unwrap_or(now_ms);
 match obj.get("kind").and_then(Value::as_str){
  Some("human-message")=>{
   let author_auth_id=obj.get("authorAuthId")?.as_str()?.to_string();if author_auth_id.is_empty(){return None}
   Some(MirrorEntry::HumanMessage{
    entry_id,author_auth_id,author_name:clamp_guest_name(obj.get("authorName").and_then(Value::as_str).unwrap_or("")),
    author_avatar_url:obj.get("authorAvatarUrl").and_then(Value::as_str).filter(|v|!v.is_empty()).map(str::to_string),
    text,images,client_nonce:obj.get("clientNonce").and_then(Value::as_str).filter(|v|!v.is_empty()).map(str::to_string),timestamp_ms})
  }
  Some("agent-message")=>{
   let owner=obj.get("agentOwnerAuthId")?.as_str()?.to_string();let agent=obj.get("agentId")?.as_str()?.to_string();if owner.is_empty()||agent.is_empty(){return None}
   Some(MirrorEntry::AgentMessage{entry_id,agent_owner_auth_id:owner,agent_id:agent,author_name:clamp_guest_name(obj.get("authorName").and_then(Value::as_str).unwrap_or("")),text,images,timestamp_ms})
  }
  _=>None
 }
}
pub fn normalize_turn_messages(raw:&Value)->Vec<TurnMessage>{
 let Some(items)=raw.as_array()else{return vec![]};let mut out=Vec::new();
 for value in items.iter().take(24){let Some(obj)=value.as_object()else{continue};let Some(text)=obj.get("text").and_then(Value::as_str).filter(|v|!v.is_empty())else{continue};
  out.push(TurnMessage{speaker_kind:if obj.get("speakerKind").and_then(Value::as_str)==Some("agent"){"agent".into()}else{"human".into()},speaker_name:clamp_guest_name(obj.get("speakerName").and_then(Value::as_str).unwrap_or("")),is_self:obj.get("isSelf").and_then(Value::as_bool)==Some(true),text:text.into()});
 }out
}
