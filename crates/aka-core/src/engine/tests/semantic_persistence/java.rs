use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_java_persistence_tables_and_repositories() {
    let repo = temp_repo("java-persistence");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/Order.java"),
        r#"package com.example.orders;

import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
    @Column(name = "status")
    String status;
}"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("src/main/java/com/example/orders/OrderRepository.java"),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;

interface OrderRepository extends JpaRepository<Order, Long> {
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "Order",
            "com.example.orders.Order",
            "src/main/java/com/example/orders/Order.java",
        ),
        (8, 12),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            "src/main/java/com/example/orders/OrderRepository.java",
        ),
        (5, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Repository"
            && node.properties.get("entityName").and_then(Value::as_str) == Some("Order")
    }));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MAPS_TO_TABLE".to_string()));
    assert!(edge_types.contains(&"MANAGES_ENTITY".to_string()));
    assert!(edge_types.contains(&"REPOSITORY_FOR".to_string()));
}

#[test]
fn synthesizes_java_persistence_tables_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-persistence-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(
    name = "orders")
class Order {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;

interface OrderRepository extends JpaRepository<Order, Long> {
}"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (8, 9),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            repo_file,
        ),
        (5, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let nodes = synth.persistence.node_recs();
    assert!(nodes.iter().any(|node| {
        node.label == "Table"
            && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
    }));
    assert!(nodes.iter().any(|node| {
        node.label == "Repository"
            && node.properties.get("entityName").and_then(Value::as_str) == Some("Order")
    }));
    let edge_types: Vec<_> = synth
        .persistence
        .edge_recs()
        .into_iter()
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"MAPS_TO_TABLE".to_string()));
    assert!(edge_types.contains(&"REPOSITORY_FOR".to_string()));
}

#[test]
fn synthesizes_java_table_access_edges() {
    let repo = temp_repo("java-table-access");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;

interface OrderRepository extends JpaRepository<Order, Long> {
    @Query(value = "select * from orders where status = ?1", nativeQuery = true)
    List<Order> findNative(String status);
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;

class OrderService {
    void cancelOrders(EntityManager em) {
        em.createNativeQuery("update orders set status = 'CANCELLED'");
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 9),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findNative",
        "com.example.orders.OrderRepository.findNative",
        repo_file,
        (7, 8),
        json!({
            "decorators": ["@Query(value = \"select * from orders where status = ?1\", nativeQuery = true)"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        6,
        (
            "Class",
            "OrderService",
            "com.example.orders.OrderService",
            service_file,
        ),
        (3, 21),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "cancelOrders",
        "com.example.orders.OrderService.cancelOrders",
        service_file,
        (4, 6),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderRepository.findNative"
            && edge.target_id == table_id
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderService.cancelOrders"
            && edge.target_id == table_id
    }));
}

#[test]
fn synthesizes_java_annotation_table_access_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-table-access-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    let mapper_file = "src/main/java/com/example/orders/OrderMapper.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;

interface OrderRepository extends JpaRepository<Order, Long> {
    @Query(
        value = "select * from orders where status = ?1",
        nativeQuery = true)
    List<Order> findNative(String status);
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(mapper_file),
        r#"package com.example.orders;

import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Update;

@Mapper
interface OrderMapper {
    @Update(
        "update orders set status = #{status} where id = #{id}")
    int updateStatus(long id, String status);
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 7),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findNative",
        "com.example.orders.OrderRepository.findNative",
        repo_file,
        (10, 11),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "updateStatus",
        "com.example.orders.OrderMapper.updateStatus",
        mapper_file,
        (10, 11),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderRepository.findNative"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-annotation-query")
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderMapper.updateStatus"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-mybatis-update")
    }));
}

