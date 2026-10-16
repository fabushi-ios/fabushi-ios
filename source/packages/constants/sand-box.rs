use url::Url;

pub const SAND_BOX_IMAGE_TAG_PREFIX: &str = "sand-box-";
pub const SAND_BOX_IMAGE_TAG_LATEST: &str = "sand-box-latest";
pub const SAND_BOX_PRIMARY_NOVNC_PORT: u16 = 6080;
pub const SAND_BOX_FORK_NOVNC_PORT: u16 = 6081;
pub const SAND_SPECIAL_TREATMENT_NOVNC_PATH: &str = "sand-special-treatment-v1/vnc.html";

pub fn is_short_git_sha(value: &str) -> bool {
    (7..=40).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out=String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte,b'-'|b'_'|b'.'|b'!'|b'~'|b'*'|b'\''|b'('|b')') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte>>4) as usize] as char);
            out.push(HEX[(byte&0x0f) as usize] as char);
        }
    }
    out
}

pub fn build_sand_box_no_vnc_url(proxy_base_url: &str, network_token: &str, token: Option<&str>, special_treatment: bool) -> String {
    let wake="resume_lower_s=900&resume_upper_s=18000";
    let token_param=token.map(|value| format!("token={value}&")).unwrap_or_default();
    let websockify_path=format!("websockify?{token_param}network_token={network_token}&{wake}");
    let viewer_path=if special_treatment { SAND_SPECIAL_TREATMENT_NOVNC_PATH } else { "vnc.html" };
    format!("{proxy_base_url}/{viewer_path}?network_token={network_token}&{wake}&path={}", encode_uri_component(&websockify_path))
}

pub fn is_sand_special_treatment_no_vnc_url(value: &str) -> bool {
    Url::parse(value).ok().is_some_and(|url| url.path().ends_with(&format!("/{SAND_SPECIAL_TREATMENT_NOVNC_PATH}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_only_lowercase_reference_short_shas() {
        assert!(is_short_git_sha("abcdef1"));
        assert!(is_short_git_sha(&"a".repeat(40)));
        assert!(!is_short_git_sha("ABCDEF1"));
        assert!(!is_short_git_sha("abcdef"));
        assert!(!is_short_git_sha("abcdefg"));
    }
    #[test]
    fn builds_reference_novnc_query_and_special_path() {
        let url=build_sand_box_no_vnc_url("https://proxy.example","net",Some("a b"),false);
        assert_eq!(url,"https://proxy.example/vnc.html?network_token=net&resume_lower_s=900&resume_upper_s=18000&path=websockify%3Ftoken%3Da%20b%26network_token%3Dnet%26resume_lower_s%3D900%26resume_upper_s%3D18000");
        let special=build_sand_box_no_vnc_url("https://proxy.example","net",None,true);
        assert!(is_sand_special_treatment_no_vnc_url(&special));
        assert!(!is_sand_special_treatment_no_vnc_url("not a url"));
    }
}
