#!/usr/bin/env node
// Convert tree-sitter-stack-graphs-python definition queries into an aka-facts bundle.
//
// This adapter is intentionally outside the AKA runtime. It runs the external
// open-source tree-sitter stack-graphs Python CLI, asks it to index/query a
// repository, and writes a facts JSON bundle that AKA can validate and import
// through ossAnalyzerFactsPath.

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { TextDecoder } from "node:util";

const DEFAULT_TOOL = "tree-sitter-stack-graphs-python";
const DEFAULT_TIMEOUT_SECS = 900;
const DEFAULT_MAX_FILE_SECS = 20;
const DEFAULT_INDEX_BATCH_SIZE = 200;
const DEFAULT_INDEX_BATCH_TIMEOUT_SECS = 300;
const DEFAULT_QUERY_BATCH_SIZE = 32;
const DEFAULT_QUERY_TIMEOUT_SECS = 5;
const DEFAULT_MAX_QUERY_TIMEOUTS_PER_FILE = 2;
const DEFAULT_MAX_QUERY_POSITIONS = 10000;
const DEFAULT_LOG_EVERY = 100;
const UTF8_DECODER = new TextDecoder("utf-8", { fatal: true });

const SKIP_DIRS = new Set([
  ".git",
  ".hg",
  ".svn",
  "__pycache__",
  ".mypy_cache",
  ".pytest_cache",
  ".ruff_cache",
  ".tox",
  ".venv",
  "venv",
  "node_modules",
  "target",
  "build",
  "dist",
]);

const PY_EXTENSIONS = new Set([".py", ".pyi"]);
const IDENTIFIER_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

function usage() {
  console.error(`Usage:
  scripts/oss-analyzer-stack-graphs-python.mjs --repo PATH --out PATH [options]

Required:
  --repo PATH                 Repository root.
  --out PATH                  Output aka-facts JSON bundle.

Options:
  --tool CMD                  stack-graphs Python CLI command. Default: ${DEFAULT_TOOL}
  --database PATH             stack-graphs database path. Default: OUT_DIR/stack-graphs-python.db
  --tool-version VERSION      Override analyzer toolVersion.
  --timeout-secs N            Whole adapter deadline. Default: ${DEFAULT_TIMEOUT_SECS}
  --max-file-secs N           stack-graphs per-file indexing budget. Default: ${DEFAULT_MAX_FILE_SECS}
  --max-files N               Optional file cap for smoke/debug runs.
  --max-query-positions N     Max stack-graphs definition queries. Default: ${DEFAULT_MAX_QUERY_POSITIONS}
  --index-batch-size N        Files per stack-graphs index invocation. Default: ${DEFAULT_INDEX_BATCH_SIZE}
  --index-batch-timeout-secs N  Per index invocation deadline. Default: ${DEFAULT_INDEX_BATCH_TIMEOUT_SECS}
  --query-batch-size N        Positions per definition query invocation. Default: ${DEFAULT_QUERY_BATCH_SIZE}
  --query-timeout-secs N       Per definition query invocation deadline. Default: ${DEFAULT_QUERY_TIMEOUT_SECS}
  --max-query-timeouts-per-file N  Skip a file after this many definition query timeouts. Default: ${DEFAULT_MAX_QUERY_TIMEOUTS_PER_FILE}
  --exclude-dir PATH          Additional repo-relative directory to skip. Repeatable.
  --log-every N               Progress log interval. Default: ${DEFAULT_LOG_EVERY}
  --no-chunks                 Do not emit evidence chunks.
  --help                      Show this message.
`);
}

