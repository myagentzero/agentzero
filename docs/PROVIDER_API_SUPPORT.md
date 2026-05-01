# ZeroClaw Custom Provider API Support
## Chat Completions vs Responses API Analysis

### Executive Summary

ZeroClaw provides a multi-provider architecture with support for both **Chat Completions** (standard OpenAI-compatible endpoint) and **Responses** (newer, structured output-focused endpoint) APIs. The system uses an abstraction layer to normalize provider differences and intelligently routes requests based on provider capabilities.

---

## Architecture Overview

### Provider Trait System (`traits.rs`)

The foundation is the `Provider` trait, an async abstraction that defines:

```rust
pub trait Provider: Send + Sync {
    // Query provider capabilities
    fn capabilities(&self) -> ProviderCapabilities { ... }
    
    // Convert tool specs to provider-native format
    fn convert_tools(&self, tools: &[ToolSpec]) -> ToolsPayload { ... }
    
    // Chat APIs (escalating complexity)
    async fn simple_chat(...)
    async fn chat_with_system(...)
    async fn chat_with_history(...)
    async fn chat(...)                        // Structured, agent-focused
    async fn chat_with_tools(...)             // Native tool calling
    
    // Streaming support
    fn supports_streaming(&self) -> bool { ... }
    fn stream_chat_with_system(...)
    fn stream_chat_with_history(...)
}
```

### Provider Capabilities Declaration

```rust
pub struct ProviderCapabilities {
    pub native_tool_calling: bool,  // Native API tool definitions
    pub vision: bool,               // Image/vision input support
}

pub enum ToolsPayload {
    Gemini { function_declarations },
    Anthropic { tools },
    OpenAI { tools },
    PromptGuided { instructions },  // Fallback: inject as text
}
```

**Key Design**: Providers declare what they support. The framework uses defaults and adapts request formatting accordingly.

---

## API Mode Support

### 1. Chat Completions API (Primary)

**Standard OpenAI-compatible endpoint**: `/v1/chat/completions`

**Request Structure:**
```json
{
  "model": "gpt-4",
  "messages": [
    { "role": "system", "content": "..." },
    { "role": "user", "content": "..." },
    { "role": "assistant", "content": "..." },
    { "role": "tool", "content": "...", "tool_call_id": "..." }
  ],
  "temperature": 0.7,
  "max_tokens": 2048,
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "tool_name",
        "description": "...",
        "parameters": { /* JSON Schema */ }
      }
    }
  ]
}
```

**Response Structure:**
```json
{
  "choices": [
    {
      "message": {
        "role": "assistant",
        "content": "text response",
        "tool_calls": [
          {
            "id": "call_123",
            "type": "function",
            "function": {
              "name": "tool_name",
              "arguments": "{...}"
            }
          }
        ]
      },
      "finish_reason": "tool_calls",
      "index": 0
    }
  ],
  "usage": {
    "prompt_tokens": 100,
    "completion_tokens": 50
  }
}
```

**Supported by:**
- OpenAI (`openai.rs`)
- OpenAI-compatible endpoints (`compatible.rs`)
  - Mistral, Groq, Together, Replicate, etc.
- OpenRouter (proxies to underlying models)
- Bedrock (through compatible layer)

**Tool Calling:**
- Native OpenAI-style function tools (type: "function")
- Tool choice: "auto" (default) or "required" or specific tool name
- Tool calls appear in `assistant.tool_calls` array

---

### 2. Responses API (Structured Output)

**Specialized OpenAI endpoint**: `/v1/responses`

**Status**: Newer API focused on structured outputs, beta availability

**Request Structure:**
```json
{
  "model": "gpt-4",
  "messages": [...],
  "response_format": {
    "type": "json_schema",
    "json_schema": {
      "name": "response_schema",
      "schema": { /* JSON Schema */ },
      "strict": true
    }
  }
}
```

**Key Differences from Chat Completions:**
- Focus on enforcing schema compliance
- Stricter validation of outputs
- Better support for reasoning/structured reasoning models
- May require different error handling

**Provider Implementation:**
In `compatible.rs`, the `CompatibleApiMode` enum:

```rust
pub enum CompatibleApiMode {
    OpenAiChatCompletions,  // Default: call /v1/chat/completions
    OpenAiResponses,        // Call /v1/responses directly
}
```

