use super::*;
use serde_json::json;

#[test]
fn synthesizes_picocli_commands_from_source_annotations_without_metadata() {
    let repo = temp_repo("java-picocli-source-annotations");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    let file = "src/main/java/com/example/ops/ReindexCli.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import picocli.CommandLine.Command;

@Command(
    name = "orders-reindex",
    aliases = {"orders-sync"})
class ReindexCli implements Runnable {
    public void run() {
        rebuildOrders();
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "ReindexCli", "com.example.ops.ReindexCli", file),
        (8, 12),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Method", "run", "com.example.ops.ReindexCli.run", file),
        (9, 11),
        json!({
            "language": "java",
            "parent_class": "com.example.ops.ReindexCli",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let command = synth
        .commands
        .iter()
        .find(|command| command.name == "orders-reindex")
        .expect("picocli command from source annotation");
    assert_eq!(command.command_type, "picocli-command");
    assert_eq!(command.handler_id, "cbm:1:com.example.ops.ReindexCli");
    assert_eq!(command.strategy, "java-picocli-command");
    assert!(command.edge_recs().iter().any(|edge| {
        edge.edge_type == "HANDLES_COMMAND"
            && edge.source_id == "cbm:1:com.example.ops.ReindexCli"
            && edge.target_id == command.id
    }));
}
