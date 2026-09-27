//! Faux / test provider for cortex AI.
//!
//! Port of hoocode `packages/ai/src/providers/faux.ts` (v0.5.89): queue canned
//! assistant messages (or factories producing them) and stream them back as
//! if they came from a model, with estimated usage, simulated prompt caching
//! per `sessionId`, optional `tokensPerSecond` pacing and abort handling.
//!
//! [`register_faux_provider`] registers the provider on the API registry like
//! `registerFauxProvider()`. [`FauxProvider::stream_fn`] exposes the same
//! stream function for callers that inject a stream function directly.

use std::collections::hash_map::RandomState;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::hash::{BuildHasher, Hasher};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cortexcode_ai_registry::{register_api_provider, unregister_api_providers, BoxError};
use cortexcode_ai_stream::{
    create_assistant_message_event_stream, spawn_producer, AssistantMessageEventStream,
};
use cortexcode_ai_types::{
    now_ms, AbortSignal, AssistantMessage, AssistantMessageEvent, CacheRetention, Content, Context,
    Cost, Message, Model, ModelCost, SimpleStreamOptions, StopReason, TextContent, ThinkingContent,
    ToolCallContent, ToolResultMessage, Usage,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_API: &str = "faux";
const DEFAULT_PROVIDER: &str = "faux";
const DEFAULT_MODEL_ID: &str = "faux-1";
const DEFAULT_MODEL_NAME: &str = "Faux Model";
const DEFAULT_BASE_URL: &str = "http://localhost:0";
const DEFAULT_MIN_TOKEN_SIZE: usize = 3;
const DEFAULT_MAX_TOKEN_SIZE: usize = 5;

const DEFAULT_USAGE: Usage = Usage {
    input: 0,
    output: 0,
    cache_read: 0,
    cache_write: 0,
    total_tokens: 0,
    cost: Cost {
        input: 0.0,
        output: 0.0,
        cache_read: 0.0,
        cache_write: 0.0,
        total: 0.0,
    },
};

// ---------------------------------------------------------------------------
// Options and helpers
// ---------------------------------------------------------------------------

/// `FauxModelDefinition`.
#[derive(Debug, Clone, Default)]
pub struct FauxModelDefinition {
    pub id: String,
    pub name: Option<String>,
    pub reasoning: Option<bool>,
    pub input: Option<Vec<String>>,
    pub cost: Option<ModelCost>,
    pub context_window: Option<u64>,
    pub max_tokens: Option<u64>,
}

impl FauxModelDefinition {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Default::default()
        }
    }
}

/// `tokenSize` in `RegisterFauxProviderOptions`.
#[derive(Debug, Clone, Copy, Default)]
pub struct FauxTokenSize {
    pub min: Option<usize>,
    pub max: Option<usize>,
}

/// `RegisterFauxProviderOptions`.
#[derive(Debug, Clone, Default)]
pub struct RegisterFauxProviderOptions {
    /// Defaults to a random `faux:<ms>:<id>` api.
    pub api: Option<String>,
    pub provider: Option<String>,
    /// Empty means the single default `faux-1` model.
    pub models: Vec<FauxModelDefinition>,
    pub tokens_per_second: Option<f64>,
    pub token_size: Option<FauxTokenSize>,
}

/// `fauxText()`.
pub fn faux_text(text: impl Into<String>) -> Content {
    Content::Text(TextContent {
        text: text.into(),
        text_signature: None,
    })
}

/// `fauxThinking()`.
pub fn faux_thinking(thinking: impl Into<String>) -> Content {
    Content::Thinking(ThinkingContent {
        thinking: thinking.into(),
        signature: None,
        redacted: false,
    })
}

/// `fauxToolCall()`: `id` defaults to a random `tool:<ms>:<id>`.
pub fn faux_tool_call(
    name: impl Into<String>,
    arguments: serde_json::Value,
    id: Option<String>,
) -> Content {
    Content::ToolCall(ToolCallContent {
        id: id.unwrap_or_else(|| random_id("tool")),
        name: name.into(),
        arguments,
        thought_signature: None,
    })
}

