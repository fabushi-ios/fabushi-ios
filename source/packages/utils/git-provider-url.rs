fn strip_git_suffix(segment: &str) -> String {
    if segment.len() >= 4 && segment[segment.len() - 4..].eq_ignore_ascii_case(".git") {
        segment[..segment.len() - 4].to_owned()
    } else {
        segment.to_owned()
    }
}

pub fn is_github_dot_com_host(hostname: &str) -> bool {
    let lower=hostname.to_ascii_lowercase();
    lower=="github.com" || lower.ends_with(".github.com")
}

pub fn is_gitlab_host(hostname: &str) -> bool {
    hostname.to_ascii_lowercase().split('.').any(|part| part=="gitlab")
}

pub fn is_bitbucket_cloud_host(hostname: &str) -> bool {
    matches!(hostname.to_ascii_lowercase().as_str(), "bitbucket.org" | "www.bitbucket.org")
}

pub fn is_azure_devops_services_host(hostname: &str) -> bool {
    matches!(hostname.to_ascii_lowercase().as_str(), "dev.azure.com" | "www.dev.azure.com")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRepositoryPath {
    pub owner: String,
    pub repo: String,
    pub has_subpath: bool,
}

pub fn parse_github_repository_path_segments(segments: &[&str]) -> Option<GitHubRepositoryPath> {
    let owner=*segments.first()?;
    let raw_repo=*segments.get(1)?;
    if owner.is_empty() || raw_repo.is_empty() { return None; }
    let repo=strip_git_suffix(raw_repo);
    (!repo.is_empty()).then(|| GitHubRepositoryPath{owner:owner.into(),repo,has_subpath:segments.len()>2})
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabRepositoryPath {
    pub repository_segments: Vec<String>,
    pub has_special_route_separator: bool,
}

pub fn parse_gitlab_repository_path_segments(segments: &[&str]) -> Option<GitLabRepositoryPath> {
    let special=segments.iter().position(|segment| *segment=="-");
    let source=&segments[..special.unwrap_or(segments.len())];
    if source.len()<2 || source.iter().any(|segment| segment.is_empty()) { return None; }
    let mut repository_segments=source.iter().map(|segment| (*segment).to_owned()).collect::<Vec<_>>();
    let last=repository_segments.len()-1;
    repository_segments[last]=strip_git_suffix(&repository_segments[last]);
    if repository_segments[last].is_empty(){return None;}
    Some(GitLabRepositoryPath{repository_segments,has_special_route_separator:special.is_some()})
}

fn hex_value(byte:u8)->Option<u8> {
    match byte {
        b'0'..=b'9'=>Some(byte-b'0'),
        b'a'..=b'f'=>Some(byte-b'a'+10),
        b'A'..=b'F'=>Some(byte-b'A'+10),
        _=>None,
    }
}

fn decode_uri_component(value:&str)->Option<String> {
    let bytes=value.as_bytes();
    let mut decoded=Vec::with_capacity(bytes.len());
    let mut index=0;
    while index<bytes.len() {
        if bytes[index]==b'%' {
            if index+2>=bytes.len(){return None;}
            decoded.push((hex_value(bytes[index+1])?<<4)|hex_value(bytes[index+2])?);
            index+=3;
        } else {
            decoded.push(bytes[index]);
            index+=1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn encode_uri_component(value:&str)->String {
    const HEX:&[u8;16]=b"0123456789ABCDEF";
    let mut out=String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte,b'-'|b'_'|b'.'|b'!'|b'~'|b'*'|b'\''|b'('|b')') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte>>4) as usize] as char);
            out.push(HEX[(byte&15) as usize] as char);
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitbucketRepositoryPath {
    pub workspace: String,
    pub repo: String,
    pub has_subpath: bool,
}

pub fn parse_bitbucket_repository_path_segments(segments:&[&str])->Option<BitbucketRepositoryPath>{
    let raw_workspace=*segments.first()?;
    let raw_repo=*segments.get(1)?;
    let workspace=decode_uri_component(raw_workspace)?;
    let decoded_repo=decode_uri_component(raw_repo)?;
    if workspace.is_empty() || decoded_repo.is_empty() || workspace.contains('/') || decoded_repo.contains('/') { return None; }
    if matches!(workspace.to_ascii_lowercase().as_str(),"account"|"dashboard"){return None;}
    let repo=strip_git_suffix(&decoded_repo);
    if repo.is_empty(){return None;}
    Some(BitbucketRepositoryPath{workspace:encode_uri_component(&workspace),repo:encode_uri_component(&repo),has_subpath:segments.len()>2})
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AzureDevopsRepositoryPath {
    pub organization:String,
    pub project:String,
    pub repo:String,
    pub has_subpath:bool,
}

pub fn parse_azure_devops_repository_path_segments(segments:&[&str])->Option<AzureDevopsRepositoryPath>{
    let git_index=segments.iter().position(|segment| segment.eq_ignore_ascii_case("_git"))?;
    if git_index<1 || git_index>2{return None;}
    let organization=*segments.first()?;
    let raw_repo=*segments.get(git_index+1)?;
    if organization.is_empty()||raw_repo.is_empty(){return None;}
    let repo=strip_git_suffix(raw_repo);
    let project=if git_index==2 { (*segments.get(1)?).to_owned() } else { repo.clone() };
    if repo.is_empty()||project.is_empty(){return None;}
    Some(AzureDevopsRepositoryPath{organization:organization.into(),project,repo,has_subpath:segments.len()>git_index+2})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_provider_hosts_like_reference() {
        assert!(is_github_dot_com_host("api.GitHub.com"));
        assert!(is_gitlab_host("gitlab.example.com"));
        assert!(!is_gitlab_host("notgitlab.example.com"));
        assert!(is_bitbucket_cloud_host("WWW.BITBUCKET.ORG"));
        assert!(is_azure_devops_services_host("dev.azure.com"));
    }

    #[test]
    fn parses_github_gitlab_and_azure_paths() {
        assert_eq!(
            parse_github_repository_path_segments(&["openai","openai.git","issues"]),
            Some(GitHubRepositoryPath{owner:"openai".into(),repo:"openai".into(),has_subpath:true})
        );
        assert_eq!(
            parse_gitlab_repository_path_segments(&["group","sub","repo.git","-","issues"]).unwrap(),
            GitLabRepositoryPath{repository_segments:vec!["group".into(),"sub".into(),"repo".into()],has_special_route_separator:true}
        );
        assert_eq!(
            parse_azure_devops_repository_path_segments(&["org","project","_git","repo.git","pullrequest"]).unwrap(),
            AzureDevopsRepositoryPath{organization:"org".into(),project:"project".into(),repo:"repo".into(),has_subpath:true}
        );
    }

    #[test]
    fn bitbucket_decodes_then_reencodes_components_and_rejects_slashes() {
        assert_eq!(
            parse_bitbucket_repository_path_segments(&["my%20team","repo%20one.git","src"]).unwrap(),
            BitbucketRepositoryPath{workspace:"my%20team".into(),repo:"repo%20one".into(),has_subpath:true}
        );
        assert!(parse_bitbucket_repository_path_segments(&["account","repo"]).is_none());
        assert!(parse_bitbucket_repository_path_segments(&["team","repo%2Fchild"]).is_none());
        assert!(parse_bitbucket_repository_path_segments(&["team","bad%ZZ"]).is_none());
    }
}
