use super::provider_session::RoutedProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceRoute {
    Cursor,
    Routed(RoutedProvider),
}

pub fn resolve_inference_route(provider: RoutedProvider) -> InferenceRoute {
    match provider {
        RoutedProvider::Cursor => InferenceRoute::Cursor,
        provider => InferenceRoute::Routed(provider),
    }
}

pub fn authorize_routed_provider_request(
    route: InferenceRoute,
    requested: RoutedProvider,
) -> Result<RoutedProvider, String> {
    match route {
        InferenceRoute::Cursor if requested == RoutedProvider::Cursor => Ok(RoutedProvider::Cursor),
        InferenceRoute::Cursor => Err(format!(
            "routed provider request does not match Host inference settings: requested={} configured=cursor",
            requested.as_str(),
        )),
        InferenceRoute::Routed(configured) if configured == requested => Ok(configured),
        InferenceRoute::Routed(configured) => Err(format!(
            "routed provider request does not match Host inference settings: requested={} configured={}",
            requested.as_str(),
            configured.as_str(),
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InferenceUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
}

pub trait InferenceSettings: Send + Sync {
    fn inference_provider(&self) -> RoutedProvider;
    fn record_inference_usage(&self, provider: RoutedProvider, usage: InferenceUsage);
}

pub struct HostInferenceService<S> {
    settings: S,
}

impl<S> HostInferenceService<S>
where
    S: InferenceSettings,
{
    pub fn new(settings: S) -> Self {
        Self { settings }
    }

    pub fn route(&self) -> InferenceRoute {
        resolve_inference_route(self.settings.inference_provider())
    }

    pub fn record_usage(&self, provider: RoutedProvider, usage: InferenceUsage) {
        self.settings.record_inference_usage(provider, usage);
    }
}

pub fn usage_from_extended_fields(
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
) -> InferenceUsage {
    fn valid(value: Option<i64>) -> Option<u64> {
        value.and_then(|value| u64::try_from(value).ok())
    }

    InferenceUsage {
        input_tokens: valid(input_tokens),
        output_tokens: valid(output_tokens),
        cache_read_tokens: valid(cache_read_tokens),
        cache_write_tokens: valid(cache_write_tokens),
    }
}
