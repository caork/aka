use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_python_identity_resources() {
    let repo = temp_repo("python-identity-resources");
    std::fs::write(
        repo.join("auth.py"),
        r#"import boto3
import jwt
from jwt import PyJWKClient

def verify_auth0(token):
    jwks = PyJWKClient("https://example.auth0.com/.well-known/jwks.json")
    key = jwks.get_signing_key_from_jwt(token)
    return jwt.decode(token, key.key, algorithms=["RS256"], issuer="https://example.auth0.com/")

def load_cognito():
    return boto3.client("cognito-idp", region_name="us-east-1")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "verify_auth0",
        "auth.verify_auth0",
        "auth.py",
        (5, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_cognito",
        "auth.load_cognito",
        "auth.py",
        (10, 11),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_identity_caller(&synth, "identity:auth0", "cbm:1:auth.verify_auth0");
    assert_identity_edge(
        &synth,
        "identity:cognito",
        "cbm:2:auth.load_cognito",
        "python-boto3-cognito-idp",
    );
}

#[test]
fn synthesizes_java_identity_resources() {
    let repo = temp_repo("java-identity-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/security")).unwrap();
    let file = "src/main/java/com/example/security/SecurityConfig.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.security;

import org.springframework.security.oauth2.jwt.JwtDecoder;
import org.springframework.security.oauth2.jwt.JwtDecoders;
import org.springframework.security.oauth2.jwt.NimbusJwtDecoder;
import software.amazon.awssdk.services.cognitoidentityprovider.CognitoIdentityProviderClient;

class SecurityConfig {
    JwtDecoder auth0Decoder() {
        return JwtDecoders.fromIssuerLocation("https://tenant.auth0.com/");
    }

    JwtDecoder keycloakDecoder() {
        return NimbusJwtDecoder.withJwkSetUri("https://keycloak.example.com/realms/acme/protocol/openid-connect/certs").build();
    }

    CognitoIdentityProviderClient cognito() {
        return CognitoIdentityProviderClient.builder().build();
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "auth0Decoder",
        "com.example.security.SecurityConfig.auth0Decoder",
        file,
        (9, 11),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "keycloakDecoder",
        "com.example.security.SecurityConfig.keycloakDecoder",
        file,
        (13, 15),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "cognito",
        "com.example.security.SecurityConfig.cognito",
        file,
        (17, 19),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_identity_edge(
        &synth,
        "identity:auth0",
        "cbm:1:com.example.security.SecurityConfig.auth0Decoder",
        "java-spring-jwt-issuer",
    );
    assert_identity_edge(
        &synth,
        "identity:keycloak",
        "cbm:2:com.example.security.SecurityConfig.keycloakDecoder",
        "java-spring-jwk-set-uri",
    );
    assert_identity_edge(
        &synth,
        "identity:cognito",
        "cbm:3:com.example.security.SecurityConfig.cognito",
        "java-aws-cognito-client",
    );
}

fn assert_identity_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected identity resource {url}"));
    assert_eq!(resource.resource_type, "identity");
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn assert_identity_caller(synth: &SynthGraph, url: &str, source_id: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected identity resource {url}"));
    assert_eq!(resource.resource_type, "identity");
    assert!(resource
        .edge_recs()
        .iter()
        .any(|edge| { edge.source_id == source_id && edge.edge_type == "ACCESSES_RESOURCE" }));
}
