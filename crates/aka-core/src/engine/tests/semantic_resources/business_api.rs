use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_configured_business_api_resources() {
    let repo = temp_repo("configured-business-api-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"integrations:
  salesforce:
    client-id: sf-client
  hubspot:
    access-token: hubspot-token
  zendesk:
    subdomain: aka
  jira:
    base-url: https://jira.example
  linear:
    api-key: lin-redacted
  intercom:
    token: ic-redacted
disabled:
  hubspot:
    access-token: ${HUBSPOT_TOKEN}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"SFDC_LOGIN_URL = "https://login.salesforce.com"
ATLASSIAN_SITE = "https://aka.atlassian.net"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 18),
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
    assert_business_api_edge(
        &synth,
        "business-api:salesforce",
        &config_id("integrations.salesforce.client.id"),
        "salesforce-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:hubspot",
        &config_id("integrations.hubspot.access.token"),
        "hubspot-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:zendesk",
        &config_id("integrations.zendesk.subdomain"),
        "zendesk-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:jira",
        &config_id("integrations.jira.base.url"),
        "jira-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:linear",
        &config_id("integrations.linear.api.key"),
        "linear-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:intercom",
        &config_id("integrations.intercom.token"),
        "intercom-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:salesforce",
        &config_id("sfdc.login.url"),
        "salesforce-config",
    );
    assert_business_api_edge(
        &synth,
        "business-api:jira",
        &config_id("atlassian.site"),
        "jira-config",
    );
    assert!(!synth.resources.iter().any(|resource| {
        resource.url == "business-api:hubspot"
            && resource
                .edge_recs()
                .iter()
                .any(|edge| edge.source_id == config_id("disabled.hubspot.access.token"))
    }));
}

#[test]
fn synthesizes_python_business_api_resources() {
    let repo = temp_repo("python-business-api-resources");
    std::fs::write(
        repo.join("integrations.py"),
        r#"from simple_salesforce import Salesforce
from hubspot import HubSpot
from zenpy import Zenpy
from jira import JIRA
from linear_client import LinearClient

sf = Salesforce(username="u", password="p", security_token="t")
hubspot = HubSpot(access_token="token")
zendesk = Zenpy(**{"token": "token"})
jira = JIRA(server="https://jira.example", token_auth="token")
linear = LinearClient(api_key="token")

def sync_accounts():
    return sf.query("SELECT Id FROM Account")

def create_contact(payload):
    return hubspot.crm.contacts.basic_api.create(payload)

def open_ticket(ticket):
    return zendesk.tickets.create(ticket)

def create_issue(fields):
    return jira.create_issue(fields=fields)

def create_linear_issue(input):
    return linear.issue_create(input)

def ordinary(client, tickets):
    client.query("select *")
    tickets.create({})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "sync_accounts",
        "integrations.sync_accounts",
        "integrations.py",
        (13, 14),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "create_contact",
        "integrations.create_contact",
        "integrations.py",
        (16, 17),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "open_ticket",
        "integrations.open_ticket",
        "integrations.py",
        (19, 20),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "create_issue",
        "integrations.create_issue",
        "integrations.py",
        (22, 23),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "create_linear_issue",
        "integrations.create_linear_issue",
        "integrations.py",
        (25, 26),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        6,
        "ordinary",
        "integrations.ordinary",
        "integrations.py",
        (28, 30),
        json!({"language": "python"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_business_api_edge(
        &synth,
        "business-api:salesforce",
        "cbm:1:integrations.sync_accounts",
        "python-salesforce-query",
    );
    assert_business_api_edge(
        &synth,
        "business-api:hubspot",
        "cbm:2:integrations.create_contact",
        "python-hubspot-contact-create",
    );
    assert_business_api_edge(
        &synth,
        "business-api:zendesk",
        "cbm:3:integrations.open_ticket",
        "python-zendesk-ticket-create",
    );
    assert_business_api_edge(
        &synth,
        "business-api:jira",
        "cbm:4:integrations.create_issue",
        "python-jira-create-issue",
    );
    assert_business_api_edge(
        &synth,
        "business-api:linear",
        "cbm:5:integrations.create_linear_issue",
        "python-linear-issue-create",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| edge.source_id == "cbm:6:integrations.ordinary"));
}

#[test]
fn synthesizes_java_business_api_resources() {
    let repo = temp_repo("java-business-api-resources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/integrations")).unwrap();
    let file = "src/main/java/com/example/integrations/BusinessIntegrations.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.integrations;

import com.atlassian.jira.rest.client.api.JiraRestClient;
import com.sforce.soap.partner.PartnerConnection;

class BusinessIntegrations {
    Object salesforce(PartnerConnection connection, String soql) throws Exception {
        return connection.query(soql);
    }

    Object jira(JiraRestClient jiraClient, Object input) {
        return jiraClient.createIssue(input);
    }

    Object zendesk(ZendeskClient zendeskClient, Object ticket) {
        return zendeskClient.createTicket(ticket);
    }

    Object hubspot(HubSpotClient hubSpotClient, Object contact) {
        return hubSpotClient.createContact(contact);
    }

    Object ordinary(Client client, Object input) {
        client.query(input);
        return client.createIssue(input);
    }
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "salesforce",
        "com.example.integrations.BusinessIntegrations.salesforce",
        file,
        (7, 9),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "jira",
        "com.example.integrations.BusinessIntegrations.jira",
        file,
        (11, 13),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "zendesk",
        "com.example.integrations.BusinessIntegrations.zendesk",
        file,
        (15, 17),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "hubspot",
        "com.example.integrations.BusinessIntegrations.hubspot",
        file,
        (19, 21),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "ordinary",
        "com.example.integrations.BusinessIntegrations.ordinary",
        file,
        (23, 26),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_business_api_edge(
        &synth,
        "business-api:salesforce",
        "cbm:1:com.example.integrations.BusinessIntegrations.salesforce",
        "java-salesforce-query",
    );
    assert_business_api_edge(
        &synth,
        "business-api:jira",
        "cbm:2:com.example.integrations.BusinessIntegrations.jira",
        "java-jira-create-issue",
    );
    assert_business_api_edge(
        &synth,
        "business-api:zendesk",
        "cbm:3:com.example.integrations.BusinessIntegrations.zendesk",
        "java-zendesk-ticket-create",
    );
    assert_business_api_edge(
        &synth,
        "business-api:hubspot",
        "cbm:4:com.example.integrations.BusinessIntegrations.hubspot",
        "java-hubspot-contact-create",
    );
    assert!(!synth
        .resources
        .iter()
        .flat_map(|resource| resource.edge_recs())
        .any(|edge| {
            edge.source_id == "cbm:5:com.example.integrations.BusinessIntegrations.ordinary"
        }));
}

fn assert_business_api_edge(synth: &SynthGraph, url: &str, source_id: &str, strategy: &str) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected business API resource {url}"));
    assert_eq!(resource.resource_type, "business-api");
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