/// Content accepted by [`faux_assistant_message`]: a string, one block or a
/// list of blocks (`string | FauxContentBlock | FauxContentBlock[]`).
pub enum FauxAssistantContent {
    Text(String),
    Blocks(Vec<Content>),
}

impl From<&str> for FauxAssistantContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_string())
    }
}

impl From<String> for FauxAssistantContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Content> for FauxAssistantContent {
    fn from(block: Content) -> Self {
        Self::Blocks(vec![block])
    }
}

impl From<Vec<Content>> for FauxAssistantContent {
    fn from(blocks: Vec<Content>) -> Self {
        Self::Blocks(blocks)
    }
}

/// Options of `fauxAssistantMessage()`.
#[derive(Debug, Clone, Default)]
pub struct FauxMessageOptions {
    pub stop_reason: Option<StopReason>,
    pub error_message: Option<String>,
    pub response_id: Option<String>,
    pub timestamp: Option<i64>,
}

/// `fauxAssistantMessage()`.
pub fn faux_assistant_message(
    content: impl Into<FauxAssistantContent>,
    options: FauxMessageOptions,
) -> AssistantMessage {
    let content = match content.into() {
        FauxAssistantContent::Text(text) => vec![faux_text(text)],
        FauxAssistantContent::Blocks(blocks) => blocks,
    };
    AssistantMessage {
        content,
        api: DEFAULT_API.to_string(),
        provider: DEFAULT_PROVIDER.to_string(),
        model: DEFAULT_MODEL_ID.to_string(),
        response_id: options.response_id,
        response_model: None,
        diagnostics: None,
        usage: DEFAULT_USAGE,
        stop_reason: options.stop_reason.unwrap_or_default(),
        error_message: options.error_message,
        timestamp: options.timestamp.unwrap_or_else(now_ms),
    }
}

// ---------------------------------------------------------------------------
// Response steps
// ---------------------------------------------------------------------------

/// `state` passed to response factories.
#[derive(Debug, Default)]
pub struct FauxState {
    call_count: AtomicUsize,
}

impl FauxState {
    /// `state.callCount`.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

/// Result of a response factory; an `Err` becomes an `error` event
/// (a thrown error in TypeScript).
pub type FauxFactoryResult = Result<AssistantMessage, BoxError>;

/// Future returned by a response factory.
pub type FauxFactoryFuture = Pin<Box<dyn Future<Output = FauxFactoryResult> + Send>>;

/// `FauxResponseFactory`: `(context, options, state, model) => AssistantMessage | Promise<…>`.
pub type FauxResponseFactory =
    Box<dyn FnOnce(&Context, &SimpleStreamOptions, &FauxState, &Model) -> FauxFactoryFuture + Send>;

/// `FauxResponseStep`: a message, or a factory called when the step is used.
// Queued test responses; not hot enough for boxing to matter.
#[allow(clippy::large_enum_variant)]
pub enum FauxResponseStep {
    Message(AssistantMessage),
    Factory(FauxResponseFactory),
}

impl FauxResponseStep {
    /// A synchronous factory.
    pub fn factory<F>(f: F) -> Self
    where
        F: FnOnce(&Context, &SimpleStreamOptions, &FauxState, &Model) -> FauxFactoryResult
            + Send
            + 'static,
    {
        Self::Factory(Box::new(move |context, options, state, model| {
            let result = f(context, options, state, model);
            Box::pin(async move { result })
        }))
    }

