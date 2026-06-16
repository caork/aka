use super::*;
use serde_json::json;

#[test]
fn synthesizes_python_transaction_decorators_from_source_without_metadata() {
    let repo = temp_repo("python-transaction-source-decorators");
    std::fs::write(
        repo.join("services.py"),
        r#"from django.db import transaction

@transaction.atomic(
    savepoint=False)
def submit_order(order):
    order.save()
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "submit_order",
        "services.submit_order",
        "services.py",
        (5, 6),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let tx = synth
        .transactions
        .iter()
        .find(|tx| tx.name == "submit_order transaction")
        .expect("python transaction decorator from source");
    assert_eq!(tx.manager, "django-transaction");
    assert_eq!(tx.endpoints[0].node_id, "cbm:1:services.submit_order");
    assert!(tx.edge_recs().iter().any(|edge| {
        edge.edge_type == "HAS_TRANSACTION_BOUNDARY"
            && edge.source_id == "cbm:1:services.submit_order"
            && edge.target_id == tx.id
            && edge.evidence.as_ref().is_some_and(|evidence| {
                evidence["strategy"] == json!("python-django-transaction-decorator")
            })
    }));
}
