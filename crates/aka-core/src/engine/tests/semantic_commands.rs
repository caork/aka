use super::*;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn synthesizes_jvm_command_entrypoints() {
    let repo = temp_repo("jvm-commands");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    let file = "src/main/java/com/example/ops/ReindexCommand.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.stereotype.Component;
import picocli.CommandLine.Command;

@Component
class ReindexOrders implements ApplicationRunner {
    @Override
    public void run(ApplicationArguments args) {
        rebuildOrders();
    }
}

class RunnerConfig {
    @Bean
    CommandLineRunner syncRunner() {
        return args -> rebuildOrders();
    }
}

@Command(name = "orders-reindex", aliases = {"orders-sync"})
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
        (
            "Class",
            "ReindexOrders",
            "com.example.ops.ReindexOrders",
            file,
        ),
        (8, 13),
        json!({
            "language": "java",
            "decorators": ["@Component"],
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        ("Method", "run", "com.example.ops.ReindexOrders.run", file),
        (10, 12),
        json!({
            "language": "java",
            "decorators": ["@Override"],
            "parent_class": "com.example.ops.ReindexOrders",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Method",
            "syncRunner",
            "com.example.ops.RunnerConfig.syncRunner",
            file,
        ),
        (17, 20),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.ops.RunnerConfig",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        ("Class", "ReindexCli", "com.example.ops.ReindexCli", file),
        (23, 28),
        json!({
            "language": "java",
            "decorators": ["@Command(name = \"orders-reindex\", aliases = {\"orders-sync\"})"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let spring = synth
        .commands
        .iter()
        .find(|command| command.command_type == "spring-runner")
        .expect("spring runner command");
    assert_eq!(spring.handler_id, "cbm:1:com.example.ops.ReindexOrders");
    assert_eq!(spring.strategy, "java-spring-runner-source-declaration");
    let picocli = synth
        .commands
        .iter()
        .find(|command| command.command_type == "picocli-command")
        .expect("picocli command");
    assert_eq!(picocli.name, "orders-reindex");
    assert_eq!(picocli.handler_id, "cbm:4:com.example.ops.ReindexCli");
    assert!(synth
        .commands
        .iter()
        .any(|command| command.handler_id == "cbm:3:com.example.ops.RunnerConfig.syncRunner"));

    let edge_types: Vec<_> = synth
        .commands
        .iter()
        .flat_map(SynthCommand::edge_recs)
        .map(|edge| edge.edge_type)
        .collect();
    assert!(edge_types.contains(&"HANDLES_COMMAND".to_string()));
}

#[test]
fn spring_runner_detection_uses_source_facts_not_class_names() {
    let repo = temp_repo("spring-runner-source-facts");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    let file = "src/main/java/com/example/ops/StartupMaintenance.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.context.annotation.Bean;

class StartupMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        warmCache();
    }
}

class MaintenanceConfiguration {
    @Bean
    public org.springframework.boot.CommandLineRunner repairOrders() {
        return args -> {};
    }
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            file,
        ),
        (7, 11),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Method",
            "repairOrders",
            "com.example.ops.MaintenanceConfiguration.repairOrders",
            file,
        ),
        (15, 17),
        json!({
            "language": "java",
            "decorators": ["@Bean"],
            "parent_class": "com.example.ops.MaintenanceConfiguration",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: BTreeSet<_> = synth
        .commands
        .iter()
        .filter(|command| command.command_type == "spring-runner")
        .map(|command| command.handler_id.as_str())
        .collect();
    assert!(handlers.contains("cbm:1:com.example.ops.StartupMaintenance"));
    assert!(handlers.contains("cbm:2:com.example.ops.MaintenanceConfiguration.repairOrders"));
}

#[test]
fn spring_runner_synthesis_scans_git_sources_when_cbm_has_no_java_nodes() {
    let repo = temp_repo("spring-runner-no-cbm-java-nodes");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/main/java/com/example/ops/StartupMaintenance.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.context.annotation.Bean;

class StartupMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        warmCache();
    }
}

class MaintenanceConfiguration {
    @Bean
    public org.springframework.boot.CommandLineRunner repairOrders() {
        return args -> {};
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let commands: BTreeMap<_, _> = synth
        .commands
        .iter()
        .filter(|command| command.command_type == "spring-runner")
        .map(|command| (command.handler_name.as_str(), command))
        .collect();
    let class_command = commands
        .get("StartupMaintenance")
        .expect("class source-scanned runner");
    assert!(class_command.handler_id.starts_with("source:java:"));
    assert_eq!(
        class_command.strategy,
        "java-spring-runner-source-declaration"
    );
    let bean_command = commands
        .get("repairOrders")
        .expect("bean source-scanned runner");
    assert!(bean_command.handler_id.starts_with("source:java:"));
    assert_eq!(
        bean_command.strategy,
        "java-spring-runner-bean-source-declaration"
    );

    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Class" && symbol.node().qn == "com.example.ops.StartupMaintenance"
    }));
    assert!(synth.source_symbols.iter().any(|symbol| {
        symbol.node().label == "Method"
            && symbol.node().qn == "com.example.ops.MaintenanceConfiguration.repairOrders"
    }));
}

#[test]
fn spring_runner_source_scan_excludes_test_roots_without_cbm_nodes() {
    let repo = temp_repo("spring-runner-no-cbm-test-roots");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/test/java/com/example/ops/TestMaintenance.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.CommandLineRunner;

class TestMaintenance implements CommandLineRunner {
    public void run(String... args) {
        resetFixtures();
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.commands.is_empty());
}

#[test]
fn ignores_test_source_command_entrypoints() {
    let repo = temp_repo("test-source-commands");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/test/java/com/example/ops/TestCommand.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

import org.springframework.boot.CommandLineRunner;

class TestCommand implements CommandLineRunner {
    public void run(String... args) {
        rebuildFixtures();
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        ("Class", "TestCommand", "com.example.ops.TestCommand", file),
        (5, 9),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(synth.commands.is_empty());
}

#[test]
fn command_synthesis_uses_project_sources_not_runner_names() {
    let repo = temp_repo("git-project-command-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let tracked_file = "src/main/java/com/example/ops/StartupMaintenance.java";
    let source_fact_file = "src/main/java/com/example/ops/BootMaintenance.java";
    let untracked_file = "src/main/java/com/example/ops/UntrackedMaintenance.java";
    let test_file = "src/test/java/com/example/ops/TestMaintenance.java";
    std::fs::write(
        repo.join(tracked_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;

class StartupMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        warmCache();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(source_fact_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;

class BootMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        hydrateIndexes();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(untracked_file),
        r#"package com.example.ops;

import org.springframework.boot.CommandLineRunner;

class UntrackedMaintenance implements CommandLineRunner {
    public void run(String... args) {
        repairOrders();
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_file),
        r#"package com.example.ops;

import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;

class TestMaintenance implements ApplicationRunner {
    public void run(ApplicationArguments args) {
        resetFixtures();
    }
}
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &["add", "pom.xml", tracked_file, source_fact_file, test_file],
    );

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            tracked_file,
        ),
        (6, 10),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "BootMaintenance",
            "com.example.ops.BootMaintenance",
            source_fact_file,
        ),
        (6, 10),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        3,
        (
            "Class",
            "UntrackedMaintenance",
            "com.example.ops.UntrackedMaintenance",
            untracked_file,
        ),
        (5, 9),
        json!({
            "language": "java",
        }),
    );
    insert_node_props_at(
        &conn,
        4,
        (
            "Class",
            "TestMaintenance",
            "com.example.ops.TestMaintenance",
            test_file,
        ),
        (6, 10),
        json!({
            "language": "java",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: Vec<_> = synth
        .commands
        .iter()
        .map(|command| command.handler_name.as_str())
        .collect();
    assert!(handlers.contains(&"StartupMaintenance"));
    assert!(
        handlers.contains(&"BootMaintenance"),
        "Spring runner detection should use implements source facts, not class-name suffixes"
    );
    assert!(handlers.contains(&"UntrackedMaintenance"));
    assert!(!handlers.contains(&"TestMaintenance"));
}

#[test]
fn spring_runner_detection_ignores_runner_names_without_source_facts() {
    let repo = temp_repo("spring-runner-name-only");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let file = "src/main/java/com/example/ops/InventoryRunner.java";
    std::fs::write(
        repo.join(file),
        r#"package com.example.ops;

class InventoryRunner {
    void execute() {
        rebuildInventory();
    }
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "InventoryRunner",
            "com.example.ops.InventoryRunner",
            file,
        ),
        (3, 7),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    assert!(
        synth.commands.is_empty(),
        "Spring runner commands require implements/@Bean source facts, not Runner name suffixes"
    );
}

#[test]
fn command_synthesis_excludes_gitignored_source_files() {
    let repo = temp_repo("gitignored-command-sources");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::write(repo.join(".gitignore"), "scratch/\n").unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let tracked_file = "src/main/java/com/example/ops/StartupMaintenance.java";
    let ignored_file = "scratch/IgnoredMaintenance.java";
    std::fs::create_dir_all(repo.join("scratch")).unwrap();
    std::fs::write(
        repo.join(tracked_file),
        r#"package com.example.ops;
import org.springframework.boot.ApplicationRunner;
class StartupMaintenance implements ApplicationRunner {
    public void run(org.springframework.boot.ApplicationArguments args) {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(ignored_file),
        r#"package com.example.ops;
import org.springframework.boot.CommandLineRunner;
class IgnoredMaintenance implements CommandLineRunner {
    public void run(String... args) {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", ".gitignore", "pom.xml", tracked_file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            tracked_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "IgnoredMaintenance",
            "com.example.ops.IgnoredMaintenance",
            ignored_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.handler_name.as_str())
        .collect();
    assert!(handlers.contains("StartupMaintenance"));
    assert!(!handlers.contains("IgnoredMaintenance"));
}

#[test]
fn command_synthesis_falls_back_to_repo_files_without_git() {
    let repo = temp_repo("filesystem-project-command-sources");
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::create_dir_all(repo.join("src/test/java/com/example/ops")).unwrap();
    std::fs::write(repo.join("pom.xml"), "<project></project>").unwrap();
    let main_file = "src/main/java/com/example/ops/StartupMaintenance.java";
    let test_file = "src/test/java/com/example/ops/TestMaintenance.java";
    std::fs::write(
        repo.join(main_file),
        r#"package com.example.ops;
import org.springframework.boot.ApplicationRunner;
class StartupMaintenance implements ApplicationRunner {
    public void run(org.springframework.boot.ApplicationArguments args) {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(test_file),
        r#"package com.example.ops;
import org.springframework.boot.CommandLineRunner;
class TestMaintenance implements CommandLineRunner {
    public void run(String... args) {}
}
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            main_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "TestMaintenance",
            "com.example.ops.TestMaintenance",
            test_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.handler_name.as_str())
        .collect();
    assert!(handlers.contains("StartupMaintenance"));
    assert!(!handlers.contains("TestMaintenance"));
}

#[test]
fn command_synthesis_excludes_build_configured_test_roots() {
    let repo = temp_repo("configured-test-source-commands");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("src/main/java/com/example/ops")).unwrap();
    std::fs::create_dir_all(repo.join("src/fixture/java/com/example/ops")).unwrap();
    std::fs::write(
        repo.join("pom.xml"),
        r#"<project>
  <build>
    <testSourceDirectory>src/fixture/java</testSourceDirectory>
  </build>
</project>
"#,
    )
    .unwrap();
    let main_file = "src/main/java/com/example/ops/StartupMaintenance.java";
    let fixture_file = "src/fixture/java/com/example/ops/FixtureMaintenance.java";
    std::fs::write(
        repo.join(main_file),
        r#"package com.example.ops;
import org.springframework.boot.ApplicationRunner;
class StartupMaintenance implements ApplicationRunner {
    public void run(org.springframework.boot.ApplicationArguments args) {}
}
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join(fixture_file),
        r#"package com.example.ops;
import org.springframework.boot.CommandLineRunner;
class FixtureMaintenance implements CommandLineRunner {
    public void run(String... args) {}
}
"#,
    )
    .unwrap();
    run_git(&repo, &["add", "pom.xml", main_file, fixture_file]);

    let conn = test_conn();
    insert_node_props_at(
        &conn,
        1,
        (
            "Class",
            "StartupMaintenance",
            "com.example.ops.StartupMaintenance",
            main_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );
    insert_node_props_at(
        &conn,
        2,
        (
            "Class",
            "FixtureMaintenance",
            "com.example.ops.FixtureMaintenance",
            fixture_file,
        ),
        (3, 5),
        json!({"language": "java"}),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let handlers: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.handler_name.as_str())
        .collect();
    assert!(handlers.contains("StartupMaintenance"));
    assert!(!handlers.contains("FixtureMaintenance"));
}

#[test]
fn synthesizes_python_command_entrypoints() {
    let repo = temp_repo("python-commands");
    std::fs::create_dir_all(repo.join("orders/management/commands")).unwrap();
    std::fs::write(
        repo.join("orders/management/commands/reindex_orders.py"),
        r#"from django.core.management.base import BaseCommand

class Command(BaseCommand):
    def handle(self, *args, **options):
        rebuild_orders()
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("cli.py"),
        r#"import argparse
import click
import typer

app = typer.Typer()

@click.command(name="sync-orders")
def sync_orders():
    pass

@app.command("ship-orders")
def ship_orders():
    pass

def main():
    parser = argparse.ArgumentParser(prog="orders-admin")
    sub = parser.add_subparsers()
    sub.add_parser("reindex")
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "handle",
        "orders.management.commands.reindex_orders.Command.handle",
        "orders/management/commands/reindex_orders.py",
        (4, 5),
        json!({
            "language": "python",
            "parent_class": "Command",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "sync_orders",
        "cli.sync_orders",
        "cli.py",
        (8, 9),
        json!({
            "language": "python",
            "decorators": ["@click.command(name=\"sync-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "ship_orders",
        "cli.ship_orders",
        "cli.py",
        (12, 13),
        json!({
            "language": "python",
            "decorators": ["@app.command(\"ship-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        4,
        "main",
        "cli.main",
        "cli.py",
        (15, 18),
        json!({
            "language": "python",
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let names: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert!(names.contains("reindex_orders"));
    assert!(names.contains("sync-orders"));
    assert!(names.contains("ship-orders"));
    assert!(names.contains("orders-admin"));
    assert!(names.contains("reindex"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "django-management-command"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "click-command"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "typer-command"));
    assert!(synth
        .commands
        .iter()
        .any(|command| command.command_type == "argparse-command"));
}

#[test]
fn python_command_entrypoints_seed_processes() {
    let repo = temp_repo("python-command-process-entry");
    std::fs::create_dir_all(repo.join("orders/management/commands")).unwrap();
    let file = "orders/management/commands/reindex_orders.py";
    std::fs::write(
        repo.join(file),
        r#"from django.core.management.base import BaseCommand

class Command(BaseCommand):
    def handle(self, *args, **options):
        load_orders()

def load_orders():
    persist_orders()

def persist_orders():
    pass
"#,
    )
    .unwrap();

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "handle",
        "orders.management.commands.reindex_orders.Command.handle",
        file,
        (4, 5),
        json!({
            "language": "python",
            "parent_class": "Command",
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_orders",
        "orders.management.commands.reindex_orders.load_orders",
        file,
        (7, 8),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "persist_orders",
        "orders.management.commands.reindex_orders.persist_orders",
        file,
        (10, 11),
        json!({
            "language": "python",
        }),
    );
    insert_edge(&conn, 1, 1, 2, "CALLS");
    insert_edge(&conn, 2, 2, 3, "CALLS");

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let process = synth
        .processes
        .iter()
        .find(|process| process.name == "handle → persist_orders")
        .expect("Django management command handler should seed process entry");
    assert_eq!(
        process.node_rec().properties["entryReason"],
        "python-django-management-command"
    );
    let command = synth
        .commands
        .iter()
        .find(|command| command.name == "reindex_orders")
        .expect("Django management command");
    assert_eq!(command.command_type, "django-management-command");
    assert!(
        command.process_ids.contains(&process.id),
        "command should link to seeded process"
    );
}

#[test]
fn command_synthesis_excludes_python_configured_test_roots() {
    let repo = temp_repo("python-configured-test-commands");
    run_git(&repo, &["init"]);
    std::fs::create_dir_all(repo.join("ops")).unwrap();
    std::fs::create_dir_all(repo.join("tests")).unwrap();
    std::fs::write(
        repo.join("pyproject.toml"),
        r#"[tool.pytest.ini_options]
testpaths = ["tests"]
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("ops/cli.py"),
        r#"import click

@click.command(name="sync-orders")
def sync_orders():
    pass
"#,
    )
    .unwrap();
    std::fs::write(
        repo.join("tests/test_cli.py"),
        r#"import click

@click.command(name="fixture-sync")
def fixture_sync():
    pass
"#,
    )
    .unwrap();
    run_git(
        &repo,
        &["add", "pyproject.toml", "ops/cli.py", "tests/test_cli.py"],
    );

    let conn = test_conn();
    insert_function_node_props_at(
        &conn,
        1,
        "sync_orders",
        "ops.cli.sync_orders",
        "ops/cli.py",
        (3, 4),
        json!({
            "language": "python",
            "decorators": ["@click.command(name=\"sync-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "fixture_sync",
        "tests.test_cli.fixture_sync",
        "tests/test_cli.py",
        (3, 4),
        json!({
            "language": "python",
            "decorators": ["@click.command(name=\"fixture-sync\")"],
        }),
    );

    let synth = synthesize_graph_quiet(&conn, &repo).unwrap();
    let names: BTreeSet<_> = synth
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert!(names.contains("sync-orders"));
    assert!(!names.contains("fixture-sync"));
}