    /// An async factory.
    pub fn async_factory<F, Fut>(f: F) -> Self
    where
        F: FnOnce(&Context, &SimpleStreamOptions, &FauxState, &Model) -> Fut + Send + 'static,
        Fut: Future<Output = FauxFactoryResult> + Send + 'static,
    {
        Self::Factory(Box::new(move |context, options, state, model| {
            Box::pin(f(context, options, state, model))
        }))
    }
}

impl From<AssistantMessage> for FauxResponseStep {
    fn from(message: AssistantMessage) -> Self {
        Self::Message(message)
    }
}

// ---------------------------------------------------------------------------
// FauxProvider
// ---------------------------------------------------------------------------

/// The faux provider: the state behind `registerFauxProvider()`, usable
/// without registering (see [`FauxProvider::stream_fn`]).
pub struct FauxProvider {
    api: String,
    provider: String,
    models: Vec<Model>,
    state: Arc<FauxState>,
    pending: Mutex<VecDeque<FauxResponseStep>>,
    prompt_cache: Mutex<HashMap<String, String>>,
    min_token_size: usize,
    max_token_size: usize,
    tokens_per_second: Option<f64>,
}

/// Stream function shape shared with the API registry and agent configs.
pub type FauxStreamFn = Box<
    dyn Fn(Model, Context, SimpleStreamOptions) -> Result<AssistantMessageEventStream, BoxError>
        + Send
        + Sync,
>;

impl FauxProvider {
    /// A provider with default options.
    pub fn new() -> Arc<Self> {
        Self::with_options(RegisterFauxProviderOptions::default())
    }

    pub fn with_options(options: RegisterFauxProviderOptions) -> Arc<Self> {
        let api = options.api.unwrap_or_else(|| random_id(DEFAULT_API));
        let provider = options
            .provider
            .unwrap_or_else(|| DEFAULT_PROVIDER.to_string());
        let token_size = options.token_size.unwrap_or_default();
        let min_token_size = token_size
            .min
            .unwrap_or(DEFAULT_MIN_TOKEN_SIZE)
            .min(token_size.max.unwrap_or(DEFAULT_MAX_TOKEN_SIZE))
            .max(1);
        let max_token_size = token_size
            .max
            .unwrap_or(DEFAULT_MAX_TOKEN_SIZE)
            .max(min_token_size);

        let definitions = if options.models.is_empty() {
            vec![FauxModelDefinition {
                id: DEFAULT_MODEL_ID.to_string(),
                name: Some(DEFAULT_MODEL_NAME.to_string()),
                reasoning: Some(false),
                input: Some(vec!["text".into(), "image".into()]),
                cost: Some(ModelCost::default()),
                context_window: Some(128_000),
                max_tokens: Some(16_384),
            }]
        } else {
            options.models
        };
        let models = definitions
            .into_iter()
            .map(|d| Model {
                name: d.name.unwrap_or_else(|| d.id.clone()),
                id: d.id,
                api: api.clone(),
                provider: provider.clone(),
                base_url: DEFAULT_BASE_URL.to_string(),
                reasoning: d.reasoning.unwrap_or(false),
                thinking_level_map: None,
                input: d
                    .input
                    .unwrap_or_else(|| vec!["text".into(), "image".into()]),
                cost: d.cost.unwrap_or_default(),
                context_window: d.context_window.unwrap_or(128_000),
                max_tokens: d.max_tokens.unwrap_or(16_384),
                headers: None,
                compat: None,
            })
            .collect();

        Arc::new(Self {
            api,
            provider,
            models,
            state: Arc::new(FauxState::default()),
            pending: Mutex::new(VecDeque::new()),
            prompt_cache: Mutex::new(HashMap::new()),
            min_token_size,
            max_token_size,
            tokens_per_second: options.tokens_per_second,
        })
    }

    /// `registration.api`.
    pub fn api(&self) -> &str {
        &self.api
    }

    /// `registration.models`.
    pub fn models(&self) -> &[Model] {
        &self.models
    }

    /// `getModel()`: the first model.
    pub fn get_model(&self) -> Model {
        self.models[0].clone()
    }