#[test]
fn synthesizes_java_spring_data_repository_table_access_edges() {
    let repo = temp_repo("java-spring-data-table-access");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;

import org.springframework.data.jpa.repository.JpaRepository;

interface OrderRepository extends JpaRepository<Order, Long> {
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;

class OrderService {
    private final OrderRepository orders;

    OrderService(OrderRepository orders) {
        this.orders = orders;
    }

    Order load(Long id) {
        return orders.findById(id).orElseThrow();
    }

    Order create(Order order) {
        return orders.save(order);
    }

    void purge(OrderRepository repository, Long id) {
        repository.deleteById(id);
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 9),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            repo_file,
        ),
        (5, 6),
        json!({
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load",
        "com.example.orders.OrderService.load",
        service_file,
        (10, 12),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "create",
        "com.example.orders.OrderService.create",
        service_file,
        (14, 16),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        5,
        "purge",
        "com.example.orders.OrderService.purge",
        service_file,
        (18, 20),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderService.load"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-spring-data-repository-read")
    }));
    for writer in [
        "cbm:4:com.example.orders.OrderService.create",
        "cbm:5:com.example.orders.OrderService.purge",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.edge_type == "WRITES_TABLE"
                    && edge.source_id == writer
                    && edge.target_id == table_id
                    && edge
                        .evidence
                        .as_ref()
                        .and_then(|v| v.get("strategy"))
                        .and_then(Value::as_str)
                        == Some("java-spring-data-repository-write")
            }),
            "expected {writer} to write orders through Spring Data"
        );
    }
}

#[test]
fn spring_data_table_access_uses_project_sources_and_excludes_tests() {
    let repo = temp_repo("java-spring-data-project-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/orders")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let repo_file = "src/main/java/com/example/orders/OrderRepository.java";
    let service_file = "src/main/java/com/example/orders/OrderService.java";
    let test_file = "src/test/java/com/example/orders/OrderServiceTest.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;
import jakarta.persistence.Entity;
import jakarta.persistence.Table;
@Entity
@Table(name = "orders")
class Order {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(repo_file),
        r#"package com.example.orders;
import org.springframework.data.jpa.repository.JpaRepository;
interface OrderRepository extends JpaRepository<Order, Long> {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(service_file),
        r#"package com.example.orders;
class OrderService {
    private final OrderRepository orders;
    Order load(Long id) {
        return orders.findById(id).orElseThrow();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_file),
        r#"package com.example.orders;
class OrderServiceTest {
    private final OrderRepository orders;
    Order fixture(Long id) {
        return orders.findById(id).orElseThrow();
    }
}
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pom.xml",
            entity_file,
            repo_file,
            service_file,
            test_file,
        ],
    );

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (4, 6),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Interface",
            "OrderRepository",
            "com.example.orders.OrderRepository",
            repo_file,
        ),
        (3, 3),
        json!({"language": "java"}),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "load",
        "com.example.orders.OrderService.load",
        service_file,
        (4, 6),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderService",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "fixture",
        "com.example.orders.OrderServiceTest.fixture",
        test_file,
        (4, 6),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderServiceTest",
        }),
    );

    let edges = synthesize_graph_quiet(&conn, &repo)
        .unwrap()
        .persistence
        .edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderService.load"
    }));
    assert!(!edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:4:com.example.orders.OrderServiceTest.fixture"
    }));
}

