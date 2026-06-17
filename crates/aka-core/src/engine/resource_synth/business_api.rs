use super::{infra_config, ResourceDetection};
use crate::engine::{find_call_args, node_at_offset, SynthNode};

pub(super) fn extract_business_api_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    if has_python_business_api_context(text) {
        out.extend(extract_python_business_api_resources(text, nodes));
    }
    if has_java_business_api_context(text) {
        out.extend(extract_java_business_api_resources(text, nodes));
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

pub(super) fn extract_business_api_config_resources(text: &str) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (key, value) in infra_config::config_pairs(text) {
        let Some(provider) = business_provider_for_config_key(&key) else {
            continue;
        };
        if !business_config_value_is_present(&value) {
            continue;
        }
        out.push(ResourceDetection::business_api(
            provider.into(),
            infra_config::config_id(&key),
            business_config_strategy(provider),
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

fn has_python_business_api_context(text: &str) -> bool {
    text.contains("simple_salesforce")
        || text.contains("Salesforce(")
        || text.contains("hubspot")
        || text.contains("HubSpot(")
        || text.contains("zenpy")
        || text.contains("Zendesk")
        || text.contains("jira")
        || text.contains("JIRA(")
        || text.contains("linear_client")
        || text.contains("LinearClient")
        || text.contains("intercom")
}

fn extract_python_business_api_resources(
    text: &str,
    nodes: &[&SynthNode],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "salesforce",
        &["Salesforce("],
        &[
            (".query", "python-salesforce-query"),
            (".create", "python-salesforce-create"),
            (".update", "python-salesforce-update"),
            (".delete", "python-salesforce-delete"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "hubspot",
        &["HubSpot("],
        &[
            (
                ".crm.contacts.basic_api.create",
                "python-hubspot-contact-create",
            ),
            (
                ".crm.contacts.basic_api.get_by_id",
                "python-hubspot-contact-read",
            ),
            (".crm.deals.basic_api.create", "python-hubspot-deal-create"),
            (
                ".crm.tickets.basic_api.create",
                "python-hubspot-ticket-create",
            ),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "zendesk",
        &["Zenpy(", "Zendesk("],
        &[
            (".tickets.create", "python-zendesk-ticket-create"),
            (".tickets.update", "python-zendesk-ticket-update"),
            (".tickets.delete", "python-zendesk-ticket-delete"),
            (".users.create", "python-zendesk-user-create"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "jira",
        &["JIRA("],
        &[
            (".create_issue", "python-jira-create-issue"),
            (".issue", "python-jira-read-issue"),
            (".search_issues", "python-jira-search-issues"),
            (".add_comment", "python-jira-add-comment"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "linear",
        &["LinearClient(", "Linear("],
        &[
            (".issue_create", "python-linear-issue-create"),
            (".issues", "python-linear-issues"),
        ],
    ));
    out.extend(extract_python_provider_calls(
        text,
        nodes,
        "intercom",
        &["Intercom(", "Client("],
        &[
            (".contacts.create", "python-intercom-contact-create"),
            (".conversations.reply", "python-intercom-conversation-reply"),
        ],
    ));
    out
}

fn extract_python_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    constructors: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !python_receiver_is_provider(text, receiver, provider, constructors) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::business_api(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn has_java_business_api_context(text: &str) -> bool {
    text.contains("PartnerConnection")
        || text.contains("EnterpriseConnection")
        || text.contains("HubSpot")
        || text.contains("Zendesk")
        || text.contains("JiraRestClient")
        || text.contains("LinearClient")
        || text.contains("Intercom")
}

fn extract_java_business_api_resources(text: &str, nodes: &[&SynthNode]) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "salesforce",
        &["PartnerConnection", "EnterpriseConnection"],
        &[
            (".query", "java-salesforce-query"),
            (".create", "java-salesforce-create"),
            (".update", "java-salesforce-update"),
            (".delete", "java-salesforce-delete"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "hubspot",
        &["HubSpotClient", "HubSpot"],
        &[
            (".createContact", "java-hubspot-contact-create"),
            (".createDeal", "java-hubspot-deal-create"),
            (".getContact", "java-hubspot-contact-read"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "zendesk",
        &["Zendesk", "ZendeskClient"],
        &[
            (".createTicket", "java-zendesk-ticket-create"),
            (".updateTicket", "java-zendesk-ticket-update"),
            (".deleteTicket", "java-zendesk-ticket-delete"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "jira",
        &["JiraRestClient", "JiraClient"],
        &[
            (".createIssue", "java-jira-create-issue"),
            (".getIssue", "java-jira-read-issue"),
            (".searchJql", "java-jira-search-jql"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "linear",
        &["LinearClient"],
        &[
            (".createIssue", "java-linear-create-issue"),
            (".issues", "java-linear-issues"),
        ],
    ));
    out.extend(extract_java_provider_calls(
        text,
        nodes,
        "intercom",
        &["Intercom", "IntercomClient"],
        &[
            (".createContact", "java-intercom-contact-create"),
            (".replyToConversation", "java-intercom-conversation-reply"),
        ],
    ));
    out
}

fn extract_java_provider_calls(
    text: &str,
    nodes: &[&SynthNode],
    provider: &str,
    types: &[&str],
    methods: &[(&str, &str)],
) -> Vec<ResourceDetection> {
    let mut out = Vec::new();
    for (callee, strategy) in methods {
        for call in find_call_args(text, callee) {
            let Some(receiver) = receiver_before_dot(text, call.start) else {
                continue;
            };
            if !java_receiver_has_type(text, receiver, types) {
                continue;
            }
            let Some(node) = node_at_offset(text, nodes, call.start) else {
                continue;
            };
            out.push(ResourceDetection::business_api(
                provider.into(),
                node.aka_id.clone(),
                *strategy,
            ));
        }
    }
    out
}

fn python_receiver_is_provider(
    text: &str,
    receiver: &str,
    provider: &str,
    constructors: &[&str],
) -> bool {
    let receiver = receiver_tail(receiver);
    receiver.to_ascii_lowercase().contains(provider)
        || python_receiver_assigned_to(text, receiver, constructors)
        || python_receiver_from_provider_member(text, receiver, provider)
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

fn python_receiver_from_provider_member(text: &str, receiver: &str, provider: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line.trim();
        let Some((lhs, rhs)) = trimmed.split_once('=') else {
            return false;
        };
        let lhs = lhs
            .trim()
            .trim_start_matches("self.")
            .trim_start_matches("cls.");
        lhs == receiver
            && match provider {
                "salesforce" => rhs.contains(".Salesforce("),
                "zendesk" => rhs.contains(".Zendesk(") || rhs.contains(".Zenpy("),
                "intercom" => rhs.contains(".Intercom("),
                _ => false,
            }
    })
}

fn java_receiver_has_type(text: &str, receiver: &str, types: &[&str]) -> bool {
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

fn business_provider_for_config_key(key: &str) -> Option<&'static str> {
    if key_contains_any(key, &["salesforce", "sfdc"]) {
        Some("salesforce")
    } else if key_contains_any(key, &["hubspot"]) {
        Some("hubspot")
    } else if key_contains_any(key, &["zendesk"]) {
        Some("zendesk")
    } else if key_contains_any(key, &["jira", "atlassian"]) {
        Some("jira")
    } else if key_contains_any(key, &["linear"]) {
        Some("linear")
    } else if key_contains_any(key, &["intercom"]) {
        Some("intercom")
    } else {
        None
    }
}

fn business_config_value_is_present(value: &str) -> bool {
    let value = value.trim().trim_matches(['"', '\'', '`']);
    !value.is_empty()
        && !value.starts_with("${")
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "false" | "none" | "null" | "0"
        )
}

fn business_config_strategy(provider: &str) -> &'static str {
    match provider {
        "salesforce" => "salesforce-config",
        "hubspot" => "hubspot-config",
        "zendesk" => "zendesk-config",
        "jira" => "jira-config",
        "linear" => "linear-config",
        "intercom" => "intercom-config",
        _ => "business-api-config",
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