    /// `getModel(modelId)`.
    pub fn get_model_by_id(&self, model_id: &str) -> Option<Model> {
        self.models.iter().find(|m| m.id == model_id).cloned()
    }

    /// `registration.state`.
    pub fn state(&self) -> &FauxState {
        &self.state
    }

    /// `state.callCount`.
    pub fn call_count(&self) -> usize {
        self.state.call_count()
    }

    /// `setResponses()`.
    pub fn set_responses(&self, responses: Vec<FauxResponseStep>) {
        *self.pending.lock().unwrap() = responses.into();
    }

    /// `appendResponses()`.
    pub fn append_responses(&self, responses: Vec<FauxResponseStep>) {
        self.pending.lock().unwrap().extend(responses);
    }

    /// `getPendingResponseCount()`.
    pub fn get_pending_response_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    /// The provider's stream function (`stream` in faux.ts). Never fails:
    /// every problem is reported as an `error` event.
    pub fn stream(
        self: &Arc<Self>,
        model: Model,
        context: Context,
        options: SimpleStreamOptions,
    ) -> AssistantMessageEventStream {
        let outer = create_assistant_message_event_stream();
        let step = self.pending.lock().unwrap().pop_front();
        self.state.call_count.fetch_add(1, Ordering::SeqCst);

        let this = Arc::clone(self);
        let producer_stream = outer.clone();
        spawn_producer(&outer, async move {
            this.produce(producer_stream, step, model, context, options)
                .await;
        });
        outer
    }

    /// [`Self::stream`] as a boxed stream function.
    pub fn stream_fn(self: &Arc<Self>) -> FauxStreamFn {
        let this = Arc::clone(self);
        Box::new(move |model, context, options| Ok(this.stream(model, context, options)))
    }

    async fn produce(
        &self,
        outer: AssistantMessageEventStream,
        step: Option<FauxResponseStep>,
        model: Model,
        context: Context,
        options: SimpleStreamOptions,
    ) {
        cortexcode_ai_types::OnResponse::notify(
            options.on_response.as_ref(),
            cortexcode_ai_types::ProviderResponse {
                status: 200,
                headers: Default::default(),
            },
            &model,
        )
        .await;
        let Some(step) = step else {
            let mut message = self.error_message("No more faux responses queued", &model);
            message.usage = self.usage_estimate(&message, &context, &options);
            outer.push(AssistantMessageEvent::Error {
                error: message.clone(),
            });
            outer.end(Some(message));
            return;
        };

        let resolved = match step {
            FauxResponseStep::Message(message) => Ok(message),
            FauxResponseStep::Factory(factory) => {
                factory(&context, &options, &self.state, &model).await
            }
        };
        let mut message = match resolved {
            Ok(message) => message,
            Err(error) => {
                let message = self.error_message(&error.to_string(), &model);
                outer.push(AssistantMessageEvent::Error {
                    error: message.clone(),
                });
                outer.end(Some(message));
                return;
            }
        };
        // cloneMessage(): stamped with this provider's api/provider and the requested model.
        message.api = self.api.clone();
        message.provider = self.provider.clone();
        message.model = model.id.clone();
        message.usage = self.usage_estimate(&message, &context, &options);
        self.stream_with_deltas(&outer, message, options.signal.as_ref())
            .await;
    }

    /// `createErrorMessage()`.
    fn error_message(&self, error: &str, model: &Model) -> AssistantMessage {
        AssistantMessage {
            content: vec![],
            api: self.api.clone(),
            provider: self.provider.clone(),
            model: model.id.clone(),
            response_id: None,
            response_model: None,
            diagnostics: None,
            usage: DEFAULT_USAGE,
            stop_reason: StopReason::Error,
            error_message: Some(error.to_string()),
            timestamp: now_ms(),
        }
    }