function parseArgs(argv) {
  const args = {
    tool: DEFAULT_TOOL,
    timeoutSecs: DEFAULT_TIMEOUT_SECS,
    maxFileSecs: DEFAULT_MAX_FILE_SECS,
    indexBatchSize: DEFAULT_INDEX_BATCH_SIZE,
    indexBatchTimeoutSecs: DEFAULT_INDEX_BATCH_TIMEOUT_SECS,
    queryBatchSize: DEFAULT_QUERY_BATCH_SIZE,
    queryTimeoutSecs: DEFAULT_QUERY_TIMEOUT_SECS,
    maxQueryTimeoutsPerFile: DEFAULT_MAX_QUERY_TIMEOUTS_PER_FILE,
    maxQueryPositions: DEFAULT_MAX_QUERY_POSITIONS,
    maxFiles: null,
    excludeDirs: [],
    logEvery: DEFAULT_LOG_EVERY,
    noChunks: false,
    toolVersion: null,
    database: null,
  };
  for (let i = 2; i < argv.length; i += 1) {
    const arg = argv[i];
    const next = () => {
      const value = argv[++i];
      if (!value) {
        throw new Error(`${arg} requires a value`);
      }
      return value;
    };
    switch (arg) {
      case "--repo":
        args.repo = next();
        break;
      case "--out":
        args.out = next();
        break;
      case "--tool":
        args.tool = next();
        break;
      case "--database":
        args.database = next();
        break;
      case "--tool-version":
        args.toolVersion = next();
        break;
      case "--timeout-secs":
        args.timeoutSecs = Number(next());
        break;
      case "--max-file-secs":
        args.maxFileSecs = Number(next());
        break;
      case "--max-files":
        args.maxFiles = Number(next());
        break;
      case "--max-query-positions":
        args.maxQueryPositions = Number(next());
        break;
      case "--index-batch-size":
        args.indexBatchSize = Number(next());
        break;
      case "--index-batch-timeout-secs":
        args.indexBatchTimeoutSecs = Number(next());
        break;
      case "--query-batch-size":
        args.queryBatchSize = Number(next());
        break;
      case "--query-timeout-secs":
        args.queryTimeoutSecs = Number(next());
        break;
      case "--max-query-timeouts-per-file":
        args.maxQueryTimeoutsPerFile = Number(next());
        break;
      case "--exclude-dir":
        args.excludeDirs.push(next());
        break;
      case "--log-every":
        args.logEvery = Number(next());
        break;
      case "--no-chunks":
        args.noChunks = true;
        break;
      case "--help":
      case "-h":
        usage();
        process.exit(0);
        break;
      default:
        throw new Error(`unknown argument: ${arg}`);
    }
  }
  if (!args.repo || !args.out) {
    usage();
    process.exit(2);
  }
  for (const [name, value] of [
    ["--timeout-secs", args.timeoutSecs],
    ["--max-file-secs", args.maxFileSecs],
    ["--index-batch-size", args.indexBatchSize],
    ["--index-batch-timeout-secs", args.indexBatchTimeoutSecs],
    ["--query-batch-size", args.queryBatchSize],
    ["--query-timeout-secs", args.queryTimeoutSecs],
    ["--max-query-timeouts-per-file", args.maxQueryTimeoutsPerFile],
    ["--max-query-positions", args.maxQueryPositions],
    ["--log-every", args.logEvery],
  ]) {
    if (!Number.isFinite(value) || value <= 0) {
      throw new Error(`${name} must be a positive number`);
    }
  }
  if (args.maxFiles !== null && (!Number.isFinite(args.maxFiles) || args.maxFiles <= 0)) {
    throw new Error("--max-files must be a positive number");
  }
  args.indexBatchSize = Math.floor(args.indexBatchSize);
  args.queryBatchSize = Math.floor(args.queryBatchSize);
  args.maxQueryTimeoutsPerFile = Math.floor(args.maxQueryTimeoutsPerFile);
  args.maxQueryPositions = Math.floor(args.maxQueryPositions);
  args.logEvery = Math.floor(args.logEvery);
  return args;
}

function normalizeRelativeDir(value) {
  return value
    .replaceAll("\\", "/")
    .replace(/^\/+/, "")
    .replace(/\/+$/, "");
}

async function discoverPythonFiles(root, maxFiles, excludeDirs) {
  const files = [];
  const skipped = [];
  const excluded = new Set(excludeDirs.map(normalizeRelativeDir).filter(Boolean));
  async function walk(dir) {
    if (maxFiles !== null && files.length >= maxFiles) {
      return;
    }
    const entries = await readdir(dir, { withFileTypes: true });
    entries.sort((a, b) => a.name.localeCompare(b.name));
    for (const entry of entries) {
      if (maxFiles !== null && files.length >= maxFiles) {
        return;
      }
      if (entry.isDirectory()) {
        if (!SKIP_DIRS.has(entry.name)) {
          const child = path.join(dir, entry.name);
          const rel = relativePath(root, child);
          if (!excluded.has(rel)) {
            await walk(child);
          }
        }
      } else if (entry.isFile() && PY_EXTENSIONS.has(path.extname(entry.name))) {
        const file = path.join(dir, entry.name);
        const readable = await canStackGraphsReadFile(file);
        if (readable.ok) {
          files.push(file);
        } else {
          skipped.push({
            file,
            reason: readable.reason,
          });
        }
      }
    }
  }
  await walk(root);
  return { files, skipped };
}

