//! Azure OpenAI Responses provider (`azure-openai-responses`) for cortex AI.
//!
//! Port of hoocode `providers/azure-openai-responses.ts` (v0.5.89), built on
//! the Responses plumbing in `cortexcode-ai-provider-openai-responses`.

use std::collections::HashSet;

use cortexcode_ai_provider_openai_responses::shared::{
    convert_responses_messages, convert_responses_tools, run_responses_stream, set_header,
    ResponsesRequest, ResponsesStreamOptions,
};
use cortexcode_ai_provider_openai_responses::{
    apply_reasoning, require_api_key, simple_options, ResponsesOptions,
};
use cortexcode_ai_stream::AssistantMessageEventStream;
use cortexcode_ai_types::{Context, Model, SimpleStreamOptions};
use serde_json::{json, Map, Value};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const DEFAULT_AZURE_API_VERSION: &str = "v1";

/// Providers whose `call_id|item_id` tool call ids stay paired on replay.
const AZURE_TOOL_CALL_PROVIDERS: [&str; 4] = [
    "openai",
    "openai-codex",
    "opencode",
    "azure-openai-responses",
];

/// `AzureOpenAIResponsesOptions`.
#[derive(Debug, Clone, Default)]
pub struct AzureOptions {
    pub base: ResponsesOptions,
    pub azure_api_version: Option<String>,
    pub azure_resource_name: Option<String>,
    pub azure_base_url: Option<String>,
    pub azure_deployment_name: Option<String>,
}

/// `streamSimpleAzureOpenAIResponses`.
pub fn stream(
    model: Model,
    context: Context,
    options: SimpleStreamOptions,
) -> Result<AssistantMessageEventStream, BoxError> {
    let api_key = require_api_key(&model, &options)?;
    let base = simple_options(&model, &options, api_key);
    Ok(stream_azure(
        model,
        context,
        AzureOptions {
            base,
            ..Default::default()
        },
    ))
}

/// `streamAzureOpenAIResponses`. Configuration errors (a missing or invalid
/// base URL) end the stream with an `error` event, as in TS.
pub fn stream_azure(
    model: Model,
    context: Context,
    options: AzureOptions,
) -> AssistantMessageEventStream {
    let deployment_name = resolve_deployment_name(&model, &options);
    let request = resolve_azure_config(&model, &options).map(|(base_url, api_version)| {
        let mut headers = vec![
            ("content-type".to_string(), "application/json".to_string()),
            (
                "api-key".to_string(),
                options.base.api_key.clone().unwrap_or_default(),
            ),
        ];
        if let Some(extra) = &model.headers {
            for (k, v) in extra {
                set_header(&mut headers, k, v);
            }
        }
        if let Some(extra) = &options.base.headers {
            for (k, v) in extra {
                set_header(&mut headers, k, v);
            }
        }
        ResponsesRequest {
            url: request_url(&base_url, &api_version),
            headers,
            body: build_params(&model, &context, &options.base, &deployment_name),
            timeout_ms: options.base.timeout_ms,
        }
    });
    run_responses_stream(
        model,
        request,
        options.base.signal.clone(),
        ResponsesStreamOptions::default(),
    )
}

/// The URL the `AzureOpenAI` client posts to: `{baseURL}/responses` with the
/// query replaced by `api-version` (`buildURL` assigns `url.search`).
fn request_url(base_url: &str, api_version: &str) -> String {
    let joined = format!("{base_url}/responses");
    match reqwest::Url::parse(&joined) {
        Ok(mut url) => {
            url.set_query(None);
            url.query_pairs_mut()
                .append_pair("api-version", api_version);
            url.to_string()
        }
        Err(_) => format!("{joined}?api-version={api_version}"),
    }
}

/// `parseDeploymentNameMap`: `model1=deployment1,model2=deployment2`.
fn parse_deployment_name_map(value: Option<&str>) -> Vec<(String, String)> {
    let Some(value) = value else {
        return Vec::new();
    };
    value
        .split(',')
        .filter_map(|entry| {
            let entry = entry.trim();
            let (model_id, deployment) = entry.split_once('=')?;
            let deployment = deployment.split('=').next().unwrap_or_default();
            (!model_id.is_empty() && !deployment.is_empty())
                .then(|| (model_id.trim().to_string(), deployment.trim().to_string()))
        })
        .collect()
}

