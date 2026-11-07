use std::env;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use prost::Message;
use uuid::Uuid;

use crate::cursor_backend::{
    resolve_sand_ghost_mode_header, send_cursor_unary_with_request_id,
};
use crate::extensions::auth::credential_renewer::get_configured_backend_url;
use crate::extensions::auth::extension::HostAuthExtension;

pub const RUN_GENERATE_IMAGE_PATH: &str = "/aiserver.v1.AiService/RunGenerateImage";
pub const SAND_DEFAULT_MODEL_ID: &str = "grok-4.5";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub image_data: String,
    pub mime_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedImage {
    pub absolute_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GenerateImageBackendError {
    #[error("{0}")]
    Generate(String),
    #[error("{0}")]
    ModelRestricted(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandGenerateImageError {
    #[error("{0}")]
    Generate(String),
    #[error("{0}")]
    ModelRestricted(String),
    #[error("Failed to save the generated image into the agent's media store.")]
    Persist,
    #[error("Generated image data was not valid base64.")]
    InvalidBase64,
}

pub trait GenerateImageAuth: Send + Sync {
    fn get_access_token(&self) -> Result<String, String>;
    fn get_machine_id(&self) -> Result<String, String>;
}

impl GenerateImageAuth for HostAuthExtension {
    fn get_access_token(&self) -> Result<String, String> {
        HostAuthExtension::get_access_token(self).map_err(|error| error.to_string())
    }

    fn get_machine_id(&self) -> Result<String, String> {
        HostAuthExtension::get_machine_id(self).map_err(|error| error.to_string())
    }
}

pub type GenerateImageBackend = Arc<
    dyn Fn(
            &str,
            &[(String, String)],
        ) -> Result<GeneratedImage, GenerateImageBackendError>
        + Send
        + Sync,
>;
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandGenerateImageExecutionOptions {
    pub resolution: Option<String>,
}

pub type GenerateImageBackendWithOptions = Arc<
    dyn Fn(
            &str,
            &[(String, String)],
            &SandGenerateImageExecutionOptions,
        ) -> Result<GeneratedImage, GenerateImageBackendError>
        + Send
        + Sync,
>;
pub type PersistGeneratedImage =
    Arc<dyn Fn(&[u8], &str) -> Result<Option<PersistedImage>, String> + Send + Sync>;
pub type GenerateImageRequestIdObserver = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct CursorGenerateImageOptions {
    pub auth: Arc<dyn GenerateImageAuth>,
    pub backend_url: String,
    pub model_id: String,
    pub on_request_id: Option<GenerateImageRequestIdObserver>,
}

impl CursorGenerateImageOptions {
    pub fn production(auth: Arc<dyn GenerateImageAuth>) -> Result<Self, String> {
        Ok(Self {
            auth,
            backend_url: get_configured_backend_url().map_err(|error| error.to_string())?,
            model_id: configured_generate_image_model_id(),
            on_request_id: None,
        })
    }
}

pub fn configured_generate_image_model_id() -> String {
    if let Ok(value) = env::var("SAND_AGENT_MODEL") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    let mut default_registry = std::collections::HashMap::new();
    let mut default_models = std::collections::HashMap::new();
    default_models.insert("image".to_string(), SAND_DEFAULT_MODEL_ID.to_string());
    default_registry.insert(
        "cursor".to_string(),
        crate::media_understanding_defaults::MediaUnderstandingProviderMetadata {
            id: "cursor".to_string(),
            capabilities: vec![crate::media_understanding_defaults::MediaUnderstandingCapability::Image],
            default_models,
            auto_priority: std::collections::HashMap::new(),
        },
    );
    crate::media_understanding_defaults::resolve_default_media_model(
        "cursor",
        crate::media_understanding_defaults::MediaUnderstandingCapability::Image,
        &default_registry,
        None,
        false,
    )
    .unwrap_or_else(|| SAND_DEFAULT_MODEL_ID.to_string())
}

#[derive(Clone, PartialEq, Message)]
struct ProtoGenerateImageReferenceImage {
    #[prost(string, tag = "1")]
    data: String,
    #[prost(string, tag = "2")]
    mime_type: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunGenerateImageRequest {
    #[prost(string, tag = "1")]
    description: String,
    #[prost(message, repeated, tag = "2")]
    reference_images: Vec<ProtoGenerateImageReferenceImage>,
    #[prost(string, tag = "3")]
    model_id: String,
    #[prost(bool, tag = "4")]
    max_mode: bool,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunGenerateImageSuccess {
    #[prost(string, tag = "1")]
    image_data: String,
    #[prost(string, tag = "2")]
    mime_type: String,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunGenerateImageFailure {
    #[prost(string, tag = "1")]
    error: String,
    #[prost(bool, tag = "2")]
    model_restricted: bool,
}

#[derive(Clone, PartialEq, Message)]
struct ProtoRunGenerateImageResponse {
    #[prost(oneof = "proto_run_generate_image_response::Result", tags = "1, 2")]
    result: Option<proto_run_generate_image_response::Result>,
}

mod proto_run_generate_image_response {
    #[derive(Clone, PartialEq, ::prost::Oneof)]
    pub enum Result {
        #[prost(message, tag = "1")]
        Success(super::ProtoRunGenerateImageSuccess),
        #[prost(message, tag = "2")]
        Error(super::ProtoRunGenerateImageFailure),
    }
}

pub fn create_cursor_generate_image_backend(
    options: CursorGenerateImageOptions,
) -> GenerateImageBackend {
    Arc::new(move |description, reference_images| {
        let access_token = options
            .auth
            .get_access_token()
            .map_err(GenerateImageBackendError::Generate)?;
        let machine_id = options
            .auth
            .get_machine_id()
            .map_err(GenerateImageBackendError::Generate)?;
        let ghost_mode = resolve_sand_ghost_mode_header(
            &options.backend_url,
            &access_token,
            &machine_id,
        );
        let request_id = Uuid::new_v4().to_string();
        if let Some(observer) = options.on_request_id.as_ref() {
            observer(&request_id);
        }

        let request = ProtoRunGenerateImageRequest {
            description: description.to_string(),
            reference_images: reference_images
                .iter()
                .map(|(data, mime_type)| ProtoGenerateImageReferenceImage {
                    data: data.clone(),
                    mime_type: mime_type.clone(),
                })
                .collect(),
            model_id: options.model_id.clone(),
            max_mode: true,
        };
        let bytes = send_cursor_unary_with_request_id(
            &options.backend_url,
            &access_token,
            &machine_id,
            RUN_GENERATE_IMAGE_PATH,
            &request.encode_to_vec(),
            None,
            ghost_mode,
            &request_id,
        )
        .map_err(|error| GenerateImageBackendError::Generate(error.to_string()))?;
        let response = ProtoRunGenerateImageResponse::decode(bytes.as_slice())
            .map_err(|error| {
                GenerateImageBackendError::Generate(format!(
                    "Image generation returned an invalid protobuf response: {error}"
                ))
            })?;
        match response.result {
            Some(proto_run_generate_image_response::Result::Success(success)) => {
                Ok(GeneratedImage {
                    image_data: success.image_data,
                    mime_type: success.mime_type,
                })
            }
            Some(proto_run_generate_image_response::Result::Error(error))
                if error.model_restricted =>
            {
                Err(GenerateImageBackendError::ModelRestricted(error.error))
            }
            Some(proto_run_generate_image_response::Result::Error(error)) => {
                Err(GenerateImageBackendError::Generate(error.error))
            }
            None => Err(GenerateImageBackendError::Generate(
                "Image generation returned no result.".into(),
            )),
        }
    })
}

#[derive(Clone)]
pub struct SandGenerateImageService {
    generate: GenerateImageBackendWithOptions,
    persist: PersistGeneratedImage,
}

impl SandGenerateImageService {
    pub fn new(generate: GenerateImageBackend, persist: PersistGeneratedImage) -> Self {
        let with_options: GenerateImageBackendWithOptions =
            Arc::new(move |description, reference_images, options| {
                if let Some(resolution) = options.resolution.as_ref() {
                    return Err(GenerateImageBackendError::Generate(format!(
                        "Backend provider does not support resolution parameter: {resolution}"
                    )));
                }
                generate(description, reference_images)
            });
        Self {
            generate: with_options,
            persist,
        }
    }

    pub fn new_with_options(
        generate: GenerateImageBackendWithOptions,
        persist: PersistGeneratedImage,
    ) -> Self {
        Self { generate, persist }
    }

    pub fn production(
        auth: Arc<dyn GenerateImageAuth>,
        persist: PersistGeneratedImage,
    ) -> Result<Self, String> {
        let options = CursorGenerateImageOptions::production(auth)?;
        Ok(Self::new(
            create_cursor_generate_image_backend(options),
            persist,
        ))
    }

    pub fn generate(
        &self,
        description: &str,
        reference_images: &[(String, String)],
    ) -> Result<(String, String), SandGenerateImageError> {
        self.generate_with_options(
            description,
            reference_images,
            &SandGenerateImageExecutionOptions::default(),
        )
    }

    pub fn generate_with_options(
        &self,
        description: &str,
        reference_images: &[(String, String)],
        options: &SandGenerateImageExecutionOptions,
    ) -> Result<(String, String), SandGenerateImageError> {
        let generated = (self.generate)(description, reference_images, options).map_err(
            |error| match error {
                GenerateImageBackendError::Generate(message) => {
                    SandGenerateImageError::Generate(message)
                }
                GenerateImageBackendError::ModelRestricted(message) => {
                    SandGenerateImageError::ModelRestricted(message)
                }
            },
        )?;
        let bytes = STANDARD
            .decode(generated.image_data.as_bytes())
            .map_err(|_| SandGenerateImageError::InvalidBase64)?;
        let persisted = (self.persist)(&bytes, &generated.mime_type)
            .map_err(SandGenerateImageError::Generate)?
            .ok_or(SandGenerateImageError::Persist)?;
        Ok((persisted.absolute_path, generated.image_data))
    }
}
