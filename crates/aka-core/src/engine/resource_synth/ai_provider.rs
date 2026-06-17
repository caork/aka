use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_ai_provider_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_ai_context(text) {
        out.extend(extract_python_ai_provider_resources(text, nodes));
    }
    if has_java_ai_context(text) {
        out.extend(extract_java_ai_provider_resources(text, nodes));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

pub(super) fn extract_ai_provider_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = ai_provider_for_config_key(&key) else {
            continue;
        };
        if !ai_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::ai_provider(
            provider.into(),
            infra_config::config_id(&key),
            ai_config_strategy(provider),
        ));
    }
    out.sort_by(|a, b| {
        a.url
            .cmp(&b.url)
            .then_with(|| a.node_id.cmp(&b.node_id))
            .then_with(|| a.strategy.cmp(&b.strategy))
    });
    out.dedup_by(|a, b| a.url == b.url && a.node_id == b.node_id && a.strategy == b.strategy);
    out
}

fn has_python_ai_context(text: &str) -> bool {
    text.contains("openai")
        || text.contains("OpenAI(")
        || text.contains("AzureOpenAI(")
        || text.contains("anthropic")
        || text.contains("Anthropic(")
        || text.contains("bedrock-runtime")
        || text.contains("google.generativeai")
        || text.contains("vertexai")
}