**Configuration:**
```rust
supports_responses_fallback: bool,  // Allow fallback to /v1/responses on 404
```

**Providers with Responses Support:**
- OpenAI (primary support)
- Selective compatible providers (when `api_mode: CompatibleApiMode::OpenAiResponses`)

**Non-Supporting Providers:**
- GLM/Zhipu: explicitly disables responses fallback
- Anthropic: uses Messages API (proprietary format)
- Gemini: uses proprietary API format

---

## Request Flow and Adaptation

### How Tool Calling Works

#### Path 1: Native Tool Calling Provider
```
User Request (with tools)
    ↓
Provider::chat(ChatRequest { messages, tools })
    ↓
supports_native_tools() == true
    ↓
Provider::chat_with_tools() → Provider-specific implementation
    ↓
Return ChatResponse { text, tool_calls: [...] }
```

#### Path 2: Prompt-Guided Fallback
```
User Request (with tools)
    ↓
Provider::chat(ChatRequest { messages, tools })
    ↓
supports_native_tools() == false
    ↓
convert_tools(&tools) → ToolsPayload::PromptGuided { instructions }
    ↓
Inject tool instructions into system prompt
    ↓
Provider::chat_with_history()
    ↓
Parse XML-style <tool_call> tags from response
    ↓
Return ChatResponse { text, tool_calls: [...] }
```

### Anthropic (Messages API) vs OpenAI (Chat Completions)

**Anthropic (`anthropic.rs`):**
- Uses proprietary Messages API (not OpenAI-compatible)
- Native request format:
  ```rust
  struct NativeChatRequest {
      model: String,
      max_tokens: u32,
      system: Option<SystemPrompt>,
      messages: Vec<NativeMessage>,
      tools: Option<Vec<NativeToolSpec>>,
  }
  ```
- Tool format: `tool_use` content blocks
- Streaming: Server-Sent Events (SSE)
- Prompt caching: `cache_control` on content blocks

**OpenAI-Compatible (`compatible.rs`):**
- Uses `/v1/chat/completions` (OpenAI standard)
- Request format aligned with OpenAI spec
- Tool format: `{ type: "function", function: {...} }`
- Streaming: newline-delimited JSON
- Fallback to `/v1/responses` on 404 (if enabled)

### GLM/Zhipu Special Case

```rust
// In compatible.rs or configuration
if provider == "glm" {
    supports_responses_fallback = false;
}
```

GLM does not support the responses API and must use chat completions only.

---

## Multi-Turn Conversation Handling

### Message History Format

Unified across all providers via `ChatMessage` and `ConversationMessage`:

```rust
pub struct ChatMessage {
    pub role: String,      // "system", "user", "assistant", "tool"
    pub content: String,
}

pub enum ConversationMessage {
    Chat(ChatMessage),
    AssistantToolCalls {
        text: Option<String>,
        tool_calls: Vec<ToolCall>,
        reasoning_content: Option<String>,
    },
    ToolResults(Vec<ToolResultMessage>),
}
```

**Adaptation Per Provider:**
- **OpenAI**: Directly maps to role/content/tool_calls
- **Anthropic**: Converts to tool_use/tool_result content blocks
- **Incompatible Providers**: Merges tool results into assistant text response

### Reasoning Content Preservation

Some models (DeepSeek-R1, Kimi K2.5, GLM-4.7) return `reasoning_content`:
- Preserved in `ChatResponse.reasoning_content`
- Sent back in subsequent requests (required by some providers)
- Isolated from user-visible `text` content

---

## Streaming Support

### Provider Interface

```rust
pub trait Provider {
    fn supports_streaming(&self) -> bool { ... }
    
    fn stream_chat_with_system(
        &self,
        system: Option<&str>,
        message: &str>,
        model: &str,
        temperature: f64,
        options: StreamOptions,
    ) -> BoxStream<'static, StreamResult<StreamChunk>>;
}
```

### Stream Chunk Format

```rust
pub struct StreamChunk {
    pub delta: String,        // Text delta for this chunk
    pub is_final: bool,       // Last chunk marker
    pub token_count: usize,   // Estimated tokens (rough)
}
```

### Implementation Details

**OpenAI-Compatible:**
- Newline-delimited JSON events
- Each line: `data: {"choices":[{"delta":{"content":"text"}}]}`
- Final message: `data: [DONE]`

