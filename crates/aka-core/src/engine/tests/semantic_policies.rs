use super::*;
use serde_json::json;

#[test]
fn synthesizes_python_policies_from_source_decorators_without_metadata() {
    let repo = temp_repo("python-policy-source-decorators");
    std::fs::write(
        repo.join("api.py"),
        r#"from django.contrib.auth.decorators import permission_required, user_passes_test

def can_cancel_order(user):
    return user.is_staff

@permission_required(
    "orders.view_order")
def list_orders(request):
    return []

@user_passes_test(can_cancel_order)
def cancel_order(request):
    return {}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "list_orders",
        "api.list_orders",
        "api.py",
        (8, 9),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "cancel_order",
        "api.cancel_order",
        "api.py",
        (12, 13),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let permission = synth
        .policies
        .iter()
        .find(|policy| policy.name == "orders.view_order")
        .expect("django permission policy from source decorator");
    assert_eq!(permission.policy_type, "permission");
    assert!(permission.edge_recs().iter().any(|edge| {
        edge.edge_type == "REQUIRES_POLICY"
            && edge.source_id == "cbm:1:api.list_orders"
            && edge
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence["strategy"] == json!("python-permission-required"))
    }));

    let predicate = synth
        .policies
        .iter()
        .find(|policy| policy.name == "can_cancel_order")
        .expect("django predicate policy from source decorator");
    assert_eq!(predicate.policy_type, "predicate");
    assert!(predicate.edge_recs().iter().any(|edge| {
        edge.edge_type == "REQUIRES_POLICY"
            && edge.source_id == "cbm:2:api.cancel_order"
            && edge
                .evidence
                .as_ref()
                .is_some_and(|evidence| evidence["strategy"] == json!("python-user-passes-test"))
    }));
}
