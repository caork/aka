use super::super::*;
use serde_json::json;

#[test]
fn extracts_django_jsonresponse_shape_keys() {
    let repo = temp_repo("django-jsonresponse-shapes");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from django.urls import path
from orders import views

urlpatterns = [
    path("orders/<int:id>/", views.get_order),
]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"from django.http import JsonResponse

def get_order(request, id):
    return JsonResponse({"id": id, "status": "ok", "total": 42, "error": None})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "orders.views.get_order",
        "orders/views.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/orders/{id}")
        .expect("Django route with JsonResponse shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
}

#[test]
fn extracts_flask_jsonify_keyword_shape_keys() {
    let repo = temp_repo("flask-jsonify-keyword-shapes");
    std::fs::create_dir_all(repo.join("api")).unwrap();
    std::fs::write(
        repo.join("api/orders.py"),
        r#"from flask import Blueprint, jsonify

bp = Blueprint("orders", __name__, url_prefix="/orders")

@bp.get("/<id>")
def get_order(id):
    return jsonify(id=id, status="ok", total=42, error=None)
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Function",
        "get_order",
        "api.orders.get_order",
        "api/orders.py",
        json!({
            "decorators": ["@bp.get(\"/<id>\")"],
            "language": "python",
            "route_method": "GET",
            "route_path": "/<id>",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/orders/{id}")
        .expect("Flask route with jsonify keyword shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
}