**Anthropic:**
- Server-Sent Events (SSE)
- Event types: `content_block_start`, `content_block_delta`, `message_stop`
- Token counts in streaming responses

---

## Provider-Specific Implementations

### OpenAI (`openai.rs`)
- **API**: `/v1/chat/completions`
- **Tool Support**: Native OpenAI function calling
- **Streaming**: ✅ Supported
- **Vision**: ✅ (via image URLs)
- **Reasoning Models**: ✅ (reasoning_content field)
- **Special**: Max tokens capped per model class

### Anthropic (`anthropic.rs`)
- **API**: Proprietary Messages API
- **Tool Support**: Native tool_use blocks
- **Streaming**: ✅ SSE-based
- **Vision**: ✅ (base64 image content blocks)
- **Prompt Caching**: ✅ (cache_control)
- **Special**: Batch API, token prediction

### OpenAI-Compatible (`compatible.rs`)
- **API**: `/v1/chat/completions` (standard)
- **Fallback**: `/v1/responses` (optional, on 404)
- **Tool Support**: Native or prompt-guided
- **Streaming**: ✅ (when enabled per provider)
- **Vision**: Configurable per provider
- **Special**: LiteLLM cache controls, merge_system_into_user

### Gemini (`gemini.rs`)
- **API**: Proprietary REST API
- **Tool Support**: Native function_declarations
- **Streaming**: ✅ (gRPC or REST streaming)
- **Vision**: ✅ (native multi-modal)
- **Special**: Fully different API structure

---

## Error Handling and Fallback Logic

### Chat Completions → Responses Fallback

```rust
// In compatible.rs
pub async fn chat_with_tools(&self, ...) -> Result<ChatResponse> {
    // Try primary API mode
    let result = self.try_primary_api_mode().await;
    
    match result {
        Ok(response) => Ok(response),
        Err(e) if e.status == 404 && self.supports_responses_fallback => {
            // Try /v1/responses on 404
            self.try_responses_api().await
        }
        Err(e) => Err(e),
    }
}
```

### Provider Capability Mismatches

**Scenario**: User requests native tool calling on a provider that only supports prompt-guided

```rust
pub async fn chat(&self, request: ChatRequest<'_>, ...) -> Result<ChatResponse> {
    if let Some(tools) = request.tools {
        if !tools.is_empty() && !self.supports_native_tools() {
            // Inject tool instructions into system prompt
            let instructions = self.convert_tools(tools);
            // Append to system prompt, call chat_with_history()
        }
    }
    // Normal chat flow
}
```

---

## Route-Level API Override (provider_api)

### Per-Route API Mode Selection

ZeroClaw now supports route-specific API protocol overrides via the `provider_api` field in `model_routes`:

```toml
[[model_routes]]
hint = "reasoning"
provider = "custom:https://example.com/v1"
model = "my-model"
provider_api = "openai_responses"  # Override per route
```

**Valid Values:**
- `openai_chat_completions` (or aliases: `chat_completions`, `openai-chat-completions`)
- `openai_responses` (or aliases: `responses`, `openai-responses`)
- `null` or omitted (use global provider default)

**Constraints:**
- Only valid for `custom:` providers
- Validation rejects `provider_api` on non-custom routes
- Both variants automatically normalized to canonical form

**Web UI Support:**
The configuration interface exposes this in `RouteListField.tsx`:
- Dropdown with options: Default, Chat Completions, Responses
- Only shown in context; applies when provider is `custom:`
- Persisted through `upsert_scenario` tool action

**Use Case:**
Route-specific overrides enable routing to the same custom endpoint with different API protocols:

```rust
// Route A: Use Chat Completions for general queries
[[model_routes]]
hint = "general"
provider = "custom:https://llm.local/v1"
model = "fast-model"
provider_api = "openai_chat_completions"

// Route B: Use Responses for structured output
[[model_routes]]
hint = "structured"
provider = "custom:https://llm.local/v1"
model = "strict-model"
provider_api = "openai_responses"
```

---

## Configuration Example

### config.toml Provider Setup

```toml
[default]
provider = "openai"
model = "gpt-4o"

[[providers]]
name = "openai"
api_key = "sk-..."
base_url = "https://api.openai.com/v1"
# Optionally disable responses fallback:
# api_mode = "chat_completions"

[[providers]]
name = "anthropic"
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com/v1"
# Anthropic uses Messages API exclusively

[[providers]]
name = "custom-openai-compat"
api_key = "key-..."
base_url = "https://custom-llm-provider.com/v1"
auth_style = "bearer"
supports_responses_fallback = false  # Disable /v1/responses
native_tool_calling = true
```

