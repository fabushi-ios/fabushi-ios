use crate::net::http_client::{HttpClient, HttpClientError};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// Percent-encodes query parameter values strictly according to RFC 3986.
pub(crate) fn percent_encode_rfc3986(input: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0F) as usize] as char);
        }
    }
    out
}

pub(crate) fn extract_api_error(status: StatusCode, body: &str) -> HttpClientError {
    let trimmed = body.trim();
    let msg = serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .and_then(|v| {
            v.get("message")
                .or_else(|| v.get("error"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| {
            if trimmed.is_empty() {
                status
                    .canonical_reason()
                    .unwrap_or("Unknown HTTP error")
                    .to_string()
            } else {
                trimmed.to_string()
            }
        });
    match status {
        StatusCode::UNAUTHORIZED => HttpClientError::Unauthorized(msg),
        StatusCode::CONFLICT => HttpClientError::Conflict(msg),
        _ => HttpClientError::Api {
            status: status.as_u16(),
            message: msg,
        },
    }
}

pub(crate) fn decode_json_response<T: DeserializeOwned>(
    status: StatusCode,
    body: String,
) -> Result<T, HttpClientError> {
    if !status.is_success() {
        return Err(extract_api_error(status, &body));
    }
    Ok(serde_json::from_str(&body)?)
}

/// Decodes either a bare JSON array `[T, ...]` or a wrapper object `{ "<field>": [T, ...] }`.
pub(crate) fn decode_vec_or_field<T: DeserializeOwned>(
    status: StatusCode,
    body: String,
    candidate_fields: &[&str],
) -> Result<Vec<T>, HttpClientError> {
    if !status.is_success() {
        return Err(extract_api_error(status, &body));
    }
    let value: serde_json::Value = serde_json::from_str(&body)?;
    if value.is_array() {
        return Ok(serde_json::from_value(value)?);
    }
    if let Some(obj) = value.as_object() {
        for field in candidate_fields {
            if let Some(arr) = obj.get(*field) {
                return Ok(serde_json::from_value(arr.clone())?);
            }
        }
    }
    Ok(serde_json::from_value(value)?)
}

impl HttpClient {
    pub(crate) fn endpoint_url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/{}", self.base_url, path)
        }
    }

    pub(crate) async fn request_json<B: Serialize + ?Sized, R: DeserializeOwned + Send + 'static>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&B>,
    ) -> Result<R, HttpClientError> {
        let client = self.client.clone();
        let url = self.endpoint_url(path);
        let bearer = token.map(|t| t.to_string());
        let json_body = match body {
            Some(b) => Some(serde_json::to_value(b)?),
            None => None,
        };
        self.run_on_tokio(async move {
            let mut req = client.request(method, &url);
            if let Some(ref t) = bearer {
                req = req.bearer_auth(t);
            }
            if let Some(ref val) = json_body {
                req = req.json(val);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            decode_json_response(status, text)
        })
        .await
    }

    pub(crate) async fn request_vec_or_field<T: DeserializeOwned + Send + 'static>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        fields: &'static [&'static str],
    ) -> Result<Vec<T>, HttpClientError> {
        let client = self.client.clone();
        let url = self.endpoint_url(path);
        let bearer = token.map(|t| t.to_string());
        self.run_on_tokio(async move {
            let mut req = client.request(method, &url);
            if let Some(ref t) = bearer {
                req = req.bearer_auth(t);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            decode_vec_or_field(status, text, fields)
        })
        .await
    }

    pub(crate) async fn request_empty<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<&B>,
    ) -> Result<(), HttpClientError> {
        let client = self.client.clone();
        let url = self.endpoint_url(path);
        let bearer = token.map(|t| t.to_string());
        let json_body = match body {
            Some(b) => Some(serde_json::to_value(b)?),
            None => None,
        };
        self.run_on_tokio(async move {
            let mut req = client.request(method, &url);
            if let Some(ref t) = bearer {
                req = req.bearer_auth(t);
            }
            if let Some(ref val) = json_body {
                req = req.json(val);
            }
            let resp = req.send().await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(extract_api_error(status, &text));
            }
            Ok(())
        })
        .await
    }

    pub(crate) async fn request_bytes(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        range_header: Option<&str>,
    ) -> Result<(u16, Vec<u8>, Option<String>), HttpClientError> {
        let client = self.client.clone();
        let url = self.endpoint_url(path);
        let bearer = token.map(|t| t.to_string());
        let range = range_header.map(|r| r.to_string());
        self.run_on_tokio(async move {
            let mut req = client.request(method, &url);
            if let Some(ref t) = bearer {
                req = req.bearer_auth(t);
            }
            if let Some(ref r) = range {
                req = req.header(reqwest::header::RANGE, r);
            }
            let resp = req.send().await?;
            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(extract_api_error(status, &text));
            }
            let content_range = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string());
            let bytes = resp.bytes().await?.to_vec();
            Ok((status.as_u16(), bytes, content_range))
        })
        .await
    }
}
