use std::sync::Arc;

use crate::extensions::inference::production::ProductionInferenceExtension;
use crate::extensions::inference::provider_session::ProviderSessionError;
use crate::runner::tools::sand_web_tools::{
    WebToolExecutor, render_web_fetch_result, render_web_search_result,
};
use serde_json::Value;

pub struct ProductionWebToolExecutor {
    inference: Arc<ProductionInferenceExtension>,
    model_id: String,
}

impl ProductionWebToolExecutor {
    pub fn new(
        inference: Arc<ProductionInferenceExtension>,
        model_id: impl Into<String>,
    ) -> Self {
        Self {
            inference,
            model_id: model_id.into(),
        }
    }
}

impl WebToolExecutor for ProductionWebToolExecutor {
    fn search(
        &self,
        search_term: &str,
        explanation: Option<&str>,
    ) -> Result<Value, ProviderSessionError> {
        let service = self
            .inference
            .create_web_search(self.model_id.clone(), None)
            .map_err(ProviderSessionError::Tool)?;
        let response = service
            .search(search_term, explanation.map(str::to_string))
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        let documents = response
            .documents
            .iter()
            .map(|document| (
                document.title.as_str(),
                document.url.as_str(),
                document.text.as_str(),
            ))
            .collect::<Vec<_>>();
        Ok(render_web_search_result(
            response.answer.as_deref(),
            &documents,
        ))
    }

    fn fetch(&self, url: &str) -> Result<Value, ProviderSessionError> {
        let service = self
            .inference
            .create_web_fetch(None)
            .map_err(ProviderSessionError::Tool)?;
        let response = service
            .fetch(url)
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        Ok(render_web_fetch_result(
            url,
            response.content.as_deref(),
            response.error.as_deref(),
            response.is_timeout,
        ))
    }
}
