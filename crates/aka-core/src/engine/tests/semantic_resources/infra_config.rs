use super::super::*;
use serde_json::json;

#[test]
fn synthesizes_spring_infra_config_resources() {
    let repo = temp_repo("spring-infra-config-resources");
    std::fs::create_dir_all(repo.join("src/main/resources")).unwrap();
    let file = "src/main/resources/application.yml";
    std::fs::write(
        repo.join(file),
        r#"spring:
  datasource:
    url: jdbc:postgresql://db.internal:5432/orders?ssl=true
  data:
    redis:
      host: redis.internal
    mongodb:
      uri: mongodb+srv://mongo.example.com/orders
  kafka:
    bootstrap-servers: kafka-a:9092,kafka-b:9092
  rabbitmq:
    addresses: amqp://rabbit.internal:5672
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "application.yml", file, file),
        (1, 12),
        json!({
            "language": "yaml",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_resource_edge(
        &synth,
        "database:postgresql://db.internal:5432/orders",
        "database",
        &config_id("spring.datasource.url"),
        "database-config",
    );
    assert_resource_edge(
        &synth,
        "redis://redis.internal",
        "redis",
        &config_id("spring.data.redis.host"),
        "redis-config",
    );
    assert_resource_edge(
        &synth,
        "mongodb://mongo.example.com/orders",
        "mongodb",
        &config_id("spring.data.mongodb.uri"),
        "mongodb-config",
    );
    assert_resource_edge(
        &synth,
        "kafka://kafka-a:9092,kafka-b:9092",
        "kafka",
        &config_id("spring.kafka.bootstrap.servers"),
        "kafka-config",
    );
    assert_resource_edge(
        &synth,
        "rabbitmq://rabbit.internal:5672",
        "rabbitmq",
        &config_id("spring.rabbitmq.addresses"),
        "rabbitmq-config",
    );
}

#[test]
fn synthesizes_env_and_python_settings_infra_config_resources() {
    let repo = temp_repo("python-infra-config-resources");
    std::fs::write(
        repo.join(".env"),
        r#"DATABASE_URL=postgres://db.example.com:5432/billing
REDIS_URL=rediss://redis.example.com:6380/0
KAFKA_BOOTSTRAP_SERVERS=kafka1:9092,kafka2:9092
RABBITMQ_URL=amqps://rabbit.example.com/vhost
MONGODB_URI=mongodb://mongo.example.com:27017/app
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("settings.py"),
        r#"SQLALCHEMY_DATABASE_URI = "mysql://mysql.example.com/orders"
CELERY_BROKER_URL = "redis://redis-celery:6379/1"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", ".env", ".env", ".env"),
        (1, 5),
        json!({
            "language": "dotenv",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Config", "settings.py", "settings.py", "settings.py"),
        (1, 2),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert_resource_edge(
        &synth,
        "database:postgresql://db.example.com:5432/billing",
        "database",
        &config_id("database.url"),
        "database-config",
    );
    assert_resource_edge(
        &synth,
        "redis://redis.example.com:6380/0",
        "redis",
        &config_id("redis.url"),
        "redis-config",
    );
    assert_resource_edge(
        &synth,
        "kafka://kafka1:9092,kafka2:9092",
        "kafka",
        &config_id("kafka.bootstrap.servers"),
        "kafka-config",
    );
    assert_resource_edge(
        &synth,
        "rabbitmq://rabbit.example.com/vhost",
        "rabbitmq",
        &config_id("rabbitmq.url"),
        "rabbitmq-config",
    );
    assert_resource_edge(
        &synth,
        "mongodb://mongo.example.com:27017/app",
        "mongodb",
        &config_id("mongodb.uri"),
        "mongodb-config",
    );
    assert_resource_edge(
        &synth,
        "database:mysql://mysql.example.com/orders",
        "database",
        &config_id("sqlalchemy.database.uri"),
        "database-config",
    );
    assert_resource_edge(
        &synth,
        "redis://redis-celery:6379/1",
        "redis",
        &config_id("celery.broker.url"),
        "redis-config",
    );
}

#[test]
fn ignores_plain_config_text_without_infra_keys() {
    let repo = temp_repo("infra-config-negative");
    std::fs::write(
        repo.join("config.py"),
        r#"CACHE_DESCRIPTION = "redis is mentioned in docs only"
BROKER_NOTE = "kafka-a:9092 is an example endpoint"
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Config", "config.py", "config.py", "config.py"),
        (1, 2),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth
        .resources
        .iter()
        .all(|resource| resource.resource_type != "redis"
            && resource.resource_type != "kafka"
            && resource.resource_type != "database"));
}

fn assert_resource_edge(
    synth: &SynthGraph,
    url: &str,
    resource_type: &str,
    source_id: &str,
    strategy: &str,
) {
    let resource = synth
        .resources
        .iter()
        .find(|resource| resource.url == url)
        .unwrap_or_else(|| panic!("expected resource {url}"));
    assert_eq!(resource.resource_type, resource_type);
    assert!(resource.edge_recs().iter().any(|edge| {
        edge.source_id == source_id
            && edge.edge_type == "ACCESSES_RESOURCE"
            && edge.evidence.as_ref().unwrap()["strategy"] == strategy
    }));
}

fn config_id(key: &str) -> String {
    format!("config:heuristic:{:016x}", stable_hash(key))
}
