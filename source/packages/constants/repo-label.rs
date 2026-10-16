use url::Url;

const KNOWN_GIT_HOSTING_DOMAINS: [&str; 7] = ["github.com","gitlab.com","bitbucket.org","bitbucket.com","codeberg.org","gitea.com","sr.ht"];

pub fn is_known_git_hosting_domain(hostname:&str)->bool {
    let lower=hostname.to_ascii_lowercase();
    KNOWN_GIT_HOSTING_DOMAINS.iter().any(|domain| lower==*domain || lower.ends_with(&format!(".{domain}")))
}
fn is_origin_repo_host(hostname:&str)->bool {
    let lower=hostname.to_ascii_lowercase();
    let Some(prefix)=lower.strip_suffix(".cursor.com") else { return false; };
    prefix=="origin" || prefix.strip_prefix("origin-").is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_lowercase()||b.is_ascii_digit()))
}
fn trim_repo_path(pathname:&str)->String {
    pathname.trim_matches('/').strip_suffix(".git").unwrap_or(pathname.trim_matches('/')).to_owned()
}
fn extract_owner_repo_from_path(pathname:&str)->Option<String>{
    let trimmed=trim_repo_path(pathname);
    let parts=trimmed.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>();
    (parts.len()>=2).then(|| format!("{}/{}",parts[0],parts[1..].join("/")))
}
fn extract_owner_repo_from_origin_path(pathname:&str)->Option<String>{
    let trimmed=trim_repo_path(pathname);
    let parts=trimmed.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>();
    if parts.len()==3 && parts[0].eq_ignore_ascii_case("git") { return Some(format!("{}/{}",parts[1],parts[2])); }
    if parts.len()==2 && !parts[0].eq_ignore_ascii_case("git") { return Some(format!("{}/{}",parts[0],parts[1])); }
    None
}
fn origin_host_without_port(value:&str)->&str { value.rsplit_once(':').filter(|(_,port)| port.bytes().all(|b| b.is_ascii_digit())).map(|(host,_)|host).unwrap_or(value) }

pub fn rewrite_scp_git_url(raw:&str)->String {
    if raw.contains("://") { return raw.to_owned(); }
    let Some((user,rest))=raw.split_once('@') else { return raw.to_owned(); };
    if user.is_empty() || user.contains('/') || user.contains('@') { return raw.to_owned(); }
    let Some((host,path))=rest.split_once(':') else { return raw.to_owned(); };
    if host.is_empty() || path.is_empty() { return raw.to_owned(); }
    format!("https://{host}/{path}")
}

pub fn parse_repo_name_from_url(repo_url:Option<&str>)->Option<String>{
    let trimmed=repo_url?.trim();
    if trimmed.is_empty(){return None;}
    if trimmed.contains("://") {
        let parsed=Url::parse(trimmed).ok()?;
        return if is_origin_repo_host(parsed.host_str()?) { extract_owner_repo_from_origin_path(parsed.path()) } else { extract_owner_repo_from_path(parsed.path()) };
    }
    let slash=trimmed.find('/')?;
    let first=&trimmed[..slash];
    let rest=&trimmed[slash..];
    if is_origin_repo_host(origin_host_without_port(first)) { return extract_owner_repo_from_origin_path(rest); }
    if is_known_git_hosting_domain(first) { extract_owner_repo_from_path(rest) } else { extract_owner_repo_from_path(&format!("/{trimmed}")) }
}

pub fn parse_self_hosted_repo_scope(repo_url:&str)->Option<String>{
    let mut candidate=repo_url.to_owned();
    if !candidate.contains("://") {
        let slash=candidate.find('/')?;
        let first=&candidate[..slash];
        if !first.contains('.') || is_known_git_hosting_domain(first) { return None; }
        candidate=format!("https://{candidate}");
    }
    let parsed=Url::parse(&candidate).ok()?;
    let hostname=parsed.host_str()?;
    if is_known_git_hosting_domain(hostname) || is_origin_repo_host(hostname) { return None; }
    let path=trim_repo_path(parsed.path());
    if path.is_empty(){return None;}
    let host=match parsed.port(){Some(port)=>format!("{hostname}:{port}"),None=>hostname.to_owned()};
    Some(format!("{host}/{path}"))
}

pub fn derive_repo_label_value_from_url(repo_url:Option<&str>)->Option<String>{
    let trimmed=repo_url?.trim();
    if trimmed.is_empty(){return None;}
    let rewritten=rewrite_scp_git_url(trimmed);
    parse_self_hosted_repo_scope(&rewritten).or_else(|| parse_repo_name_from_url(Some(&rewritten)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recognizes_known_hosts_and_parses_common_repo_urls() {
        assert!(is_known_git_hosting_domain("api.github.com"));
        assert_eq!(parse_repo_name_from_url(Some("https://github.com/openai/openai.git")),Some("openai/openai".into()));
        assert_eq!(parse_repo_name_from_url(Some("gitlab.com/group/sub/repo")),Some("group/sub/repo".into()));
        assert_eq!(parse_repo_name_from_url(Some("https://origin-us1.cursor.com/git/acme/repo")),Some("acme/repo".into()));
        assert_eq!(parse_repo_name_from_url(Some("origin.cursor.com/acme/repo")),Some("acme/repo".into()));
    }
    #[test]
    fn derives_self_hosted_scope_and_rewrites_scp_urls() {
        assert_eq!(rewrite_scp_git_url("git@git.example.com:team/repo.git"),"https://git.example.com/team/repo.git");
        assert_eq!(derive_repo_label_value_from_url(Some("git@git.example.com:team/repo.git")),Some("git.example.com/team/repo".into()));
        assert_eq!(parse_self_hosted_repo_scope("git.example.com:8443/team/repo.git"),Some("git.example.com:8443/team/repo".into()));
        assert_eq!(parse_self_hosted_repo_scope("github.com/openai/openai"),None);
    }
}
