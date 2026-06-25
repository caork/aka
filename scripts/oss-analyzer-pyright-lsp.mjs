#!/usr/bin/env node
// Convert Pyright LSP document symbols into an aka-facts OSS analyzer bundle.
//
// This adapter is intentionally outside the AKA runtime. It starts an external
// open-source Pyright language server, asks it for document symbols, and writes
// a facts JSON bundle that AKA can validate and import through ossAnalyzerFactsPath.

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const DEFAULT_SERVER = "pyright-langserver --stdio";
const DEFAULT_TIMEOUT_SECS = 300;
const DEFAULT_CHUNK_LINES = 80;
const DEFAULT_LOG_EVERY = 100;

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

function usage() {
  console.error(`Usage:
  scripts/oss-analyzer-pyright-lsp.mjs --repo PATH --out PATH [options]

Required:
  --repo PATH              Repository root.
  --out PATH               Output aka-facts JSON bundle.

Options:
  --server CMD             Pyright LSP command. Default: ${DEFAULT_SERVER}
  --tool-version VERSION   Override analyzer toolVersion when serverInfo omits it.
  --timeout-secs N         Whole adapter deadline. Default: ${DEFAULT_TIMEOUT_SECS}
  --max-files N            Optional cap for smoke/debug runs.
  --chunk-lines N          Max source lines per symbol chunk. Default: ${DEFAULT_CHUNK_LINES}
  --log-every N            Progress log interval. Default: ${DEFAULT_LOG_EVERY}
  --no-chunks              Do not emit symbol chunks.
  --help                   Show this message.

Example:
  scripts/oss-analyzer-pyright-lsp.mjs \\
    --repo /src/cpython \\
    --out /src/cpython/.aka/oss-analyzer-facts.json \\
    --server 'npx --yes pyright@latest pyright-langserver --stdio'
`);
}

