use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_drf_router_actions_from_source_decorators_without_metadata() {
    let repo = temp_repo("python-drf-action-source-decorators");
    std::fs::create_dir_all(repo.join("project")).unwrap();
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("project/urls.py"),
        r#"from django.urls import include, path
from orders.urls import router

urlpatterns = [
    path("api/", include(router.urls)),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from rest_framework.routers import DefaultRouter
from . import views

router = DefaultRouter()
router.register("orders", views.OrderViewSet, basename="order")

urlpatterns = router.urls
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"from rest_framework.decorators import action

class OrderViewSet:
    def list(self, request):
        return []

    def retrieve(self, request, pk=None):
        return {"id": pk}

    @action(
        detail=True,
        methods=["post"],
        url_path="cancel")
    def cancel(self, request, pk=None):
        return {"id": pk}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "OrderViewSet",
            "orders.views.OrderViewSet",
            "orders/views.py",
        ),
        (3, 14),
        json!({"language": "python"}),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "list",
        "orders.views.OrderViewSet.list",
        "orders/views.py",
        (4, 5),
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "retrieve",
        "orders.views.OrderViewSet.retrieve",
        "orders/views.py",
        (7, 8),
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "cancel",
        "orders.views.OrderViewSet.cancel",
        "orders/views.py",
        (14, 15),
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let cancel = synth
        .routes
        .iter()
        .find(|route| route.route == "/api/orders/{id}/cancel")
        .expect("DRF action route from source decorator");
    assert_eq!(
        cancel.handler_id.as_deref(),
        Some("cbm:4:orders.views.OrderViewSet.cancel")
    );
    assert_eq!(cancel.method.as_deref(), Some("POST"));
    assert!(cancel
        .edge_recs()
        .into_iter()
        .any(|edge| edge.edge_type == "HANDLES_ROUTE"
            && edge.source_id == "cbm:4:orders.views.OrderViewSet.cancel"
            && edge.target_id == cancel.id));
}
