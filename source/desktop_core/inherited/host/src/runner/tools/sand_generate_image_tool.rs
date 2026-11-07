use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::extensions::inference::provider_session::{
    ProviderPartialToolCall, ProviderSessionError, RoutedMcpMetaToolDefinition,
    RoutedToolDefinition,
};
use crate::runner::box_tool_access::{
    RunnerBoxReadRequest, RunnerBoxResourcePort, RunnerBoxWriteRequest,
};
use crate::runner::routed_provider_runtime::RoutedToolBridge;

pub const GENERATE_IMAGE_TOOL_NAME: &str = "GenerateImage";
pub const GENERATE_IMAGE_TOOL_PROVIDER: &str = "mahayana-generate-image";
const SHORT_PROMPT_WORD_THRESHOLD: usize = 5;
const VERY_SHORT_PROMPT_WORD_THRESHOLD: usize = 3;
const DEFAULT_SUSPICIOUS_KEYWORDS: &[&str] = &["ignore", "skip", "stop", "noop"];
const SUPPORTED_ASPECT_RATIOS: &[&str] = &["1:1", "4:3", "3:4", "16:9", "9:16"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImageGenerationResolution {
    #[serde(rename = "1K")]
    R1K,
    #[serde(rename = "2K")]
    R2K,
    #[serde(rename = "4K")]
    R4K,
}

impl ImageGenerationResolution {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::R1K => "1K",
            Self::R2K => "2K",
            Self::R4K => "4K",
        }
    }
}

pub fn normalize_image_generation_resolution(
    raw: Option<&str>,
) -> Result<Option<ImageGenerationResolution>, ProviderSessionError> {
    let Some(raw_str) = raw else {
        return Ok(None);
    };
    let normalized = raw_str.trim().to_ascii_uppercase();
    if normalized.is_empty() {
        return Ok(None);
    }
    match normalized.as_str() {
        "1K" => Ok(Some(ImageGenerationResolution::R1K)),
        "2K" => Ok(Some(ImageGenerationResolution::R2K)),
        "4K" => Ok(Some(ImageGenerationResolution::R4K)),
        _ => Err(ProviderSessionError::Tool(
            "resolution must be one of 1K, 2K, or 4K".into(),
        )),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedImageToolOutput {
    pub persisted_path: String,
    pub image_data_base64: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GenerateImageToolOptions {
    pub resolution: Option<ImageGenerationResolution>,
    pub aspect_ratio: Option<String>,
}

pub trait GenerateImageToolExecutor: Send + Sync {
    fn generate(
        &self,
        description: &str,
        reference_images: &[(String, String)],
    ) -> Result<GeneratedImageToolOutput, ProviderSessionError> {
        self.generate_with_options(
            description,
            reference_images,
            &GenerateImageToolOptions::default(),
        )
    }

    fn generate_with_options(
        &self,
        description: &str,
        reference_images: &[(String, String)],
        _options: &GenerateImageToolOptions,
    ) -> Result<GeneratedImageToolOutput, ProviderSessionError> {
        self.generate(description, reference_images)
    }
}

pub struct SandGenerateImageToolBridge {
    delegate: Arc<dyn RoutedToolBridge>,
    executor: Arc<dyn GenerateImageToolExecutor>,
    box_resources: Arc<dyn RunnerBoxResourcePort>,
}

impl SandGenerateImageToolBridge {
    pub fn new(
        delegate: Arc<dyn RoutedToolBridge>,
        executor: Arc<dyn GenerateImageToolExecutor>,
        box_resources: Arc<dyn RunnerBoxResourcePort>,
    ) -> Self {
        Self {
            delegate,
            executor,
            box_resources,
        }
    }
}

impl RoutedToolBridge for SandGenerateImageToolBridge {
    fn list_tools(&self) -> Result<Vec<RoutedToolDefinition>, ProviderSessionError> {
        let mut tools = self.delegate.list_tools()?;
        tools.push(generate_image_tool_definition());
        Ok(tools)
    }

    fn list_mcp_meta_tools(
        &self,
    ) -> Result<Vec<RoutedMcpMetaToolDefinition>, ProviderSessionError> {
        self.delegate.list_mcp_meta_tools()
    }

    fn observe_partial_tool_call(
        &self,
        partial: &ProviderPartialToolCall,
    ) -> Result<(), ProviderSessionError> {
        self.delegate.observe_partial_tool_call(partial)
    }

    fn call_tool(
        &self,
        tool: &RoutedToolDefinition,
        args: Value,
        tool_call_id: &str,
    ) -> Result<Value, ProviderSessionError> {
        let effective = if tool.tool_name.trim().is_empty() {
            tool.name.as_str()
        } else {
            tool.tool_name.as_str()
        };
        if effective != GENERATE_IMAGE_TOOL_NAME {
            return self.delegate.call_tool(tool, args, tool_call_id);
        }
        let description = args
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderSessionError::Tool("GenerateImage requires description".into())
            })?;
        validate_prompt(description)?;
        if let Some(aspect_ratio) = args.get("aspect_ratio").and_then(Value::as_str)
            && !SUPPORTED_ASPECT_RATIOS.contains(&aspect_ratio)
        {
            return Err(ProviderSessionError::Tool(format!(
                "GenerateImage aspect_ratio must be one of {}",
                SUPPORTED_ASPECT_RATIOS.join(", ")
            )));
        }
        let resolution = normalize_image_generation_resolution(
            args.get("resolution").and_then(Value::as_str),
        )?;
        let options = GenerateImageToolOptions {
            resolution,
            aspect_ratio: args.get("aspect_ratio").and_then(Value::as_str).map(str::to_string),
        };
        let references = self.read_reference_images(&args, tool_call_id);
        let generated = self.executor.generate_with_options(description, &references, &options)?;
        let image_bytes = STANDARD
            .decode(generated.image_data_base64.as_bytes())
            .map_err(|_| ProviderSessionError::Tool(
                "GenerateImage returned invalid base64 image data".into(),
            ))?;
        let filename = Path::new(&generated.persisted_path)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("generated-image.png");
        let output_path = format!("/workspace/assets/{filename}");
        self.box_resources.execute_write(RunnerBoxWriteRequest {
            path: output_path.clone(),
            data: image_bytes,
            tool_call_id: tool_call_id.to_string(),
        })?;
        let mut success_obj = json!({
            "filePath": output_path,
            "imageData": generated.image_data_base64,
        });
        if let Some(res) = resolution {
            success_obj["resolution"] = json!(res.as_str());
        }
        Ok(json!({
            "success": success_obj
        }))
    }
}

impl SandGenerateImageToolBridge {
    fn read_reference_images(
        &self,
        args: &Value,
        tool_call_id: &str,
    ) -> Vec<(String, String)> {
        args.get("reference_image_paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|path| {
                let value = self
                    .box_resources
                    .execute_read(RunnerBoxReadRequest {
                        path: path.to_string(),
                        tool_call_id: tool_call_id.to_string(),
                        offset: None,
                        limit: None,
                        encoding_hint: None,
                    })
                    .ok()?;
                if value.get("kind").and_then(Value::as_str) != Some("success") {
                    return None;
                }
                let data = value
                    .get("output")?
                    .get("data")?;
                let bytes = if let Some(array) = data.as_array() {
                    array
                        .iter()
                        .filter_map(Value::as_u64)
                        .filter_map(|byte| u8::try_from(byte).ok())
                        .collect::<Vec<_>>()
                } else if let Some(encoded) = data.as_str() {
                    STANDARD.decode(encoded.as_bytes()).ok()?
                } else {
                    return None;
                };
                if bytes.is_empty() {
                    return None;
                }
                Some((STANDARD.encode(bytes), image_mime_from_path(path).to_string()))
            })
            .collect()
    }
}