    /// `withUsageEstimate()`: estimate tokens from the serialized context and
    /// simulate prompt caching per `sessionId` (unless `cacheRetention` is none).
    fn usage_estimate(
        &self,
        message: &AssistantMessage,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> Usage {
        let prompt_text = serialize_context(context);
        let prompt: Vec<u16> = prompt_text.encode_utf16().collect();
        let prompt_tokens = estimate_units(prompt.len());
        let output = estimate_tokens(&assistant_content_to_text(&message.content));
        let mut input = prompt_tokens;
        let mut cache_read = 0;
        let mut cache_write = 0;

        if let Some(session_id) = options.session_id.as_deref() {
            if options.cache_retention != Some(CacheRetention::None) {
                let mut cache = self.prompt_cache.lock().unwrap();
                match cache.get(session_id).filter(|p| !p.is_empty()) {
                    Some(previous) => {
                        let previous: Vec<u16> = previous.encode_utf16().collect();
                        let cached = previous
                            .iter()
                            .zip(&prompt)
                            .take_while(|(a, b)| a == b)
                            .count();
                        cache_read = estimate_units(cached);
                        cache_write = estimate_units(prompt.len() - cached);
                        input = prompt_tokens.saturating_sub(cache_read);
                    }
                    None => cache_write = prompt_tokens,
                }
                cache.insert(session_id.to_string(), prompt_text);
            }
        }

        Usage {
            input,
            output,
            cache_read,
            cache_write,
            total_tokens: input + output + cache_read + cache_write,
            cost: DEFAULT_USAGE.cost,
        }
    }

    /// `streamWithDeltas()`.
    async fn stream_with_deltas(
        &self,
        stream: &AssistantMessageEventStream,
        message: AssistantMessage,
        signal: Option<&AbortSignal>,
    ) {
        let aborted = |partial: &AssistantMessage| {
            signal
                .is_some_and(|s| s.aborted())
                .then(|| AssistantMessage {
                    stop_reason: StopReason::Aborted,
                    error_message: Some("Request was aborted".to_string()),
                    timestamp: now_ms(),
                    ..partial.clone()
                })
        };
        let fail = |error: AssistantMessage| {
            stream.push(AssistantMessageEvent::Error {
                error: error.clone(),
            });
            stream.end(Some(error));
        };

        let mut partial = AssistantMessage {
            content: vec![],
            ..message.clone()
        };
        if let Some(error) = aborted(&partial) {
            return fail(error);
        }
        stream.push(AssistantMessageEvent::Start {
            partial: partial.clone(),
        });

        for (index, block) in message.content.iter().enumerate() {
            if let Some(error) = aborted(&partial) {
                return fail(error);
            }
            match block {
                Content::Thinking(thinking) => {
                    partial.content.push(faux_thinking(""));
                    stream.push(AssistantMessageEvent::ThinkingStart {
                        index,
                        partial: partial.clone(),
                    });
                    for chunk in self.split(&thinking.thinking) {
                        self.schedule_chunk(&chunk).await;
                        if let Some(error) = aborted(&partial) {
                            return fail(error);
                        }
                        if let Some(Content::Thinking(t)) = partial.content.get_mut(index) {
                            t.thinking.push_str(&chunk);
                        }
                        stream.push(AssistantMessageEvent::ThinkingDelta {
                            index,
                            delta: chunk,
                            partial: partial.clone(),
                        });
                    }
                    stream.push(AssistantMessageEvent::ThinkingEnd {
                        index,
                        partial: partial.clone(),
                    });
                }
                Content::Text(text) => {
                    partial.content.push(faux_text(""));
                    stream.push(AssistantMessageEvent::TextStart {
                        index,
                        partial: partial.clone(),
                    });
                    for chunk in self.split(&text.text) {
                        self.schedule_chunk(&chunk).await;
                        if let Some(error) = aborted(&partial) {
                            return fail(error);
                        }
                        if let Some(Content::Text(t)) = partial.content.get_mut(index) {
                            t.text.push_str(&chunk);
                        }
                        stream.push(AssistantMessageEvent::TextDelta {
                            index,
                            delta: chunk,
                            partial: partial.clone(),
                        });
                    }
                    stream.push(AssistantMessageEvent::TextEnd {
                        index,
                        partial: partial.clone(),
                    });
                }
                Content::ToolCall(call) => {
                    partial.content.push(Content::ToolCall(ToolCallContent {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        arguments: serde_json::json!({}),
                        thought_signature: None,
                    }));
                    stream.push(AssistantMessageEvent::ToolCallStart {
                        index,
                        partial: partial.clone(),
                    });
                    for chunk in self.split(&call.arguments.to_string()) {
                        self.schedule_chunk(&chunk).await;
                        if let Some(error) = aborted(&partial) {
                            return fail(error);
                        }
                        stream.push(AssistantMessageEvent::ToolCallDelta {
                            index,
                            delta: chunk,
                            partial: partial.clone(),
                        });
                    }
                    if let Some(Content::ToolCall(c)) = partial.content.get_mut(index) {
                        c.arguments = call.arguments.clone();
                    }
                    stream.push(AssistantMessageEvent::ToolCallEnd {
                        index,
                        partial: partial.clone(),
                    });
                }
                // Not a FauxContentBlock; carried through without events.
                Content::Image(_) => partial.content.push(block.clone()),
            }
        }

        if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
            return fail(message);
        }
        stream.push(AssistantMessageEvent::Done {
            message: message.clone(),
        });
        stream.end(Some(message));
    }

