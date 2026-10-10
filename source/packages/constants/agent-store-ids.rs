pub const AGENT_STORE_USER_MOUNT_NAME: &str = "user";
pub const AGENT_STORE_TEAM_MOUNT_NAME: &str = "team";
pub const AGENT_STORE_AUTOMATION_MOUNT_NAME: &str = "automation";
pub const AGENT_STORE_RESERVED_CURSOR_PATH_PREFIX: &str = ".cursor";
pub const NAMED_AGENT_HOME_STORE_MOUNT_NAME: &str = "home";
pub const CURSOR_AGENT_STORE_FILES_DIR_ENV: &str = "CURSOR_AGENT_STORE_FILES_DIR";
const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn is_hex(value:&str)->bool { value.bytes().all(|b| b.is_ascii_hexdigit()) }
pub fn is_valid_bare_uuid(value:&str)->bool {
    let parts=value.split('-').collect::<Vec<_>>();
    parts.len()==5
        && [8,4,4,4,12].into_iter().zip(parts.iter()).all(|(len,part)| part.len()==len && is_hex(part))
        && parts[2].bytes().next().is_some_and(|b| matches!(b.to_ascii_lowercase(),b'1'..=b'5'|b'7'))
        && parts[3].bytes().next().is_some_and(|b| matches!(b.to_ascii_lowercase(),b'8'|b'9'|b'a'|b'b'))
}
pub fn is_agent_store_id(value:&str)->bool { value.strip_prefix("store-").is_some_and(is_valid_bare_uuid) }
pub fn is_agent_store_share_mount_key(value:&str)->bool {
    value.strip_prefix("store-").is_some_and(|tail| tail.len()==24 && tail.bytes().all(|b| b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-')))
}
pub fn is_cloud_agent_store_id(value:&str)->bool {
    let Some(rest)=value.get(3..).filter(|_| value[..3].eq_ignore_ascii_case("bc-")) else { return false; };
    if rest.len()<36 { return false; }
    let uuid_start=rest.len()-36;
    let uuid=&rest[uuid_start..];
    if !is_valid_bare_uuid(uuid) { return false; }
    if uuid_start==0 { return true; }
    let prefix_with_sep=&rest[..uuid_start];
    let Some(prefix)=prefix_with_sep.strip_suffix('-') else { return false; };
    !prefix.is_empty()
        && prefix.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && prefix.bytes().all(|b| b.is_ascii_alphanumeric()||b==b'-')
}
pub fn is_agent_store_source_id(value:&str)->bool { is_cloud_agent_store_id(value)||is_valid_bare_uuid(value) }

fn positive_safe_integer(value:&str)->Option<u64>{
    if value.is_empty() || value.starts_with('+') || (value.len()>1 && value.starts_with('0')) || !value.bytes().all(|b| b.is_ascii_digit()) { return None; }
    value.parse::<u64>().ok().filter(|v| *v>0 && *v<=JS_MAX_SAFE_INTEGER)
}
pub fn parse_user_agent_store_source_id(source_id:&str)->Option<(u64,Option<u64>)>{
    let (team,user)=if let Some(rest)=source_id.strip_prefix('t') {
        let (team,user)=rest.split_once("-u")?;
        (Some(positive_safe_integer(team)?),positive_safe_integer(user)?)
    } else {
        (None,positive_safe_integer(source_id.strip_prefix('u')?)?)
    };
    Some((user,team))
}
pub fn parse_team_agent_store_source_id(source_id:&str)->Option<u64>{
    positive_safe_integer(source_id.strip_prefix('t')?)
}

#[cfg(test)]
mod tests {
    use super::*;
    const UUID:&str="12345678-1234-5123-8123-123456789abc";
    #[test]
    fn validates_uuid_store_share_and_cloud_shapes() {
        assert!(is_valid_bare_uuid(UUID));
        assert!(is_valid_bare_uuid("12345678-1234-7123-b123-123456789ABC"));
        assert!(!is_valid_bare_uuid("12345678-1234-6123-8123-123456789abc"));
        assert!(is_agent_store_id(&format!("store-{UUID}")));
        assert!(is_agent_store_share_mount_key("store-Abcdefghijklmnopqrstuv_1"));
        assert!(is_cloud_agent_store_id(&format!("bc-{UUID}")));
        assert!(is_cloud_agent_store_id(&format!("bc-prod-us-{UUID}")));
        assert!(!is_cloud_agent_store_id(&format!("bc--{UUID}")));
        assert!(is_agent_store_source_id(UUID));
    }
    #[test]
    fn parses_positive_safe_user_and_team_sources() {
        assert_eq!(parse_user_agent_store_source_id("u12"),Some((12,None)));
        assert_eq!(parse_user_agent_store_source_id("t7-u12"),Some((12,Some(7))));
        assert_eq!(parse_team_agent_store_source_id("t7"),Some(7));
        assert_eq!(parse_user_agent_store_source_id("u0"),None);
        assert_eq!(parse_user_agent_store_source_id("u9007199254740992"),None);
        assert_eq!(parse_team_agent_store_source_id("t01"),None);
    }
}
