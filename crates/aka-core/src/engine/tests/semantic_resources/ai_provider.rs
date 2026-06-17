use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_ai_provider_resources() {
    let repo = temp_repo("configured-ai-provider-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"spring:
  ai:
    openai:
      api-key: sk-redacted
      chat:
        options:
          model: gpt-4.1-mini
    azure:
      openai:
        endpoint: https://aka-openai.openai.azure.com
anthropic:
  api-key: anth-redacted
aws:
  bedrock:
    region: us-west-2
google:
  ai:
    gemini:
      api-key: gemini-redacted
disabled:
  openai:
    api-key: ${OPENAI_API_KEY}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"CLAUDE_MODEL = "claude-3-5-sonnet"
VERTEX_AI_PROJECT = "aka-prod"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 22),
        json!({"language": "yaml"}),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 2),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_ai_provider_edge(
        &synth,
        "ai-provider:openai",
        &config_id("spring.ai.openai.api.key"),
        "openai-config",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:azure-openai",
        &config_id("spring.ai.azure.openai.endpoint"),
        "azure-openai-config",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:anthropic",
        &config_id("anthropic.api.key"),
        "anthropic-config",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:bedrock",
        &config_id("aws.bedrock.region"),
        "bedrock-config",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:gemini",
        &config_id("google.ai.gemini.api.key"),
        "gemini-config",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:anthropic",
        &config_id("claude.model"),
        "anthropic-config",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:gemini",
        &config_id("vertex.ai.project"),
        "gemini-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "ai-provider:openai"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.openai.api.key"))
    }));
}

#[test]
fn synthesizes_python_ai_provider_resources() {
    let repo = temp_repo("python-ai-provider-resources");
    std::fs::write(
        repo.join("llm.py"),
        r#"from openai import OpenAI
import anthropic
import boto3
import google.generativeai as genai

openai_client = OpenAI()
anthropic_client = anthropic.Anthropic()
bedrock = boto3.client("bedrock-runtime")
model = genai.GenerativeModel("gemini-1.5-pro")

def answer(prompt):
    return openai_client.chat.completions.create(model="gpt-4.1-mini", messages=[{"role": "user", "content": prompt}])

def summarize(prompt):
    return openai_client.responses.create(model="gpt-4.1-mini", input=prompt)

def ask_claude(prompt):
    return anthropic_client.messages.create(model="claude-3-5-sonnet-latest", messages=[{"role": "user", "content": prompt}])

def invoke_bedrock(body):
    return bedrock.invoke_model(modelId="anthropic.claude-3-sonnet", body=body)

def generate(prompt):
    return model.generate_content(prompt)

def ordinary_create(service):
    return service.chat.completions.create(payload={})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "answer",
        "llm.answer",
        "llm.py",
        (11, 12),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "summarize",
        "llm.summarize",
        "llm.py",
        (14, 15),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ask_claude",
        "llm.ask_claude",
        "llm.py",
        (17, 18),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "invoke_bedrock",
        "llm.invoke_bedrock",
        "llm.py",
        (20, 21),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "generate",
        "llm.generate",
        "llm.py",
        (23, 24),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        6,
        "ordinary_create",
        "llm.ordinary_create",
        "llm.py",
        (26, 27),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_ai_provider_edge(
        &synth,
        "ai-provider:openai",
        "cbm:1:llm.answer",
        "python-openai-chat-completions",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:openai",
        "cbm:2:llm.summarize",
        "python-openai-responses",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:anthropic",
        "cbm:3:llm.ask_claude",
        "python-anthropic-messages",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:bedrock",
        "cbm:4:llm.invoke_bedrock",
        "python-bedrock-invoke-model",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:gemini",
        "cbm:5:llm.generate",
        "python-gemini-generate-content",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:6:llm.ordinary_create"));
}

#[test]
fn synthesizes_java_ai_provider_resources() {
    let repo = temp_repo("java-ai-provider-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ai")).unwrap();
    let file = "src/main/java/com/example/ai/AiGateway.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ai;

import com.anthropic.client.AnthropicClient;
import com.anthropic.models.messages.MessageCreateParams;
import com.google.cloud.vertexai.VertexAI;
import com.google.cloud.vertexai.generativeai.GenerativeModel;
import com.openai.client.OpenAIClient;
import com.openai.models.ChatCompletionCreateParams;
import software.amazon.awssdk.services.bedrockruntime.BedrockRuntimeClient;

class AiGateway {
    Object answer(OpenAIClient client, ChatCompletionCreateParams params) {
        return client.chat().completions().create(params);
    }

    Object askClaude(AnthropicClient client, MessageCreateParams params) {
        return client.messages().create(params);
    }

    Object invokeBedrock(BedrockRuntimeClient client, Object request) {
        return client.invokeModel(request);
    }

    Object generate(VertexAI vertex, String prompt) {
        GenerativeModel model = new GenerativeModel("gemini-1.5-pro", vertex);
        return model.generateContent(prompt);
    }

    Object ordinary(Widget widget, Object params) {
        widget.messages().create(params);
        return widget.generateContent(params);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "answer",
        "com.example.ai.AiGateway.answer",
        file,
        (12, 14),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "askClaude",
        "com.example.ai.AiGateway.askClaude",
        file,
        (16, 18),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "invokeBedrock",
        "com.example.ai.AiGateway.invokeBedrock",
        file,
        (20, 22),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "generate",
        "com.example.ai.AiGateway.generate",
        file,
        (24, 27),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinary",
        "com.example.ai.AiGateway.ordinary",
        file,
        (29, 32),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_ai_provider_edge(
        &synth,
        "ai-provider:openai",
        "cbm:1:com.example.ai.AiGateway.answer",
        "java-openai-chat-completions",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:anthropic",
        "cbm:2:com.example.ai.AiGateway.askClaude",
        "java-anthropic-messages",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:bedrock",
        "cbm:3:com.example.ai.AiGateway.invokeBedrock",
        "java-bedrock-invoke-model",
    );
    assert_ai_provider_edge(
        &synth,
        "ai-provider:gemini",
        "cbm:4:com.example.ai.AiGateway.generate",
        "java-gemini-generate-content",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:5:com.example.ai.AiGateway.ordinary"));
}

fn assert_ai_provider_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected AI provider resource {url}"));
    assert_eq!(resource.resource_type, "ai-provider");
    let edges = resource.edge_recs();
    assert!(
        edges.iter().any(|edge| {
            edge.source_id == source_id
                && edge.edge_type == "ACCESSES_RESOURCE"
                && edge.evidence.as_ref().unwrap()["strategy"] == strategy
        }),
        "expected edge source={source_id} strategy={strategy}; edges={edges:#?}"
    );
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