    fn split(&self, text: &str) -> Vec<String> {
        split_string_by_token_size(text, self.min_token_size, self.max_token_size)
    }

    /// `scheduleChunk()`: yield, or sleep for the chunk's share of
    /// `tokensPerSecond`.
    async fn schedule_chunk(&self, chunk: &str) {
        match self.tokens_per_second.filter(|tps| *tps > 0.0) {
            None => tokio::task::yield_now().await,
            Some(tps) => {
                let secs = estimate_tokens(chunk) as f64 / tps;
                tokio::time::sleep(Duration::from_secs_f64(secs)).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// `FauxProviderRegistration`: derefs to the [`FauxProvider`] for
/// `getModel`, `setResponses`, `state`, …
pub struct FauxProviderRegistration {
    provider: Arc<FauxProvider>,
    source_id: String,
}

impl FauxProviderRegistration {
    /// `unregister()`.
    pub fn unregister(&self) {
        unregister_api_providers(&self.source_id);
    }

    /// The registered provider.
    pub fn provider(&self) -> &Arc<FauxProvider> {
        &self.provider
    }
}

impl Deref for FauxProviderRegistration {
    type Target = Arc<FauxProvider>;

    fn deref(&self) -> &Self::Target {
        &self.provider
    }
}

/// `registerFauxProvider()`: create a faux provider and register it on the
/// API registry under its api.
pub fn register_faux_provider(options: RegisterFauxProviderOptions) -> FauxProviderRegistration {
    let provider = FauxProvider::with_options(options);
    let source_id = random_id("faux-provider");
    register_api_provider(
        provider.api(),
        Arc::from(provider.stream_fn()),
        Some(&source_id),
    );
    FauxProviderRegistration {
        provider,
        source_id,
    }
}

// ---------------------------------------------------------------------------
// Serialization and estimates
// ---------------------------------------------------------------------------

/// `estimateTokens()`: `ceil(text.length / 4)` in UTF-16 code units.
fn estimate_tokens(text: &str) -> u64 {
    estimate_units(text.encode_utf16().count())
}

fn estimate_units(units: usize) -> u64 {
    units.div_ceil(4) as u64
}

/// `randomId()`: `<prefix>:<ms>:<base36 random>`.
fn random_id(prefix: &str) -> String {
    format!("{prefix}:{}:{}", now_ms(), to_base36(random_u64()))
}

fn random_u64() -> u64 {
    RandomState::new().build_hasher().finish()
}

fn to_base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// `contentToText()` for user and tool-result content.
fn content_to_text(content: &[Content]) -> String {
    content
        .iter()
        .map(|block| match block {
            Content::Image(image) => format!(
                "[image:{}:{}]",
                image.media_type,
                image.data.encode_utf16().count()
            ),
            other => assistant_content_to_text(std::slice::from_ref(other)),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `assistantContentToText()`.
fn assistant_content_to_text(content: &[Content]) -> String {
    content
        .iter()
        .map(|block| match block {
            Content::Text(text) => text.text.clone(),
            Content::Thinking(thinking) => thinking.thinking.clone(),
            Content::ToolCall(call) => format!("{}:{}", call.name, call.arguments),
            Content::Image(_) => content_to_text(std::slice::from_ref(block)),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `toolResultToText()`.
fn tool_result_to_text(message: &ToolResultMessage) -> String {
    std::iter::once(message.tool_name.clone())
        .chain(
            message
                .content
                .iter()
                .map(|block| content_to_text(std::slice::from_ref(block))),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

/// `serializeContext()`.
fn serialize_context(context: &Context) -> String {
    let mut parts = Vec::new();
    if !context.system_prompt.is_empty() {
        parts.push(format!("system:{}", context.system_prompt));
    }
    for message in &context.messages {
        parts.push(match message {
            Message::User(m) => format!("user:{}", content_to_text(&m.content.blocks())),
            Message::Assistant(m) => format!("assistant:{}", assistant_content_to_text(&m.content)),
            Message::ToolResult(m) => format!("toolResult:{}", tool_result_to_text(m)),
        });
    }
    if !context.tools.is_empty() {
        let tools: Vec<serde_json::Value> = context
            .tools
            .iter()
            .map(|tool| {
                let mut value = serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                });
                if let Some(defer) = tool.defer_loading {
                    value["deferLoading"] = defer.into();
                }
                value
            })
            .collect();
        parts.push(format!("tools:{}", serde_json::Value::from(tools)));
    }
    parts.join("\n\n")
}

/// `splitStringByTokenSize()`: chunks of a random `min..=max` tokens (4
/// UTF-16 units each). Chunks end on char boundaries, so a chunk can be one
/// unit longer than in TypeScript when it would split a surrogate pair.
fn split_string_by_token_size(
    text: &str,
    min_token_size: usize,
    max_token_size: usize,
) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut units = 0;
    let mut target = 0;
    for ch in text.chars() {
        if units == 0 {
            let span = (max_token_size - min_token_size + 1) as u64;
            let token_size = min_token_size + (random_u64() % span) as usize;
            target = (token_size * 4).max(1);
        }
        current.push(ch);
        units += ch.len_utf16();
        if units >= target {
            chunks.push(std::mem::take(&mut current));
            units = 0;
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_reassembles_and_respects_fixed_size() {
        assert_eq!(split_string_by_token_size("", 3, 5), vec![""]);
        assert_eq!(split_string_by_token_size("hi", 3, 5), vec!["hi"]);
        let text = "hello world this is a test of the chunking function";
        let chunks = split_string_by_token_size(text, 1, 1);
        assert!(chunks[..chunks.len() - 1].iter().all(|c| c.len() == 4));
        assert_eq!(chunks.concat(), text);
        let chunks = split_string_by_token_size(text, 3, 5);
        assert!(chunks.iter().all(|c| (1..=20).contains(&c.len())));
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn estimate_tokens_counts_utf16_units() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("hello"), 2);
        // One astral char is two UTF-16 units, as `"😀".length` in JS.
        assert_eq!(estimate_tokens("😀😀😀"), 2);
    }

    #[test]
    fn random_ids_have_ts_shape() {
        let id = random_id("tool");
        let parts: Vec<&str> = id.split(':').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "tool");
        assert!(parts[1].parse::<i64>().is_ok());
        assert!(parts[2].chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(random_id("tool"), random_id("tool"));
    }
}