---

## Request Normalization Pipeline

```
Incoming Request
    ↓
Normalize to Provider-Agnostic Format (ChatMessage[])
    ↓
Detect Provider Type
    ↓
Select API Mode (Chat Completions vs Responses)
    ↓
Convert Tools to Provider-Specific Format
    ↓
Format Request per Provider Spec
    ↓
Add Auth Headers (Bearer, X-Api-Key, etc.)
    ↓
Send HTTP Request
    ↓
Parse Response
    ↓
Normalize to ChatResponse
    ↓
Return to Agent/Caller
```

---

## Key Differences: Chat Completions vs Responses

| Aspect | Chat Completions | Responses API |
|--------|-----------------|---------------|
| **Endpoint** | `/v1/chat/completions` | `/v1/responses` |
| **Use Case** | General chat, tool calling | Structured outputs, strict schemas |
| **Tool Calling** | Native (via tools array) | Via response_format |
| **Streaming** | Yes | Limited |
| **Model Support** | All OpenAI models | Subset (beta) |
| **Error Handling** | Standard HTTP codes | Schema validation errors |
| **Reasoning Support** | Yes (reasoning_content) | Planned |
| **Fallback** | N/A (primary) | Falls back from Responses on 404 |

---

## Best Practices for Custom Providers

1. **Declare Capabilities Truthfully**
   - Only set `native_tool_calling = true` if you actually support it
   - Tool injection will happen automatically if declared false

2. **Handle Tool Calling Format**
   - OpenAI-compatible: Use `{ type: "function", function: {...} }`
   - If not native: Allow prompt-guided fallback

3. **Support Streaming If Possible**
   - Implement newline-delimited JSON if OpenAI-compatible
   - Return `SStreamResult<StreamChunk>` from `stream_chat_with_system()`

4. **Preserve Reasoning Content**
   - Pass `reasoning_content` from API response to `ChatResponse`
   - Include it in subsequent requests if provider requires it

5. **Test Tool Calling**
   - Verify tool invocations parse correctly
   - Test both single and multiple tool calls in one response
   - Validate reasoning models preserve thinking content

---

## Examples: Adding a New Provider

### Example 1: OpenAI-Compatible (e.g., Mistral)

```rust
// In mod.rs
pub async fn create_openai_compatible(
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<Box<dyn Provider>> {
    Ok(Box::new(compatible::OpenAiCompatibleProvider::new_with_options(
        name,
        base_url,
        api_key,
        compatible::AuthStyle::Bearer,
        false,              // no vision
        true,               // supports_responses_fallback
        None,               // user_agent
        false,              // merge_system_into_user
        true,               // native_tool_calling
        CompatibleApiMode::OpenAiChatCompletions,
        None,               // max_tokens_override
    )))
}
```

### Example 2: New API (Responses-First Provider)

```rust
let provider = compatible::OpenAiCompatibleProvider::new_with_options(
    "responses-first-llm",
    "https://api.example.com",
    Some(&api_key),
    compatible::AuthStyle::Bearer,
    true,               // vision support
    false,              // NO responses fallback (we use it primarily)
    None,
    false,
    true,               // native tools
    CompatibleApiMode::OpenAiResponses,  // PRIMARY API MODE
    None,
);
```

---

## Summary

ZeroClaw's provider abstraction elegantly handles the transition from Chat Completions to Responses APIs:

- **Unified Interface**: Single `Provider` trait abstracts API differences
- **Capability Declaration**: Providers declare what they support; framework adapts
- **Intelligent Fallback**: Attempts chat completions first, falls back to responses on 404
- **Tool Calling**: Native support where available, prompt-guided fallback otherwise
- **Multi-Turn Conversations**: Reasoning content preserved, tool results tracked
- **Streaming**: Supported per provider, with consistent chunk interface
- **Route-Level Overrides**: Per-route `provider_api` selection for custom providers enables fine-grained protocol control
- **Extensibility**: Easy to add new providers by implementing the trait

The design prioritizes pragmatism: support the broadest set of providers while enabling providers to opt into newer APIs as they stabilize. Route-level API overrides allow operators to leverage multiple protocols from the same custom endpoint without duplicating provider configurations.