function parseArgs(argv) {
  const args = {
    server: DEFAULT_SERVER,
    timeoutSecs: DEFAULT_TIMEOUT_SECS,
    chunkLines: DEFAULT_CHUNK_LINES,
    logEvery: DEFAULT_LOG_EVERY,
    maxFiles: null,
    noChunks: false,
    toolVersion: null,
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
      case "--server":
        args.server = next();
        break;
      case "--tool-version":
        args.toolVersion = next();
        break;
      case "--timeout-secs":
        args.timeoutSecs = Number(next());
        break;
      case "--max-files":
        args.maxFiles = Number(next());
        break;
      case "--chunk-lines":
        args.chunkLines = Number(next());
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
  if (!Number.isFinite(args.timeoutSecs) || args.timeoutSecs <= 0) {
    throw new Error("--timeout-secs must be a positive number");
  }
  if (args.maxFiles !== null && (!Number.isFinite(args.maxFiles) || args.maxFiles <= 0)) {
    throw new Error("--max-files must be a positive number");
  }
  if (!Number.isFinite(args.chunkLines) || args.chunkLines < 0) {
    throw new Error("--chunk-lines must be zero or positive");
  }
  if (!Number.isFinite(args.logEvery) || args.logEvery <= 0) {
    throw new Error("--log-every must be a positive number");
  }
  return args;
}

class JsonRpc {
  constructor(child, deadlineAt) {
    this.child = child;
    this.deadlineAt = deadlineAt;
    this.nextId = 1;
    this.pending = new Map();
    this.buffer = Buffer.alloc(0);
    child.stdout.on("data", (chunk) => this.onData(chunk));
    child.stderr.on("data", (chunk) => {
      for (const line of chunk.toString("utf8").split(/\r?\n/)) {
        if (line.trim()) {
          console.error(`[pyright] ${line}`);
        }
      }
    });
    child.on("exit", (code, signal) => {
      const err = new Error(`Pyright language server exited code=${code} signal=${signal}`);
      for (const { reject, timer } of this.pending.values()) {
        clearTimeout(timer);
        reject(err);
      }
      this.pending.clear();
    });
  }

  request(method, params, timeoutMs = this.remainingMs()) {
    const id = this.nextId++;
    const payload = { jsonrpc: "2.0", id, method, params };
    this.send(payload);
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`LSP request timed out: ${method}`));
      }, Math.max(1, timeoutMs));
      this.pending.set(id, { method, resolve, reject, timer });
    });
  }

  notify(method, params) {
    this.send({ jsonrpc: "2.0", method, params });
  }

  respond(id, result, error = null) {
    const payload = error
      ? { jsonrpc: "2.0", id, error }
      : { jsonrpc: "2.0", id, result };
    this.send(payload);
  }

  send(payload) {
    const body = JSON.stringify(payload);
    const header = `Content-Length: ${Buffer.byteLength(body, "utf8")}\r\n\r\n`;
    this.child.stdin.write(header);
    this.child.stdin.write(body);
  }

  onData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    while (true) {
      const headerEnd = this.buffer.indexOf("\r\n\r\n");
      if (headerEnd < 0) {
        return;
      }
      const header = this.buffer.subarray(0, headerEnd).toString("ascii");
      const match = /^Content-Length:\s*(\d+)$/im.exec(header);
      if (!match) {
        throw new Error(`missing Content-Length header from LSP server: ${header}`);
      }
      const length = Number(match[1]);
      const bodyStart = headerEnd + 4;
      const bodyEnd = bodyStart + length;
      if (this.buffer.length < bodyEnd) {
        return;
      }
      const body = this.buffer.subarray(bodyStart, bodyEnd).toString("utf8");
      this.buffer = this.buffer.subarray(bodyEnd);
      this.onMessage(JSON.parse(body));
    }
  }

  onMessage(message) {
    if (Object.prototype.hasOwnProperty.call(message, "id") && !message.method) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      clearTimeout(pending.timer);
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(
          new Error(`${pending.method} failed: ${message.error.message ?? JSON.stringify(message.error)}`),
        );
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (Object.prototype.hasOwnProperty.call(message, "id") && message.method) {
      this.handleServerRequest(message);
    }
  }

  handleServerRequest(message) {
    switch (message.method) {
      case "workspace/configuration": {
        const items = message.params?.items ?? [];
        this.respond(
          message.id,
          items.map(() => ({
            python: {
              analysis: {
                autoSearchPaths: true,
                diagnosticMode: "openFilesOnly",
                typeCheckingMode: "off",
                useLibraryCodeForTypes: false,
              },
            },
          })),
        );
        break;
      }
      case "window/workDoneProgress/create":
      case "client/registerCapability":
      case "client/unregisterCapability":
        this.respond(message.id, null);
        break;
      default:
        this.respond(message.id, null);
        break;
    }
  }

  remainingMs() {
    return Math.max(1, this.deadlineAt - Date.now());
  }
}

async function discoverPythonFiles(root, maxFiles) {
  const files = [];
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
          await walk(path.join(dir, entry.name));
        }
      } else if (entry.isFile() && PY_EXTENSIONS.has(path.extname(entry.name))) {
        files.push(path.join(dir, entry.name));
      }
    }
  }
  await walk(root);
  return files;
}

function relativePath(root, file) {
  return path.relative(root, file).split(path.sep).join("/");
}

function stableId(prefix, value) {
  return `${prefix}:${createHash("sha1").update(value).digest("hex").slice(0, 20)}`;
}

function lspKindLabel(kind) {
  switch (kind) {
    case 1:
      return "File";
    case 5:
      return "Class";
    case 6:
    case 9:
      return "Method";
    case 7:
    case 8:
    case 22:
      return "Field";
    case 10:
      return "Enum";
    case 11:
      return "Interface";
    case 12:
      return "Function";
    case 13:
    case 14:
    case 20:
      return "Variable";
    case 23:
    case 26:
      return "Type";
    case 2:
    case 3:
    case 4:
      return "Module";
    default:
      return "Symbol";
  }
}

