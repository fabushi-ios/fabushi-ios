use std::io::Cursor;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use mahayana_host_runtime::extensions::attachments::safe_link_preview_fetch::{
    MAX_URL_LENGTH, SandLinkPreviewError, get_safe_link_preview_connection_target_with,
    is_blocked_hostname, is_blocked_link_preview_ip, normalize_hostname,
    parse_safe_link_preview_url, read_limited_body, resolve_safe_link_preview_redirect,
};

#[test]
fn link_preview_url_policy_matches_frozen_https_and_hostname_rules() {
    assert_eq!(normalize_hostname("Example.COM."), "example.com");
    assert!(is_blocked_hostname("localhost"));
    assert!(is_blocked_hostname("service"));
    assert!(is_blocked_hostname("box.internal"));
    assert!(is_blocked_hostname("foo.svc"));
    assert!(is_blocked_hostname("hidden.onion"));
    assert!(!is_blocked_hostname("example.com"));

    assert!(matches!(
        parse_safe_link_preview_url("http://example.com"),
        Err(SandLinkPreviewError::RequiresHttps)
    ));
    assert!(matches!(
        parse_safe_link_preview_url("https://user:pass@example.com"),
        Err(SandLinkPreviewError::Credentials)
    ));
    assert!(matches!(
        parse_safe_link_preview_url("https://example.com:8443"),
        Err(SandLinkPreviewError::CustomPort)
    ));
    assert!(matches!(
        parse_safe_link_preview_url("https://localhost"),
        Err(SandLinkPreviewError::NonPublicHostname)
    ));
    assert!(matches!(
        parse_safe_link_preview_url("https://127.0.0.1"),
        Err(SandLinkPreviewError::NonPublicIp)
    ));
    assert!(parse_safe_link_preview_url("https://example.com/path?q=1").is_ok());

    let too_long = format!("https://example.com/{}", "a".repeat(MAX_URL_LENGTH));
    assert!(matches!(
        parse_safe_link_preview_url(&too_long),
        Err(SandLinkPreviewError::UrlTooLong)
    ));
}

#[test]
fn link_preview_ip_blocklist_matches_frozen_private_reserved_and_public_ranges() {
    for address in [
        IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)),
        IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
        IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(172, 31, 255, 255)),
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)),
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 7)),
        IpAddr::V4(Ipv4Addr::new(224, 0, 0, 1)),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        "fc00::1".parse().unwrap(),
        "fe80::1".parse().unwrap(),
        "2001:db8::1".parse().unwrap(),
        "ff02::1".parse().unwrap(),
    ] {
        assert!(is_blocked_link_preview_ip(address), "{address} should be blocked");
    }

    assert!(!is_blocked_link_preview_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    assert!(!is_blocked_link_preview_ip("2606:4700:4700::1111".parse().unwrap()));
}

#[test]
fn dns_resolution_rejects_any_non_public_answer_and_pins_public_ipv4_first() {
    let url = parse_safe_link_preview_url("https://example.com/page").unwrap();
    let blocked = get_safe_link_preview_connection_target_with(&url, |_| {
        Ok(vec![
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)),
        ])
    });
    assert!(matches!(blocked, Err(SandLinkPreviewError::NonPublicIp)));

    let target = get_safe_link_preview_connection_target_with(&url, |_| {
        Ok(vec![
            "2606:4700:4700::1111".parse().unwrap(),
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        ])
    })
    .unwrap();
    assert_eq!(target.connect_host, "1.1.1.1");
    assert_eq!(target.host_header, "example.com");
    assert_eq!(target.servername.as_deref(), Some("example.com"));
    assert_eq!(target.socket_addr.ip(), IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));

    let empty = get_safe_link_preview_connection_target_with(&url, |_| Ok(Vec::new()));
    assert!(matches!(
        empty,
        Err(SandLinkPreviewError::HostnameDidNotResolve)
    ));
}

#[test]
fn redirect_resolution_revalidates_destination_and_never_relaxes_ssrf_policy() {
    let current = parse_safe_link_preview_url("https://example.com/a/b").unwrap();
    assert_eq!(
        resolve_safe_link_preview_redirect(&current, "../next")
            .unwrap()
            .as_str(),
        "https://example.com/next"
    );
    assert!(matches!(
        resolve_safe_link_preview_redirect(&current, "http://example.com"),
        Err(SandLinkPreviewError::RequiresHttps)
    ));
    assert!(matches!(
        resolve_safe_link_preview_redirect(&current, "https://127.0.0.1/private"),
        Err(SandLinkPreviewError::NonPublicIp)
    ));
}

#[test]
fn response_body_limit_truncates_only_when_explicitly_allowed() {
    let bytes = vec![b'x'; 17];
    let mut strict = Cursor::new(bytes.clone());
    assert!(matches!(
        read_limited_body(&mut strict, 16, false),
        Err(SandLinkPreviewError::BodyTooLarge)
    ));

    let mut truncated = Cursor::new(bytes);
    assert_eq!(
        read_limited_body(&mut truncated, 16, true).unwrap().len(),
        16
    );
}
