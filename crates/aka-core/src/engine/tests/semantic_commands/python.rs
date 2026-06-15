use super::*;
use serde_json::json;
use std::collections::BTreeSet;

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
fn flask_cli_commands_seed_processes() {
    let repo = temp_repo("flask-cli-command-process-entry");
    let file = "orders/app.py";
    std::fs::create_dir_all(repo.join("orders")).unwrap();
    std::fs::write(
        repo.join(file),
        r#"from flask import Flask

app = Flask(__name__)

@app.cli.command("sync-orders")
def sync_orders():
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
        "sync_orders",
        "orders.app.sync_orders",
        file,
        (6, 7),
        json!({
            "language": "python",
            "decorators": ["@app.cli.command(\"sync-orders\")"],
        }),
    );
    insert_function_node_props_at(
        &conn,
        2,
        "load_orders",
        "orders.app.load_orders",
        file,
        (9, 10),
        json!({
            "language": "python",
        }),
    );
    insert_function_node_props_at(
        &conn,
        3,
        "persist_orders",
        "orders.app.persist_orders",
        file,
        (12, 13),
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
        .find(|process| process.name == "sync_orders → persist_orders")
        .expect("Flask CLI command should seed process entry");
    assert_eq!(
        process.node_rec().properties["entryReason"],
        "python-flask-command"
    );
    let command = synth
        .commands
        .iter()
        .find(|command| command.name == "sync-orders")
        .expect("Flask CLI command");
    assert_eq!(command.command_type, "flask-command");
    assert!(
        command.process_ids.contains(&process.id),
        "Flask command should link to seeded process"
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