function textForRange(lines, range, maxLines) {
  if (maxLines === 0 || !range?.start || !range?.end) {
    return "";
  }
  const start = Math.max(0, range.start.line);
  const end = Math.min(lines.length - 1, range.end.line, start + maxLines - 1);
  if (end < start) {
    return "";
  }
  return lines.slice(start, end + 1).join("\n");
}

function flattenDocumentSymbols(symbols, fileNodeId, rel, lines, options) {
  const nodes = [];
  const edges = [];
  const chunks = [];

  function visit(symbol, parents) {
    const range = symbol.selectionRange ?? symbol.range ?? {
      start: { line: 0, character: 0 },
      end: { line: 0, character: 0 },
    };
    const qualifiedName = [...parents.map((parent) => parent.name), symbol.name].join(".");
    const nodeId = stableId(
      "pyright:symbol",
      `${rel}:${qualifiedName}:${symbol.kind}:${range.start.line}:${range.start.character}:${range.end.line}:${range.end.character}`,
    );
    const label = lspKindLabel(symbol.kind);
    nodes.push({
      id: nodeId,
      label,
      properties: {
        name: symbol.name,
        qualifiedName,
        filePath: rel,
        startLine: range.start.line,
        endLine: range.end.line,
        startCol: range.start.character,
        endCol: range.end.character,
        lspKind: symbol.kind,
        ...(symbol.detail ? { detail: symbol.detail } : {}),
      },
    });
    const parent = parents.at(-1);
    if (parent?.nodeId) {
      edges.push({
        id: stableId("pyright:edge:contains", `${parent.nodeId}->${nodeId}`),
        sourceId: parent.nodeId,
        targetId: nodeId,
        type: "CONTAINS",
        confidence: 1,
        reason: "pyright documentSymbol hierarchy",
        evidence: { source: "lsp", rule: "documentSymbol", filePath: rel },
      });
    } else {
      edges.push({
        id: stableId("pyright:edge:defines", `${fileNodeId}->${nodeId}`),
        sourceId: fileNodeId,
        targetId: nodeId,
        type: "DEFINES",
        confidence: 1,
        reason: "pyright documentSymbol top-level definition",
        evidence: { source: "lsp", rule: "documentSymbol", filePath: rel },
      });
    }
    if (!options.noChunks) {
      const text = textForRange(lines, symbol.range ?? range, options.chunkLines);
      if (text.trim()) {
        chunks.push({
          nodeId,
          kind: "pyright-document-symbol",
          filePath: rel,
          startLine: Math.max(0, (symbol.range ?? range).start.line),
          endLine: Math.max(0, (symbol.range ?? range).end.line),
          text,
        });
      }
    }
    const nextParents = [...parents, { name: symbol.name, nodeId }];
    for (const child of symbol.children ?? []) {
      visit(child, nextParents);
    }
  }

  for (const symbol of symbols ?? []) {
    visit(symbol, []);
  }
  return { nodes, edges, chunks };
}

function spawnServer(command, deadlineAt) {
  const isWindows = process.platform === "win32";
  const child = isWindows
    ? spawn("cmd.exe", ["/d", "/s", "/c", command], { stdio: ["pipe", "pipe", "pipe"] })
    : spawn("bash", ["-lc", command], { stdio: ["pipe", "pipe", "pipe"] });
  return new JsonRpc(child, deadlineAt);
}

