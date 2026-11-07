use serde_json::Value;

pub const PRIMARY_NOVNC_PORT: u16 = 6080;
pub const FORK_NOVNC_PORT: u16 = 6081;
pub const SPECIAL_TREATMENT_NOVNC_PATH: &str = "sand-special-treatment-v1/vnc.html";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VncProxyDescriptor {
    pub primary_url: String,
    pub fork_base_url: String,
    pub network_token: String,
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2])) {
                output.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        output.push(if bytes[index] == b'+' { b' ' } else { bytes[index] });
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn percent_encode(value: &str) -> String {
    let mut output = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b'.' | b'~') {
            output.push(*byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(&mut output, "%{:02X}", byte);
        }
    }
    output
}

fn split_url(value: &str) -> Option<(&str, &str, &str)> {
    let (_, remainder) = value.split_once("://")?;
    let authority_end = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let path_and_query = remainder.get(authority_end..).unwrap_or("/");
    let (host, port) = authority.rsplit_once(':')?;
    if host.is_empty() || port.is_empty() {
        return None;
    }
    Some((host, port, path_and_query))
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (candidate, value) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(candidate) == key).then(|| percent_decode(value))
    })
}

fn fork_display_token(path_and_query: &str) -> Option<String> {
    let (_, query) = path_and_query.split_once('?')?;
    let encoded_path = query_value(query, "path")?;
    let (_, nested_query) = encoded_path.split_once('?')?;
    query_value(nested_query, "token").filter(|value| !value.is_empty())
}

fn primary_uses_special_treatment(primary_url: &str) -> bool {
    primary_url
        .split('?')
        .next()
        .is_some_and(|path| path.ends_with(&format!("/{SPECIAL_TREATMENT_NOVNC_PATH}")))
}

fn build_fork_url(descriptor: &VncProxyDescriptor, token: Option<&str>) -> String {
    let wake = "resume_lower_s=900&resume_upper_s=18000";
    let token_parameter = token
        .filter(|value| !value.is_empty())
        .map(|value| format!("token={value}&"))
        .unwrap_or_default();
    let websockify_path = format!(
        "websockify?{token_parameter}network_token={}&{wake}",
        descriptor.network_token
    );
    let viewer_path = if primary_uses_special_treatment(&descriptor.primary_url) {
        SPECIAL_TREATMENT_NOVNC_PATH
    } else {
        "vnc.html"
    };
    format!(
        "{}/{viewer_path}?network_token={}&{wake}&path={}",
        descriptor.fork_base_url.trim_end_matches('/'),
        descriptor.network_token,
        percent_encode(&websockify_path)
    )
}

pub fn proxify_box_vnc_url(vnc_url: &str, descriptor: &VncProxyDescriptor) -> String {
    let Some((host, port, path_and_query)) = split_url(vnc_url) else {
        return vnc_url.to_string();
    };
    if !matches!(host, "127.0.0.1" | "localhost") {
        return vnc_url.to_string();
    }
    let path = path_and_query.split('?').next().unwrap_or_default();
    if !path.ends_with("/vnc.html") {
        return vnc_url.to_string();
    }
    match port.parse::<u16>() {
        Ok(PRIMARY_NOVNC_PORT) => descriptor.primary_url.clone(),
        Ok(FORK_NOVNC_PORT) => build_fork_url(descriptor, fork_display_token(path_and_query).as_deref()),
        _ => vnc_url.to_string(),
    }
}

pub fn proxify_forever_box_status(status: &Value, descriptor: Option<&VncProxyDescriptor>) -> Value {
    let Some(descriptor) = descriptor else {
        return status.clone();
    };
    let mut output = status.clone();
    let Some(object) = output.as_object_mut() else {
        return output;
    };

    if let Some(Value::String(url)) = object.get_mut("vncUrl") {
        *url = proxify_box_vnc_url(url, descriptor);
    }
    if let Some(Value::Array(windows)) = object.get_mut("windows") {
        for window in windows {
            if let Some(url) = window
                .get_mut("vncUrl")
                .and_then(|value| value.as_str())
                .map(str::to_string)
            {
                if let Some(slot) = window.get_mut("vncUrl") {
                    *slot = Value::String(proxify_box_vnc_url(&url, descriptor));
                }
            }
        }
    }
    output
}
