#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandLocalToolResolution { AllowOnce, Deny, Always, Never }

pub fn parse_sand_local_tool_permission_resolution(value:&str)->Option<SandLocalToolResolution>{
    match value {
        "allow-once"=>Some(SandLocalToolResolution::AllowOnce),
        "deny"=>Some(SandLocalToolResolution::Deny),
        "always"=>Some(SandLocalToolResolution::Always),
        "never"=>Some(SandLocalToolResolution::Never),
        _=>None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalToolPermissionResolutionArgs {
    pub agent_id:String,
    pub entry_id:String,
    pub request_id:String,
    pub resolution:String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingLocalToolPermissionRequest { pub agent_id:String }

pub trait LocalToolPermissionAskStore {
    fn get_pending_request_by_id(&self,request_id:&str)->Option<PendingLocalToolPermissionRequest>;
    fn was_settled(&self,request_id:&str)->bool;
    fn resolve_request(&self,request_id:&str,resolution:SandLocalToolResolution)->bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleLocalToolPermissionCardSettlement { NotSettled, Settled, Retired }

pub trait LocalToolPermissionWidgetResponses {
    fn settle_stale_local_tool_permission_card(
        &self,agent_id:&str,entry_id:&str,request_id:&str
    )->Result<StaleLocalToolPermissionCardSettlement,String>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandLocalToolPermissionResolutionError {
    #[error("Unknown local-tool permission resolution.")]
    UnknownResolution,
    #[error("That local-tool permission request is no longer waiting for an answer.")]
    Stale,
    #[error("{0}")]
    Transcript(String),
}

pub fn resolve_local_tool_permission_ask(
    asks:&dyn LocalToolPermissionAskStore,
    transcript:&dyn LocalToolPermissionWidgetResponses,
    args:&LocalToolPermissionResolutionArgs,
    on_stranded_retirement:Option<&dyn Fn()>,
)->Result<(),SandLocalToolPermissionResolutionError>{
    let resolution=parse_sand_local_tool_permission_resolution(&args.resolution)
        .ok_or(SandLocalToolPermissionResolutionError::UnknownResolution)?;
    let Some(pending)=asks.get_pending_request_by_id(&args.request_id) else {
        if asks.was_settled(&args.request_id) { return Ok(()); }
        let settlement=transcript.settle_stale_local_tool_permission_card(
            &args.agent_id,&args.entry_id,&args.request_id
        ).map_err(SandLocalToolPermissionResolutionError::Transcript)?;
        return match settlement {
            StaleLocalToolPermissionCardSettlement::Settled=>Ok(()),
            StaleLocalToolPermissionCardSettlement::Retired=>{
                if let Some(callback)=on_stranded_retirement { callback(); }
                Ok(())
            }
            StaleLocalToolPermissionCardSettlement::NotSettled=>
                Err(SandLocalToolPermissionResolutionError::Stale),
        };
    };
    if pending.agent_id!=args.agent_id { return Err(SandLocalToolPermissionResolutionError::Stale); }
    if !asks.resolve_request(&args.request_id,resolution) {
        return Err(SandLocalToolPermissionResolutionError::Stale);
    }
    Ok(())
}