async function main() {
  const args = parseArgs(process.argv);
  const repo = path.resolve(args.repo);
  const out = path.resolve(args.out);
  const deadlineAt = Date.now() + args.timeoutSecs * 1000;

  const files = await discoverPythonFiles(repo, args.maxFiles);
  if (files.length === 0) {
    throw new Error(`no Python files found in ${repo}`);
  }
  console.error(
    `[aka-pyright] repo=${repo} files=${files.length} out=${out} server=${args.server}`,
  );

  const rpc = spawnServer(args.server, deadlineAt);
  const rootUri = pathToFileURL(repo.endsWith(path.sep) ? repo : `${repo}${path.sep}`).toString();
  let initResult;
  try {
    initResult = await rpc.request("initialize", {
      processId: process.pid,
      clientInfo: { name: "aka-pyright-lsp-adapter", version: "0.1" },
      rootPath: repo,
      rootUri,
      workspaceFolders: [{ uri: rootUri, name: path.basename(repo) || "repo" }],
      capabilities: {
        textDocument: {
          documentSymbol: {
            hierarchicalDocumentSymbolSupport: true,
            symbolKind: { valueSet: Array.from({ length: 26 }, (_, i) => i + 1) },
          },
          synchronization: {
            didOpen: true,
            didClose: true,
          },
        },
        workspace: {
          configuration: true,
          workspaceFolders: true,
        },
        window: {
          workDoneProgress: true,
        },
      },
      initializationOptions: {
        typeCheckingMode: "off",
      },
    });
    rpc.notify("initialized", {});

    const toolVersion = args.toolVersion || initResult?.serverInfo?.version || "unknown";
    const nodes = [];
    const edges = [];
    const chunks = [];
    for (const [index, file] of files.entries()) {
      if (Date.now() >= deadlineAt) {
        throw new Error(`adapter timed out after ${args.timeoutSecs}s`);
      }
      const rel = relativePath(repo, file);
      if (index % args.logEvery === 0) {
        console.error(`[aka-pyright] documentSymbol ${index + 1}/${files.length} ${rel}`);
      }
      const text = await readFile(file, "utf8");
      const uri = pathToFileURL(file).toString();
      const fileNodeId = stableId("pyright:file", rel);
      nodes.push({
        id: fileNodeId,
        label: "File",
        properties: {
          name: rel,
          path: rel,
          filePath: rel,
          language: "python",
        },
      });
      rpc.notify("textDocument/didOpen", {
        textDocument: {
          uri,
          languageId: "python",
          version: 1,
          text,
        },
      });
      try {
        const symbols = await rpc.request(
          "textDocument/documentSymbol",
          { textDocument: { uri } },
          Math.min(60_000, rpc.remainingMs()),
        );
        const lowered = flattenDocumentSymbols(
          Array.isArray(symbols) ? symbols : [],
          fileNodeId,
          rel,
          text.split(/\r?\n/),
          args,
        );
        nodes.push(...lowered.nodes);
        edges.push(...lowered.edges);
        chunks.push(...lowered.chunks);
      } catch (error) {
        console.error(`[aka-pyright] skip ${rel}: ${error.message}`);
      } finally {
        rpc.notify("textDocument/didClose", { textDocument: { uri } });
      }
    }
    if (nodes.length <= files.length) {
      throw new Error("Pyright produced no document symbols");
    }
    const bundle = {
      analyzer: {
        analyzerId: "pyright",
        toolVersion,
      },
      stats: {
        files: files.length,
        nodes: nodes.length,
        edges: edges.length,
        chunks: chunks.length,
      },
      nodes,
      edges,
      chunks,
    };
    await mkdir(path.dirname(out), { recursive: true });
    await writeFile(out, `${JSON.stringify(bundle)}\n`);
    console.error(
      `[aka-pyright] wrote nodes=${nodes.length} edges=${edges.length} chunks=${chunks.length} toolVersion=${toolVersion}`,
    );
  } finally {
    try {
      await rpc.request("shutdown", null, 2_000);
    } catch {
      // best-effort shutdown
    }
    rpc.notify("exit", null);
    setTimeout(() => rpc.child.kill(), 500).unref();
  }
}

main().catch((error) => {
  console.error(`error: ${error.stack || error.message}`);
  process.exit(1);
});