fn generate_image_tool_definition() -> RoutedToolDefinition {
    RoutedToolDefinition {
        name: GENERATE_IMAGE_TOOL_NAME.into(),
        provider_identifier: GENERATE_IMAGE_TOOL_PROVIDER.into(),
        tool_name: GENERATE_IMAGE_TOOL_NAME.into(),
        description: Some(
            "Generate an image from a detailed description and save it under /workspace/assets."
                .into(),
        ),
        input_schema: json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["description"],
            "properties": {
                "description": {
                    "type": "string",
                    "description": "A detailed description of the image."
                },
                "filename": {
                    "type": "string",
                    "description": "Optional filename hint. The canonical generated asset name is content-addressed."
                },
                "reference_image_paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional box file paths to reference images."
                },
                "aspect_ratio": {
                    "type": "string",
                    "enum": SUPPORTED_ASPECT_RATIOS,
                    "description": "Optional aspect ratio. The frozen Grok backend currently accepts but does not forward this hint."
                },
                "resolution": {
                    "type": "string",
                    "enum": ["1K", "2K", "4K"],
                    "description": "Optional image resolution: 1K, 2K, or 4K."
                }
            }
        }),
    }
}

fn validate_prompt(description: &str) -> Result<(), ProviderSessionError> {
    let words = description.split_whitespace().count();
    let lower = description.to_ascii_lowercase();
    let suspicious = words <= VERY_SHORT_PROMPT_WORD_THRESHOLD
        || (words <= SHORT_PROMPT_WORD_THRESHOLD
            && DEFAULT_SUSPICIOUS_KEYWORDS
                .iter()
                .any(|keyword| lower.contains(keyword)));
    if suspicious {
        return Err(ProviderSessionError::Tool(
            "Image generation was not performed — the tool call appears to be unintended. Do not retry unless the user explicitly asks for an image.".into(),
        ));
    }
    Ok(())
}

fn image_mime_from_path(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/png",
    }
}
