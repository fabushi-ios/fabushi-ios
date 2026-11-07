use std::sync::Arc;

use crate::extensions::attachments::generate_image_service::{
    GenerateImageAuth, PersistGeneratedImage, SandGenerateImageExecutionOptions,
    SandGenerateImageService,
};
use crate::extensions::inference::provider_session::ProviderSessionError;
use crate::runner::tools::sand_generate_image_tool::{
    GenerateImageToolExecutor, GenerateImageToolOptions, GeneratedImageToolOutput,
};

pub struct ProductionGenerateImageToolExecutor {
    auth: Arc<dyn GenerateImageAuth>,
    persist: PersistGeneratedImage,
}

impl ProductionGenerateImageToolExecutor {
    pub fn new(
        auth: Arc<dyn GenerateImageAuth>,
        persist: PersistGeneratedImage,
    ) -> Self {
        Self { auth, persist }
    }
}

impl GenerateImageToolExecutor for ProductionGenerateImageToolExecutor {
    fn generate_with_options(
        &self,
        description: &str,
        reference_images: &[(String, String)],
        options: &GenerateImageToolOptions,
    ) -> Result<GeneratedImageToolOutput, ProviderSessionError> {
        let service = SandGenerateImageService::production(
            Arc::clone(&self.auth),
            Arc::clone(&self.persist),
        )
        .map_err(ProviderSessionError::Tool)?;
        let service_options = SandGenerateImageExecutionOptions {
            resolution: options.resolution.as_ref().map(|r| r.as_str().to_string()),
        };
        let (persisted_path, image_data_base64) = service
            .generate_with_options(description, reference_images, &service_options)
            .map_err(|error| ProviderSessionError::Tool(error.to_string()))?;
        Ok(GeneratedImageToolOutput {
            persisted_path,
            image_data_base64,
        })
    }
}
