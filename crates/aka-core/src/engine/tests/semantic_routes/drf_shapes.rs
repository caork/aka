use super::super::*;
use serde_json::json;

#[test]
fn extracts_drf_viewset_serializer_shape_keys() {
    let repo = temp_repo("python-drf-serializer-response-shapes");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from rest_framework.routers import DefaultRouter
from orders import views

router = DefaultRouter()
router.register("orders", views.OrderViewSet, basename="order")
urlpatterns = router.urls
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"from rest_framework import serializers, viewsets

class OrderSerializer(serializers.Serializer):
    id = serializers.CharField()
    status = serializers.CharField()
    total = serializers.IntegerField()
    error = serializers.CharField(required=False)

class OrderViewSet(viewsets.ViewSet):
    serializer_class = OrderSerializer

    def list(self, request):
        return Response([])

    def retrieve(self, request, pk=None):
        return Response({})
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import requests

def sync_order(order_id: str):
    response = requests.get(f"http://orders.internal/api/orders/{order_id}")
    data = response.json()
    return data["status"] + data["missing"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderViewSet",
        "orders.views.OrderViewSet",
        "orders/views.py",
        json!({
            "language": "python",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "retrieve",
        "orders.views.OrderViewSet.retrieve",
        "orders/views.py",
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );
    insert_node_props(
        &conn,
        3,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/orders/{id}")
        .expect("DRF detail route with serializer response shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
    assert!(route.consumers[0].keys.contains(&"missing".to_string()));
}

#[test]
fn extracts_drf_model_serializer_meta_fields_shape_keys() {
    let repo = temp_repo("python-drf-model-serializer-response-shapes");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::create_dir_all(repo.join("workers")).unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from rest_framework.routers import DefaultRouter
from orders import views

router = DefaultRouter()
router.register("orders", views.OrderViewSet, basename="order")
urlpatterns = router.urls
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"from rest_framework import serializers, viewsets

class OrderSerializer(serializers.ModelSerializer):
    class Meta:
        model = Order
        fields = ("id", "status", "total", "error")

class OrderViewSet(viewsets.ModelViewSet):
    serializer_class = OrderSerializer

    def retrieve(self, request, pk=None):
        return Response({})
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("workers/sync.py"),
        r#"import requests

def sync_order(order_id: str):
    response = requests.get(f"http://orders.internal/api/orders/{order_id}")
    data = response.json()
    return data["status"] + data["missing"]
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderViewSet",
        "orders.views.OrderViewSet",
        "orders/views.py",
        json!({
            "language": "python",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "retrieve",
        "orders.views.OrderViewSet.retrieve",
        "orders/views.py",
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );
    insert_node_props(
        &conn,
        3,
        "Function",
        "sync_order",
        "workers.sync.sync_order",
        "workers/sync.py",
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/orders/{id}")
        .expect("DRF detail route with ModelSerializer response shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
    assert_eq!(route.consumers.len(), 1);
    assert!(route.consumers[0].keys.contains(&"status".to_string()));
    assert!(route.consumers[0].keys.contains(&"missing".to_string()));
}

#[test]
fn extracts_drf_dynamic_serializer_shape_keys() {
    let repo = temp_repo("python-drf-dynamic-serializer-response-shapes");
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join("orders/urls.py"),
        r#"from rest_framework.routers import DefaultRouter
from orders import views

router = DefaultRouter()
router.register("orders", views.OrderViewSet, basename="order")
urlpatterns = router.urls
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("orders/views.py"),
        r#"from rest_framework import serializers, viewsets

class OrderListSerializer(serializers.Serializer):
    id = serializers.CharField()
    status = serializers.CharField()

class OrderDetailSerializer(serializers.Serializer):
    id = serializers.CharField()
    status = serializers.CharField()
    total = serializers.IntegerField()
    error = serializers.CharField(required=False)

class OrderViewSet(viewsets.ModelViewSet):
    serializer_action_classes = {
        "list": OrderListSerializer,
        "retrieve": OrderDetailSerializer,
    }

    def get_serializer_class(self):
        if self.action == "retrieve":
            return OrderDetailSerializer
        return self.serializer_action_classes.get(self.action, OrderListSerializer)

    def retrieve(self, request, pk=None):
        return Response({})
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props(
        &conn,
        1,
        "Class",
        "OrderViewSet",
        "orders.views.OrderViewSet",
        "orders/views.py",
        json!({
            "language": "python",
        }),
    );
    insert_node_props(
        &conn,
        2,
        "Function",
        "retrieve",
        "orders.views.OrderViewSet.retrieve",
        "orders/views.py",
        json!({
            "language": "python",
            "parent_class": "OrderViewSet",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let route = synth
        .routes
        .iter()
        .find(|route| route.route == "/orders/{id}")
        .expect("DRF detail route with dynamic serializer response shape");
    assert!(route.response_keys.contains(&"id".to_string()));
    assert!(route.response_keys.contains(&"status".to_string()));
    assert!(route.response_keys.contains(&"total".to_string()));
    assert!(route.error_keys.contains(&"error".to_string()));
}