#[test]
fn synthesizes_java_mybatis_annotation_table_access_edges() {
    let repo = temp_repo("java-mybatis-annotation-table-access");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let mapper_file = "src/main/java/com/example/orders/OrderMapper.java";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(mapper_file),
        r#"package com.example.orders;

import org.apache.ibatis.annotations.Delete;
import org.apache.ibatis.annotations.Mapper;
import org.apache.ibatis.annotations.Select;
import org.apache.ibatis.annotations.Update;

@Mapper
interface OrderMapper {
    @Select("select * from orders where id = #{id}")
    Order findById(long id);

    @Update("update orders set status = #{status} where id = #{id}")
    int updateStatus(long id, String status);

    @Delete("delete from orders where status = #{status}")
    int deleteByStatus(String status);
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 8),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findById",
        "com.example.orders.OrderMapper.findById",
        mapper_file,
        (10, 11),
        json!({
            "decorators": ["@Select(\"select * from orders where id = #{id}\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "updateStatus",
        "com.example.orders.OrderMapper.updateStatus",
        mapper_file,
        (13, 14),
        json!({
            "decorators": ["@Update(\"update orders set status = #{status} where id = #{id}\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "deleteByStatus",
        "com.example.orders.OrderMapper.deleteByStatus",
        mapper_file,
        (16, 17),
        json!({
            "decorators": ["@Delete(\"delete from orders where status = #{status}\")"],
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderMapper.findById"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-mybatis-select")
    }));
    for writer in [
        "cbm:3:com.example.orders.OrderMapper.updateStatus",
        "cbm:4:com.example.orders.OrderMapper.deleteByStatus",
    ] {
        assert!(
            edges.iter().any(|edge| {
                edge.edge_type == "WRITES_TABLE"
                    && edge.source_id == writer
                    && edge.target_id == table_id
                    && edge
                        .evidence
                        .as_ref()
                        .and_then(|v| v.get("strategy"))
                        .and_then(Value::as_str)
                        .is_some_and(|strategy| strategy.starts_with("java-mybatis-"))
            }),
            "expected {writer} to write orders through MyBatis annotations"
        );
    }
}

#[test]
fn synthesizes_java_mybatis_xml_table_access_edges() {
    let repo = temp_repo("java-mybatis-xml-table-access");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/orders")).unwrap();
    std::fs::create_dir_all(repo.join("src/main/resources/mapper")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/resources/mapper")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let entity_file = "src/main/java/com/example/orders/Order.java";
    let mapper_file = "src/main/java/com/example/orders/OrderMapper.java";
    let xml_file = "src/main/resources/mapper/OrderMapper.xml";
    let test_xml_file = "src/test/resources/mapper/OrderMapper.xml";
    std::fs::write(
        repo.join(entity_file),
        r#"package com.example.orders;

import jakarta.persistence.Entity;
import jakarta.persistence.Table;

@Entity
@Table(name = "orders")
class Order {}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(mapper_file),
        r#"package com.example.orders;

interface OrderMapper {
    Order findById(long id);
    int markShipped(long id);
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(xml_file),
        r#"<mapper namespace="com.example.orders.OrderMapper">
  <select id="findById" resultType="com.example.orders.Order">
    select * from orders where id = #{id}
  </select>
  <update id="markShipped">
    update orders set status = 'SHIPPED' where id = #{id}
  </update>
</mapper>
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_xml_file),
        r#"<mapper namespace="com.example.orders.OrderMapper">
  <delete id="findById">delete from orders</delete>
</mapper>
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &[
            "add",
            "pom.xml",
            entity_file,
            mapper_file,
            xml_file,
            test_xml_file,
        ],
    );

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "Order", "com.example.orders.Order", entity_file),
        (6, 8),
        json!({
            "decorators": ["@Entity", "@Table(name = \"orders\")"],
            "language": "java",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "findById",
        "com.example.orders.OrderMapper.findById",
        mapper_file,
        (4, 4),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderMapper",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "markShipped",
        "com.example.orders.OrderMapper.markShipped",
        mapper_file,
        (5, 5),
        json!({
            "language": "java",
            "parent_class": "com.example.orders.OrderMapper",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let table_id = synth
        .persistence
        .node_recs()
        .into_iter()
        .find(|node| {
            node.label == "Table"
                && node.properties.get("tableName").and_then(Value::as_str) == Some("orders")
        })
        .expect("orders table")
        .id;
    let edges = synth.persistence.edge_recs();
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "READS_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderMapper.findById"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-mybatis-xml-select")
    }));
    assert!(edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:3:com.example.orders.OrderMapper.markShipped"
            && edge.target_id == table_id
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-mybatis-xml-update")
    }));
    assert!(!edges.iter().any(|edge| {
        edge.edge_type == "WRITES_TABLE"
            && edge.source_id == "cbm:2:com.example.orders.OrderMapper.findById"
            && edge
                .evidence
                .as_ref()
                .and_then(|v| v.get("strategy"))
                .and_then(Value::as_str)
                == Some("java-mybatis-xml-delete")
    }));
}
