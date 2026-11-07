use std::env;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HOST_BUNDLE_BUCKET: &str = "public-asphr-vm-daemon-bucket";
pub const HOST_BUNDLE_REGION: &str = "us-east-1";
pub const HOST_BUNDLE_PREFIX: &str = "sand-host-bundle";
pub const DEFAULT_BASE_URL: &str =
    "https://public-asphr-vm-daemon-bucket.s3.us-east-1.amazonaws.com/sand-host-bundle";
pub const LATEST_VERSION_FILE: &str = "sand-host-bundle-latest.version";
pub const VERSION_CACHE_TTL_MS: u64 = 10 * 60_000;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SandHostBundleSourceError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBundleHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HostBundleHttpResponse {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

pub trait HostBundleFetcher: Send + Sync {
    fn fetch(&self, url: &str) -> Result<HostBundleHttpResponse, String>;
}

/// Shipping HTTP transport for the frozen Host bundle source. Keeping this
/// behind HostBundleFetcher preserves deterministic contract tests while the
/// production Host uses rustls-backed reqwest and never shells out to curl.
#[derive(Clone)]
pub struct ReqwestHostBundleFetcher {
    client: reqwest::blocking::Client,
}

impl ReqwestHostBundleFetcher {
    pub fn new() -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|error| format!("could not build host bundle HTTP client: {error}"))?;
        Ok(Self { client })
    }
}

impl HostBundleFetcher for ReqwestHostBundleFetcher {
    fn fetch(&self, url: &str) -> Result<HostBundleHttpResponse, String> {
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|error| format!("host bundle GET {url} failed: {error}"))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .map_err(|error| format!("host bundle GET {url} body failed: {error}"))?
            .to_vec();
        Ok(HostBundleHttpResponse { status, body })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedVersion {
    version: String,
    at: u64,
}

#[derive(Default)]
pub struct HostBundleVersionCache {
    cached: Mutex<Option<CachedVersion>>,
}

impl HostBundleVersionCache {
    pub fn clear(&self) {
        *self.cached.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    pub fn fetch_latest(
        &self,
        fetcher: &dyn HostBundleFetcher,
        base: &str,
        now_ms: u64,
    ) -> Option<String> {
        if let Some(cached) = self
            .cached
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            if now_ms.saturating_sub(cached.at) < VERSION_CACHE_TTL_MS {
                return Some(cached.version);
            }
        }
        let response = fetcher.fetch(&latest_host_bundle_version_url(base)).ok()?;
        if !response.ok() {
            return None;
        }
        let raw = std::str::from_utf8(&response.body).ok()?.trim();
        if !is_short_git_sha(raw) {
            return None;
        }
        let version = raw.to_string();
        *self.cached.lock().unwrap_or_else(|p| p.into_inner()) = Some(CachedVersion {
            version: version.clone(),
            at: now_ms,
        });
        Some(version)
    }
}

static VERSION_CACHE: OnceLock<HostBundleVersionCache> = OnceLock::new();

fn global_cache() -> &'static HostBundleVersionCache {
    VERSION_CACHE.get_or_init(HostBundleVersionCache::default)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn is_short_git_sha(value: &str) -> bool {
    (7..=40).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn host_bundle_base_url_from(value: Option<&str>) -> String {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    match value {
        Some(value) => value.trim_end_matches('/').to_string(),
        None => DEFAULT_BASE_URL.to_string(),
    }
}

pub fn host_bundle_base_url() -> String {
    host_bundle_base_url_from(env::var("SAND_HOST_BUNDLE_S3_BASE_URL").ok().as_deref())
}

pub fn latest_host_bundle_version_url(base: &str) -> String {
    format!("{}/{LATEST_VERSION_FILE}", base.trim_end_matches('/'))
}

pub fn host_bundle_tarball_url(version: &str, base: &str) -> String {
    format!(
        "{}/{}-{version}.tgz",
        base.trim_end_matches('/'),
        HOST_BUNDLE_PREFIX
    )
}

pub fn clear_host_bundle_version_cache() {
    global_cache().clear();
}

pub fn fetch_latest_host_bundle_version(fetcher: &dyn HostBundleFetcher) -> Option<String> {
    let base = host_bundle_base_url();
    global_cache().fetch_latest(fetcher, &base, now_ms())
}

pub fn fetch_host_bundle_tarball_with_base(
    fetcher: &dyn HostBundleFetcher,
    version: &str,
    base: &str,
) -> Result<Vec<u8>, SandHostBundleSourceError> {
    if !is_short_git_sha(version) {
        return Err(SandHostBundleSourceError(format!(
            "sand host bundle: refusing malformed version \"{version}\""
        )));
    }
    let url = host_bundle_tarball_url(version, base);
    let response = fetcher
        .fetch(&url)
        .map_err(SandHostBundleSourceError)?;
    if !response.ok() {
        return Err(SandHostBundleSourceError(format!(
            "sand host bundle: fetch {url} failed (status {})",
            response.status
        )));
    }
    if response.body.is_empty() {
        return Err(SandHostBundleSourceError(format!(
            "sand host bundle: fetched empty tarball from {url}"
        )));
    }
    Ok(response.body)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBundleSource {
    pub version: String,
    base_url: String,
}

impl HostBundleSource {
    pub fn load_bundle_bytes(
        &self,
        fetcher: &dyn HostBundleFetcher,
    ) -> Result<Vec<u8>, SandHostBundleSourceError> {
        fetch_host_bundle_tarball_with_base(fetcher, &self.version, &self.base_url)
    }
}

pub fn resolve_host_bundle_source_with(
    fetcher: &dyn HostBundleFetcher,
    cache: &HostBundleVersionCache,
    base: &str,
    now_ms: u64,
) -> Option<HostBundleSource> {
    let version = cache.fetch_latest(fetcher, base, now_ms)?;
    Some(HostBundleSource {
        version,
        base_url: base.trim_end_matches('/').to_string(),
    })
}