fn extract_python_ai_provider_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("openai") || text.contains("OpenAI(") || text.contains("AzureOpenAI(") {
        for (callee, strategy) in [
            (".chat.completions.create", "python-openai-chat-completions"),
            (".responses.create", "python-openai-responses"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(receiver) = receiver_before_dot(text, call.start) else {
                    continue;
                };
                if !is_python_openai_receiver(text, receiver) {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::ai_provider(
                    "openai".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("anthropic") || text.contains("Anthropic(") {
        for (callee, strategy) in [
            (".messages.create", "python-anthropic-messages"),
            (".beta.messages.create", "python-anthropic-beta-messages"),
        ] {
            for call in find_call_args(text, callee) {
                let Some(receiver) = receiver_before_dot(text, call.start) else {
                    continue;
                };
                if !is_python_anthropic_receiver(text, receiver) {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::ai_provider(
                    "anthropic".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("bedrock-runtime") || text.contains("BedrockRuntime") {
        for call in find_call_args(text, ".invoke_model") {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !is_python_bedrock_receiver(text, receiver) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::ai_provider(
                "bedrock".into(),
                node.aka_id.clone(),
                "python-bedrock-invoke-model",
            ));
        }
    }
    if text.contains("google.generativeai") || text.contains("vertexai") {
        for (callee, strategy) in [
            (".generate_content", "python-gemini-generate-content"),
            (
                ".generate_content_async",
                "python-gemini-generate-content-async",
            ),
        ] {
            for call in find_call_args(text, callee) {
                let Some(receiver) = receiver_before_dot(text, call.start) else {
                    continue;
                };
                if !is_python_gemini_receiver(text, receiver) {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::ai_provider(
                    "gemini".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    out
}

fn has_java_ai_context(text: &str) -> bool {
    text.contains("com.openai")
        || text.contains("OpenAIClient")
        || text.contains("OpenAIOkHttpClient")
        || text.contains("ChatCompletionCreateParams")
        || text.contains("AnthropicClient")
        || text.contains("MessageCreateParams")
        || text.contains("BedrockRuntimeClient")
        || text.contains("software.amazon.awssdk.services.bedrockruntime")
        || text.contains("VertexAI")
        || text.contains("GenerativeModel")
}

fn extract_java_ai_provider_resources(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if text.contains("com.openai")
        || text.contains("OpenAIClient")
        || text.contains("OpenAIOkHttpClient")
        || text.contains("ChatCompletionCreateParams")
    {
        for (anchor, strategy) in [
            (
                ".chat().completions().create",
                "java-openai-chat-completions",
            ),
            (".chatCompletions().create", "java-openai-chat-completions"),
            (".responses().create", "java-openai-responses"),
        ] {
            for call in find_call_args(text, anchor) {
                let Some(receiver) = receiver_before_dot(text, call.start) else {
                    continue;
                };
                if !is_java_typed_receiver(text, receiver, &["OpenAIClient", "OpenAIOkHttpClient"])
                {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::ai_provider(
                    "openai".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
        for call in find_call_args(text, ".create") {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !is_java_typed_receiver(text, receiver, &["OpenAIClient", "OpenAIOkHttpClient"])
                && !has_window_context(
                    text,
                    call.start,
                    call.args.len(),
                    &["ChatCompletionCreateParams", "ResponseCreateParams"],
                )
            {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::ai_provider(
                "openai".into(),
                node.aka_id.clone(),
                "java-openai-create-params",
            ));
        }
    }
    if text.contains("AnthropicClient") || text.contains("MessageCreateParams") {
        for (anchor, strategy) in [
            (".messages().create", "java-anthropic-messages"),
            (
                ".messages().batches().create",
                "java-anthropic-message-batches",
            ),
        ] {
            for call in find_call_args(text, anchor) {
                let Some(receiver) = receiver_before_dot(text, call.start) else {
                    continue;
                };
                if !is_java_typed_receiver(text, receiver, &["AnthropicClient"]) {
                    continue;
                }
                let Some(node) = node_at_offset(text, nodes, call.start) else {
                    continue;
                };
                out.push(ResourceDetection::ai_provider(
                    "anthropic".into(),
                    node.aka_id.clone(),
                    strategy,
                ));
            }
        }
    }
    if text.contains("BedrockRuntimeClient")
        || text.contains("software.amazon.awssdk.services.bedrockruntime")
    {
        for call in find_call_args(text, ".invokeModel") {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !is_java_typed_receiver(text, receiver, &["BedrockRuntimeClient"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::ai_provider(
                "bedrock".into(),
                node.aka_id.clone(),
                "java-bedrock-invoke-model",
            ));
        }
    }
    if text.contains("VertexAI") || text.contains("GenerativeModel") {
        for call in find_call_args(text, ".generateContent") {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !is_java_typed_receiver(text, receiver, &["GenerativeModel"]) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::ai_provider(
                "gemini".into(),
                node.aka_id.clone(),
                "java-gemini-generate-content",
            ));
        }
    }
    out
}

fn is_python_openai_receiver(text: &str, receiver: &str) -> bool {
    let receiver = receiver_tail(receiver);
    receiver == "openai"
        || python_receiver_assigned_to(text, receiver, &["OpenAI(", "AzureOpenAI("])
}

fn is_python_anthropic_receiver(text: &str, receiver: &str) -> bool {
    let receiver = receiver_tail(receiver);
    receiver == "anthropic"
        || python_receiver_assigned_to(text, receiver, &["anthropic.Anthropic(", "Anthropic("])
}

fn is_python_bedrock_receiver(text: &str, receiver: &str) -> bool {
    python_receiver_assigned_to(
        text,
        receiver_tail(receiver),
        &[
            "boto3.client(\"bedrock-runtime\"",
            "boto3.client('bedrock-runtime'",
        ],
    )
}

fn is_python_gemini_receiver(text: &str, receiver: &str) -> bool {
    python_receiver_assigned_to(
        text,
        receiver_tail(receiver),
        &["GenerativeModel(", "genai.GenerativeModel("],
    )
}

fn python_receiver_assigned_to(text: &str, receiver: &str, constructors: &[&str]) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver && constructors.iter().any(|ctor| rhs.contains(ctor))
    })
}

fn is_java_typed_receiver(text: &str, receiver: &str, types: &[&str]) -> bool {
    let receiver = receiver_tail(receiver);
    text.lines().any(|line| {
        let line = line.trim();
        types.iter().any(|ty| {
            line.contains(&format!("{ty} {receiver}"))
                || line.contains(&format!("{ty} {receiver},"))
                || line.contains(&format!("{ty} {receiver})"))
                || line.contains(&format!("{ty} {receiver} ="))
        })
    })
}

fn receiver_before_dot(text: &str, dot_start: usize) -> Option<&str> {
    if text.as_bytes().get(dot_start) != Some(&b'.') {
        return None;
    }
    let mut start = dot_start;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if ch == '.' || ch == '_' || ch == '$' || ch.is_ascii_alphanumeric() {
            start -= ch.len_utf8();
        } else {
            break;
        }
    }
    let receiver = text[start..dot_start].trim_matches('.');
    (!receiver.is_empty()).then_some(receiver)
}

fn receiver_tail(receiver: &str) -> &str {
    receiver.rsplit('.').next().unwrap_or(receiver)
}

fn has_window_context(text: &str, start: usize, args_len: usize, needles: &[&str]) -> bool {
    let window_start = start.saturating_sub(900);
    let window_end = (start + args_len + 900).min(text.len());
    let window = &text[window_start..window_end];
    needles.iter().any(|needle| window.contains(needle))
}

fn ai_provider_for_config_key(key: &str) -> Option<&'static str> {
    if key_contains_any(key, &["azure.openai", "azure.open.ai", "aoai"]) {
        Some("azure-openai")
    } else if key_contains_any(key, &["openai", "open.ai"]) {
        Some("openai")
    } else if key_contains_any(key, &["anthropic", "claude"]) {
        Some("anthropic")
    } else if key_contains_any(key, &["aws.bedrock", "bedrock"]) {
        Some("bedrock")
    } else if key_contains_any(key, &["gemini", "google.ai", "vertex.ai", "vertexai"]) {
        Some("gemini")
    } else {
        None
    }
}

fn ai_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn ai_config_strategy(provider: &str) -> &'static str {
    match provider {
        "azure-openai" => "azure-openai-config",
        "openai" => "openai-config",
        "anthropic" => "anthropic-config",
        "bedrock" => "bedrock-config",
        "gemini" => "gemini-config",
        _ => "ai-provider-config",
    }
}

fn key_contains_any(key: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| {
        if needle.contains('.') && key.contains(needle) {
            return true;
        }
        key.split('.')
            .any(|part| part == *needle || part.contains(needle))
    })
}
