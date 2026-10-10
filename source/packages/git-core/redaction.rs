fn is_trailing_url_wrapper(character: char) -> bool {
    matches!(character, ')' | '"' | '\'' | '<' | '>' | ',' | '.' | ';' | ':' | ']' | '}')
}

fn split_trailing_url_wrappers(candidate:&str)->(&str,&str) {
    let mut end=candidate.len();
    while end>0 {
        let Some(character)=candidate[..end].chars().next_back() else { break; };
        if !is_trailing_url_wrapper(character){break;}
        end-=character.len_utf8();
    }
    (&candidate[..end],&candidate[end..])
}

fn redact_http_url_candidate(candidate:&str)->String {
    let (url,wrappers)=split_trailing_url_wrappers(candidate);
    let suffix_start=url.find(['?','#']).unwrap_or(url.len());
    let clean=&url[..suffix_start];
    let authority_start=clean.find("//").map(|index|index+2).unwrap_or(1);
    let authority_tail=&clean[authority_start..];
    let delimiter=authority_tail.find(['/', '?', '#']).unwrap_or(authority_tail.len());
    let authority_end=authority_start+delimiter;
    let userinfo=clean[authority_start..authority_end].rfind('@');
    match userinfo {
        None=>format!("{clean}{wrappers}"),
        Some(relative)=> {
            let userinfo_end=authority_start+relative;
            format!("{}{}{}", &clean[..authority_start], &clean[userinfo_end+1..], wrappers)
        }
    }
}

pub fn redact_http_url_userinfo(value:&str)->String {
    let lower=value.to_ascii_lowercase();
    let bytes=value.as_bytes();
    let mut out=String::new();
    let mut index=0usize;
    while index<value.len() {
        let http=lower[index..].starts_with("http://");
        let https=lower[index..].starts_with("https://");
        if !(http||https) {
            let ch=value[index..].chars().next().unwrap();
            out.push(ch);
            index+=ch.len_utf8();
            continue;
        }
        let start=index;
        let mut end=index;
        while end<bytes.len() {
            let ch=value[end..].chars().next().unwrap();
            if ch.is_whitespace(){break;}
            end+=ch.len_utf8();
        }
        out.push_str(&redact_http_url_candidate(&value[start..end]));
        index=end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_userinfo_query_and_fragment_while_preserving_wrappers() {
        assert_eq!(
            redact_http_url_userinfo("clone https://user:pass@example.com/org/repo.git?token=x#frag), next"),
            "clone https://example.com/org/repo.git), next"
        );
        assert_eq!(
            redact_http_url_userinfo("https://example.com/repo.git?x=1"),
            "https://example.com/repo.git"
        );
    }

    #[test]
    fn handles_multiple_urls_case_insensitively() {
        assert_eq!(
            redact_http_url_userinfo("HTTP://u@a.test/x HTTPS://v:b@b.test/y"),
            "HTTP://a.test/x HTTPS://b.test/y"
        );
    }
}