async function canStackGraphsReadFile(file) {
  try {
    const bytes = await readFile(file);
    UTF8_DECODER.decode(bytes);
    return { ok: true };
  } catch (error) {
    return {
      ok: false,
      reason: `not valid UTF-8 for tree-sitter stack-graphs: ${error.message}`,
    };
  }
}

function relativePath(root, file) {
  return path.relative(root, file).split(path.sep).join("/");
}

function shellQuote(value) {
  return `'${String(value).replaceAll("'", `'\\''`)}'`;
}

function runTool(toolCommand, args, options) {
  const command = `${toolCommand} ${args.map(shellQuote).join(" ")}`;
  const child = spawn(
    process.platform === "win32" ? "cmd.exe" : "bash",
    process.platform === "win32" ? ["/d", "/s", "/c", command] : ["-lc", command],
    {
      cwd: options.cwd,
      detached: process.platform !== "win32",
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => {
    stdout += chunk.toString("utf8");
  });
  child.stderr.on("data", (chunk) => {
    const text = chunk.toString("utf8");
    stderr += text;
    if (options.forwardStderr) {
      for (const line of text.split(/\r?\n/)) {
        if (line.trim()) {
          console.error(`[stack-graphs] ${line}`);
        }
      }
    }
  });
  return new Promise((resolve, reject) => {
    let timedOut = false;
    let forceKillTimer = null;
    const timer = setTimeout(() => {
      timedOut = true;
      terminateChild(child, "SIGTERM");
      forceKillTimer = setTimeout(() => terminateChild(child, "SIGKILL"), 2_000);
      forceKillTimer.unref?.();
    }, Math.max(1, options.timeoutMs));
    child.on("exit", (code, signal) => {
      clearTimeout(timer);
      if (forceKillTimer) {
        clearTimeout(forceKillTimer);
      }
      if (timedOut) {
        reject(new Error(`timed out running ${args[0]} after ${Math.ceil(options.timeoutMs / 1000)}s`));
        return;
      }
      if (code === 0) {
        resolve({ stdout, stderr });
      } else {
        reject(new Error(`stack-graphs ${args[0]} exited code=${code} signal=${signal}: ${stderr.trim()}`));
      }
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      if (forceKillTimer) {
        clearTimeout(forceKillTimer);
      }
      reject(error);
    });
  });
}

function terminateChild(child, signal) {
  try {
    if (process.platform === "win32") {
      spawn("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
      return;
    }
    process.kill(-child.pid, signal);
  } catch {
    try {
      child.kill(signal);
    } catch {
      // Process is already gone.
    }
  }
}

function remainingMs(deadlineAt) {
  return Math.max(1, deadlineAt - Date.now());
}

async function toolVersion(tool, deadlineAt) {
  try {
    const { stdout, stderr } = await runTool(tool, ["--version"], {
      timeoutMs: Math.min(10_000, remainingMs(deadlineAt)),
      forwardStderr: false,
    });
    return (stdout || stderr).trim() || "unknown";
  } catch {
    return "unknown";
  }
}

function batches(items, size) {
  const result = [];
  for (let i = 0; i < items.length; i += size) {
    result.push(items.slice(i, i + size));
  }
  return result;
}

async function indexFiles(tool, database, files, args, deadlineAt) {
  await rm(database, { force: true, recursive: true });
  const skipped = [];
  const indexed = [];
  const fileBatches = batches(files, args.indexBatchSize);
  for (let index = 0; index < fileBatches.length; index += 1) {
    const batch = fileBatches[index];
    console.error(`[aka-stack-graphs-python] index batch ${index + 1}/${fileBatches.length} files=${batch.length}`);
    const result = await indexFileBatch(tool, database, batch, args, deadlineAt);
    indexed.push(...result.indexed);
    skipped.push(...result.skipped);
  }
  return { indexed, skipped };
}

async function indexFileBatch(tool, database, files, args, deadlineAt) {
  try {
    await runTool(tool, [
      "index",
      "-D",
      database,
      "--force",
      "--hide-error-details",
      "--max-file-time",
      String(args.maxFileSecs),
      ...files,
    ], {
      timeoutMs: Math.min(args.indexBatchTimeoutSecs * 1000, remainingMs(deadlineAt)),
      forwardStderr: true,
    });
    return { indexed: files, skipped: [] };
  } catch (error) {
    if (files.length === 1) {
      console.error(`[aka-stack-graphs-python] skip index ${files[0]}: ${error.message}`);
      return {
        indexed: [],
        skipped: [{
          file: files[0],
          reason: error.message,
        }],
      };
    }
    const split = Math.max(1, Math.floor(files.length / 2));
    const left = await indexFileBatch(tool, database, files.slice(0, split), args, deadlineAt);
    const right = await indexFileBatch(tool, database, files.slice(split), args, deadlineAt);
    return {
      indexed: [...left.indexed, ...right.indexed],
      skipped: [...left.skipped, ...right.skipped],
    };
  }
}

async function collectQueryPositions(tool, files, args, deadlineAt) {
  const byKey = new Map();
  for (let i = 0; i < files.length; i += 1) {
    if (byKey.size >= args.maxQueryPositions) {
      break;
    }
    if (i % args.logEvery === 0) {
      console.error(`[aka-stack-graphs-python] match ${i + 1}/${files.length} positions=${byKey.size}`);
    }
    let stdout;
    try {
      ({ stdout } = await runTool(tool, ["match", files[i]], {
        timeoutMs: Math.min(30_000, remainingMs(deadlineAt)),
        forwardStderr: false,
      }));
    } catch (error) {
      console.error(`[aka-stack-graphs-python] skip match ${files[i]}: ${error.message}`);
      continue;
    }
    for (const position of parseMatchPositions(stdout)) {
      if (!IDENTIFIER_RE.test(position.name)) {
        continue;
      }
      const key = `${position.file}:${position.line}:${position.column}:${position.name}`;
      if (!byKey.has(key)) {
        byKey.set(key, position);
        if (byKey.size >= args.maxQueryPositions) {
          break;
        }
      }
    }
  }
  return [...byKey.values()];
}

function parseMatchPositions(output) {
  const positions = [];
  const regex = /@(?:name|fn|first|last|node)\s+=\s+\([^[]+\[(\d+):(\d+)\s+-\s+(\d+):(\d+)\]\), text: "([^"]+)", path: (.*):(\d+):(\d+)$/gm;
  let match;
  while ((match = regex.exec(output)) !== null) {
    positions.push({
      file: match[6],
      line: Number(match[7]),
      column: Number(match[8]),
      name: match[5],
    });
  }
  return positions;
}

async function queryDefinitions(tool, database, positions, args, deadlineAt) {
  const resolutions = [];
  const context = {
    timedOutFiles: new Map(),
    skippedFiles: new Set(),
  };
  const positionBatches = batches(positions, args.queryBatchSize);
  for (let index = 0; index < positionBatches.length; index += 1) {
    if (Date.now() >= deadlineAt) {
      console.error("[aka-stack-graphs-python] stop query: adapter deadline exceeded");
      break;
    }
    const batch = positionBatches[index].filter((position) => !context.skippedFiles.has(position.file));
    if (batch.length === 0) {
      continue;
    }
    console.error(`[aka-stack-graphs-python] query batch ${index + 1}/${positionBatches.length} positions=${batch.length}`);
    resolutions.push(...await queryDefinitionBatch(
      tool,
      database,
      batch,
      args,
      deadlineAt,
      `batch ${index + 1}`,
      context,
    ));
  }
  return resolutions;
}

async function queryDefinitionBatch(tool, database, positions, args, deadlineAt, label, context) {
  positions = positions.filter((position) => !context.skippedFiles.has(position.file));
  if (positions.length === 0) {
    return [];
  }
  if (Date.now() >= deadlineAt) {
    console.error(`[aka-stack-graphs-python] skip query ${label}: adapter deadline exceeded`);
    return [];
  }
  try {
    const { stdout } = await runTool(tool, [
      "query",
      "-D",
      database,
      "definition",
      ...positions.map((position) => `${position.file}:${position.line}:${position.column}`),
    ], {
      timeoutMs: Math.min(args.queryTimeoutSecs * 1000, remainingMs(deadlineAt)),
      forwardStderr: false,
    });
    return parseDefinitionOutput(stdout, positions);
  } catch (error) {
    if (positions.length === 1) {
      const position = positions[0];
      console.error(
        `[aka-stack-graphs-python] skip query ${position.file}:${position.line}:${position.column}: ${error.message}`,
      );
      recordQueryTimeout(position.file, args, context);
      return [];
    }
    const timedOutFile = singleFile(positions);
    if (timedOutFile) {
      recordQueryTimeout(timedOutFile, args, context);
      if (context.skippedFiles.has(timedOutFile)) {
        return [];
      }
    }
    const split = Math.max(1, Math.floor(positions.length / 2));
    console.error(
      `[aka-stack-graphs-python] split query ${label} positions=${positions.length}: ${error.message}`,
    );
    const left = await queryDefinitionBatch(
      tool,
      database,
      positions.slice(0, split),
      args,
      deadlineAt,
      `${label}.left`,
      context,
    );
    if (Date.now() >= deadlineAt) {
      return left;
    }
    const right = await queryDefinitionBatch(
      tool,
      database,
      positions.slice(split),
      args,
      deadlineAt,
      `${label}.right`,
      context,
    );
    return [...left, ...right];
  }
}

function singleFile(positions) {
  const file = positions[0]?.file;
  if (!file) {
    return null;
  }
  return positions.every((position) => position.file === file) ? file : null;
}

function recordQueryTimeout(file, args, context) {
  if (context.skippedFiles.has(file)) {
    return;
  }
  const count = (context.timedOutFiles.get(file) ?? 0) + 1;
  context.timedOutFiles.set(file, count);
  if (count >= args.maxQueryTimeoutsPerFile) {
    context.skippedFiles.add(file);
    console.error(
      `[aka-stack-graphs-python] skip remaining query positions in ${file}: ${count} definition query timeouts`,
    );
  }
}

function parseDefinitionOutput(output, positions) {
  const byPosition = new Map(positions.map((position) => [
    `${position.file}:${position.line}:${position.column}`,
    position,
  ]));
  const resolutions = [];
  let current = null;
  let readingDefinitions = false;
  for (const line of output.split(/\r?\n/)) {
    const header = /^(.*):(\d+):(\d+): found \d+ definitions? for \d+ references?$/.exec(line);
    if (header) {
      const key = `${header[1]}:${header[2]}:${header[3]}`;
      current = byPosition.get(key) ?? {
        file: header[1],
        line: Number(header[2]),
        column: Number(header[3]),
        name: "",
      };
      readingDefinitions = false;
      continue;
    }
    if (!current) {
      continue;
    }
    if (/^\s*has (?:\d+ )?definitions?$/.test(line)) {
      readingDefinitions = true;
      continue;
    }
    if (/^\s*has no definitions$/.test(line) || /^\s*(?:queried reference|found \d+ references at position)/.test(line)) {
      readingDefinitions = false;
      continue;
    }
    if (!readingDefinitions) {
      continue;
    }
    const definition = /^\s*(.*):(\d+):(\d+):$/.exec(line);
    if (definition) {
      resolutions.push({
        source: current,
        target: {
          file: definition[1],
          line: Number(definition[2]),
          column: Number(definition[3]),
        },
      });
    }
  }
  return resolutions;
}

function stableId(prefix, value) {
  return `${prefix}:${createHash("sha1").update(value).digest("hex").slice(0, 20)}`;
}

async function readLine(file, line) {
  try {
    const text = await readFile(file, "utf8");
    return text.split(/\r?\n/)[Math.max(0, line - 1)] ?? "";
  } catch {
    return "";
  }
}

function identifierAt(lineText, oneBasedColumn, fallback = "") {
  const start = Math.max(0, oneBasedColumn - 1);
  const suffix = lineText.slice(start);
  const match = /^[A-Za-z_][A-Za-z0-9_]*/.exec(suffix);
  return match?.[0] ?? fallback;
}

function toRepoRelative(repo, file) {
  const absolute = path.resolve(file);
  if (absolute.startsWith(`${repo}${path.sep}`)) {
    return relativePath(repo, absolute);
  }
  return absolute;
}

async function lowerResolutions(repo, files, resolutions, noChunks) {
  const nodes = [];
  const edges = [];
  const chunks = [];
  const seenNodes = new Set();
  const seenEdges = new Set();
  const fileNodeIds = new Map();

  function addNode(node) {
    if (!seenNodes.has(node.id)) {
      seenNodes.add(node.id);
      nodes.push(node);
    }
  }

  function addEdge(edge) {
    if (!seenEdges.has(edge.id)) {
      seenEdges.add(edge.id);
      edges.push(edge);
    }
  }

  function fileNode(file) {
    const rel = toRepoRelative(repo, file);
    let id = fileNodeIds.get(rel);
    if (!id) {
      id = stableId("stack-graphs:file", rel);
      fileNodeIds.set(rel, id);
      addNode({
        id,
        label: "File",
        properties: {
          name: rel,
          path: rel,
          filePath: rel,
          language: "python",
        },
      });
    }
    return { id, rel };
  }

  for (const file of files) {
    fileNode(file);
  }

  const lineCache = new Map();
  async function lineText(file, line) {
    const key = `${file}:${line}`;
    if (!lineCache.has(key)) {
      lineCache.set(key, await readLine(file, line));
    }
    return lineCache.get(key);
  }

  for (const resolution of resolutions) {
    const sourceFile = fileNode(resolution.source.file);
    const targetFile = fileNode(resolution.target.file);
    const sourceLine = await lineText(resolution.source.file, resolution.source.line);
    const targetLine = await lineText(resolution.target.file, resolution.target.line);
    const sourceName = resolution.source.name || identifierAt(sourceLine, resolution.source.column);
    const targetName = identifierAt(targetLine, resolution.target.column, sourceName);
    if (!sourceName || !targetName) {
      continue;
    }
    const sourceNodeId = stableId(
      "stack-graphs:ref",
      `${sourceFile.rel}:${resolution.source.line}:${resolution.source.column}:${sourceName}`,
    );
    const targetNodeId = stableId(
      "stack-graphs:symbol",
      `${targetFile.rel}:${resolution.target.line}:${resolution.target.column}:${targetName}`,
    );
    addNode({
      id: sourceNodeId,
      label: "Reference",
      properties: {
        name: sourceName,
        qualifiedName: `${sourceFile.rel}:${sourceName}`,
        filePath: sourceFile.rel,
        language: "python",
        startLine: resolution.source.line - 1,
        endLine: resolution.source.line - 1,
        startCol: resolution.source.column - 1,
        endCol: resolution.source.column - 1 + sourceName.length,
      },
    });
    addNode({
      id: targetNodeId,
      label: "Symbol",
      properties: {
        name: targetName,
        qualifiedName: `${targetFile.rel}:${targetName}`,
        filePath: targetFile.rel,
        language: "python",
        startLine: resolution.target.line - 1,
        endLine: resolution.target.line - 1,
        startCol: resolution.target.column - 1,
        endCol: resolution.target.column - 1 + targetName.length,
      },
    });
    addEdge({
      id: stableId("stack-graphs:edge:file-contains-ref", `${sourceFile.id}->${sourceNodeId}`),
      sourceId: sourceFile.id,
      targetId: sourceNodeId,
      type: "CONTAINS",
      confidence: 1,
      reason: "stack-graphs queried reference position",
      evidence: { source: "stack-graphs", rule: "query-position", filePath: sourceFile.rel },
    });
    addEdge({
      id: stableId("stack-graphs:edge:file-defines-symbol", `${targetFile.id}->${targetNodeId}`),
      sourceId: targetFile.id,
      targetId: targetNodeId,
      type: "DEFINES",
      confidence: 1,
      reason: "stack-graphs resolved definition position",
      evidence: { source: "stack-graphs", rule: "definition-position", filePath: targetFile.rel },
    });
    addEdge({
      id: stableId("stack-graphs:edge:refers", `${sourceNodeId}->${targetNodeId}`),
      sourceId: sourceNodeId,
      targetId: targetNodeId,
      type: "REFERS_TO",
      confidence: 1,
      reason: "stack-graphs definition resolution",
      evidence: {
        source: "stack-graphs",
        rule: "definition",
        query: {
          filePath: sourceFile.rel,
          line: resolution.source.line,
          column: resolution.source.column,
        },
        definition: {
          filePath: targetFile.rel,
          line: resolution.target.line,
          column: resolution.target.column,
        },
      },
    });
    if (!noChunks) {
      const sourceText = sourceLine.trim();
      if (sourceText) {
        chunks.push({
          nodeId: sourceNodeId,
          kind: "stack-graphs-reference",
          filePath: sourceFile.rel,
          startLine: resolution.source.line - 1,
          endLine: resolution.source.line - 1,
          text: sourceText,
        });
      }
      const targetText = targetLine.trim();
      if (targetText) {
        chunks.push({
          nodeId: targetNodeId,
          kind: "stack-graphs-definition",
          filePath: targetFile.rel,
          startLine: resolution.target.line - 1,
          endLine: resolution.target.line - 1,
          text: targetText,
        });
      }
    }
  }
  return { nodes, edges, chunks };
}

async function main() {
  const args = parseArgs(process.argv);
  const repo = path.resolve(args.repo);
  const out = path.resolve(args.out);
  const database = path.resolve(args.database ?? path.join(path.dirname(out), "stack-graphs-python.db"));
  const deadlineAt = Date.now() + args.timeoutSecs * 1000;

  const discovered = await discoverPythonFiles(repo, args.maxFiles, args.excludeDirs);
  const files = discovered.files;
  if (files.length === 0) {
    throw new Error(`no Python files found in ${repo}`);
  }
  console.error(`[aka-stack-graphs-python] repo=${repo} files=${files.length} out=${out} database=${database}`);
  for (const skipped of discovered.skipped.slice(0, 20)) {
    console.error(`[aka-stack-graphs-python] skip file ${skipped.file}: ${skipped.reason}`);
  }
  if (discovered.skipped.length > 20) {
    console.error(`[aka-stack-graphs-python] skipped ${discovered.skipped.length - 20} additional unreadable files`);
  }

  const version = args.toolVersion || await toolVersion(args.tool, deadlineAt);
  await mkdir(path.dirname(out), { recursive: true });
  const indexed = await indexFiles(args.tool, database, files, args, deadlineAt);
  if (indexed.indexed.length === 0) {
    throw new Error("stack-graphs indexed no Python files");
  }
  console.error(
    `[aka-stack-graphs-python] indexed files=${indexed.indexed.length} skipped=${discovered.skipped.length + indexed.skipped.length}`,
  );
  const positions = await collectQueryPositions(args.tool, indexed.indexed, args, deadlineAt);
  if (positions.length === 0) {
    throw new Error("stack-graphs produced no query positions");
  }
  console.error(`[aka-stack-graphs-python] query positions=${positions.length}`);
  const resolutions = await queryDefinitions(args.tool, database, positions, args, deadlineAt);
  if (resolutions.length === 0) {
    throw new Error("stack-graphs produced no definition resolutions");
  }
  console.error(`[aka-stack-graphs-python] resolutions=${resolutions.length}`);
  const { nodes, edges, chunks } = await lowerResolutions(repo, files, resolutions, args.noChunks);
  const bundle = {
    analyzer: {
      analyzerId: "stack-graphs",
      toolVersion: version,
    },
    stats: {
      files: indexed.indexed.length,
      nodes: nodes.length,
      edges: edges.length,
      chunks: chunks.length,
    },
    diagnostics: {
      skippedFiles: [
        ...discovered.skipped.map((item) => ({
          path: toRepoRelative(repo, item.file),
          reason: item.reason,
        })),
        ...indexed.skipped.map((item) => ({
          path: toRepoRelative(repo, item.file),
          reason: item.reason,
        })),
      ],
    },
    nodes,
    edges,
    chunks,
  };
  await writeFile(out, `${JSON.stringify(bundle)}\n`);
  console.error(
    `[aka-stack-graphs-python] wrote nodes=${nodes.length} edges=${edges.length} chunks=${chunks.length} toolVersion=${version}`,
  );
}

main().catch((error) => {
  console.error(`error: ${error.stack || error.message}`);
  process.exit(1);
});