/// `resolveDeploymentName`: the explicit option, else the
/// `AZURE_OPENAI_DEPLOYMENT_NAME_MAP` entry for the model, else its id.
pub fn resolve_deployment_name(model: &Model, options: &AzureOptions) -> String {
    if let Some(name) = options
        .azure_deployment_name
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        return name.to_string();
    }
    let map = std::env::var("AZURE_OPENAI_DEPLOYMENT_NAME_MAP").ok();
    parse_deployment_name_map(map.as_deref())
        .into_iter()
        .rev()
        .find(|(id, _)| *id == model.id)
        .map(|(_, deployment)| deployment)
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| model.id.clone())
}

/// `normalizeAzureBaseUrl`: Azure hosts get the `/openai/v1` base path (and
/// lose their query); other URLs are kept.
pub fn normalize_azure_base_url(base_url: &str) -> Result<String, String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    let mut url = reqwest::Url::parse(trimmed)
        .map_err(|_| format!("Invalid Azure OpenAI base URL: {base_url}"))?;
    let host = url.host_str().unwrap_or_default().to_string();
    let is_azure_host =
        host.ends_with(".openai.azure.com") || host.ends_with(".cognitiveservices.azure.com");
    let path = url.path().trim_end_matches('/').to_string();
    if is_azure_host && (path.is_empty() || path == "/openai") {
        url.set_path("/openai/v1");
        url.set_query(None);
    }
    Ok(url.to_string().trim_end_matches('/').to_string())
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `resolveAzureConfig`: base URL from the option, `AZURE_OPENAI_BASE_URL`,
/// `AZURE_OPENAI_RESOURCE_NAME` or the model, and the API version.
pub fn resolve_azure_config(
    model: &Model,
    options: &AzureOptions,
) -> Result<(String, String), String> {
    let api_version = options
        .azure_api_version
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| env_value("AZURE_OPENAI_API_VERSION"))
        .unwrap_or_else(|| DEFAULT_AZURE_API_VERSION.to_string());
    let base_url = options
        .azure_base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("AZURE_OPENAI_BASE_URL")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
    let resource_name = options
        .azure_resource_name
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| env_value("AZURE_OPENAI_RESOURCE_NAME"));

    let resolved = base_url
        .or_else(|| resource_name.map(|name| format!("https://{name}.openai.azure.com/openai/v1")))
        .or_else(|| Some(model.base_url.clone()).filter(|u| !u.is_empty()))
        .ok_or_else(|| {
            "Azure OpenAI base URL is required. Set AZURE_OPENAI_BASE_URL or AZURE_OPENAI_RESOURCE_NAME, or pass azureBaseUrl, azureResourceName, or model.baseUrl.".to_string()
        })?;
    Ok((normalize_azure_base_url(&resolved)?, api_version))
}

/// `buildParams` for azure-openai-responses.
pub fn build_params(
    model: &Model,
    context: &Context,
    options: &ResponsesOptions,
    deployment_name: &str,
) -> Value {
    let allowed: HashSet<&str> = AZURE_TOOL_CALL_PROVIDERS.into_iter().collect();
    let input = convert_responses_messages(model, context, &allowed, true);
    let mut params = Map::new();
    params.insert("model".into(), json!(deployment_name));
    params.insert("input".into(), Value::Array(input));
    params.insert("stream".into(), json!(true));
    if let Some(session_id) = &options.session_id {
        params.insert("prompt_cache_key".into(), json!(session_id));
    }
    if let Some(max_tokens) = options.max_tokens.filter(|&n| n > 0) {
        params.insert("max_output_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = options.temperature {
        params.insert("temperature".into(), json!(temperature));
    }
    if !context.tools.is_empty() {
        params.insert(
            "tools".into(),
            Value::Array(convert_responses_tools(
                &context.tools,
                None,
                options.constrain_tool_calls,
            )),
        );
    }
    apply_reasoning(
        &mut params,
        model,
        options.reasoning_effort.as_deref(),
        options.reasoning_summary.as_deref(),
        false,
    );
    Value::Object(params)
}

#[cfg(test)]
mod tests;
