#!/usr/bin/env node
// Convert TypeScript Language Server document symbols into an aka-facts OSS analyzer bundle.
//
// This adapter is intentionally outside the AKA runtime. It starts the external
// open-source typescript-language-server, asks it for document symbols, and
// writes a facts JSON bundle that AKA can validate and import through
// ossAnalyzerFactsPath.

import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const DEFAULT_SERVER = "npx --yes --package typescript-language-server@latest --package typescript@latest typescript-language-server --stdio";
const DEFAULT_TIMEOUT_SECS = 300;
const DEFAULT_CHUNK_LINES = 80;
const DEFAULT_LOG_EVERY = 100;
const DEFAULT_CONCURRENCY = 16;

const SKIP_DIRS = new Set([
  ".git",
  ".hg",
  ".svn",
  ".cache",
  ".idea",
  ".vscode",
  "node_modules",
  "target",
  "build",
  "dist",
  "out",
  "coverage",
  ".next",
  ".nuxt",
  ".turbo",
]);

const TS_EXTENSIONS = new Set([".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"]);

function usage() {
  console.error(`Usage:
  scripts/oss-analyzer-typescript-lsp.mjs --repo PATH --out PATH [options]

Required:
  --repo PATH              Repository root.
  --out PATH               Output aka-facts JSON bundle.

Options:
  --server CMD             TypeScript LSP command. Default: ${DEFAULT_SERVER}
  --tool-version VERSION   Override analyzer toolVersion when serverInfo omits it.
  --timeout-secs N         Whole adapter deadline. Default: ${DEFAULT_TIMEOUT_SECS}
  --max-files N            Optional cap for smoke/debug runs.
  --exclude-dir PATH       Additional repo-relative directory to skip. Repeatable.
  --concurrency N          Concurrent documentSymbol requests. Default: ${DEFAULT_CONCURRENCY}
  --chunk-lines N          Max source lines per symbol chunk. Default: ${DEFAULT_CHUNK_LINES}
  --log-every N            Progress log interval. Default: ${DEFAULT_LOG_EVERY}
  --no-chunks              Do not emit symbol chunks.
  --help                   Show this message.

Example:
  scripts/oss-analyzer-typescript-lsp.mjs \\
    --repo /src/TypeScript \\
    --out /src/TypeScript/.aka/typescript-oss-analyzer-facts.json
`);
}

function parseArgs(argv) {
  const args = {
    server: DEFAULT_SERVER,
    timeoutSecs: DEFAULT_TIMEOUT_SECS,
    chunkLines: DEFAULT_CHUNK_LINES,
    logEvery: DEFAULT_LOG_EVERY,
    concurrency: DEFAULT_CONCURRENCY,
    maxFiles: null,
    excludeDirs: [],
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
      case "--exclude-dir":
        args.excludeDirs.push(next());
        break;
      case "--concurrency":
        args.concurrency = Number(next());
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
  if (!Number.isFinite(args.concurrency) || args.concurrency <= 0) {
    throw new Error("--concurrency must be a positive number");
  }
  args.concurrency = Math.floor(args.concurrency);
  if (!Number.isFinite(args.chunkLines) || args.chunkLines < 0) {
    throw new Error("--chunk-lines must be zero or positive");
  }
  if (!Number.isFinite(args.logEvery) || args.logEvery <= 0) {
    throw new Error("--log-every must be a positive number");
  }
  return args;
}

function normalizeRelativeDir(value) {
  return value
    .replaceAll("\\", "/")
    .replace(/^\/+/, "")
    .replace(/\/+$/, "");
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
          console.error(`[typescript-language-server] ${line}`);
        }
      }
    });
    child.on("exit", (code, signal) => {
      const err = new Error(`TypeScript language server exited code=${code} signal=${signal}`);
      for (const { reject, timer } of this.pending.values()) {
        clearTimeout(timer);
        reject(err);
      }
      this.pending.clear();
    });
  }

  request(method, params, timeoutMs = this.remainingMs()) {
    const id = this.nextId++;
    this.send({ jsonrpc: "2.0", id, method, params });
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
            preferences: {
              includePackageJsonAutoImports: "off",
            },
            completions: {
              completeFunctionCalls: false,
            },
            tsserver: {
              maxTsServerMemory: 4096,
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

async function discoverTsFiles(root, maxFiles, excludeDirs) {
  const files = [];
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
      } else if (entry.isFile() && TS_EXTENSIONS.has(path.extname(entry.name))) {
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

function languageIdForPath(file) {
  switch (path.extname(file)) {
    case ".tsx":
      return "typescriptreact";
    case ".jsx":
      return "javascriptreact";
    case ".js":
    case ".mjs":
    case ".cjs":
      return "javascript";
    default:
      return "typescript";
  }
}

function lspKindLabel(kind) {
  switch (kind) {
    case 1:
      return "File";
    case 2:
    case 3:
    case 4:
      return "Module";
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
      "typescript:symbol",
      `${rel}:${qualifiedName}:${symbol.kind}:${range.start.line}:${range.start.character}:${range.end.line}:${range.end.character}`,
    );
    nodes.push({
      id: nodeId,
      label: lspKindLabel(symbol.kind),
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
        id: stableId("typescript:edge:contains", `${parent.nodeId}->${nodeId}`),
        sourceId: parent.nodeId,
        targetId: nodeId,
        type: "CONTAINS",
        confidence: 1,
        reason: "typescript-language-server documentSymbol hierarchy",
        evidence: { source: "lsp", rule: "documentSymbol", filePath: rel },
      });
    } else {
      edges.push({
        id: stableId("typescript:edge:defines", `${fileNodeId}->${nodeId}`),
        sourceId: fileNodeId,
        targetId: nodeId,
        type: "DEFINES",
        confidence: 1,
        reason: "typescript-language-server documentSymbol top-level definition",
        evidence: { source: "lsp", rule: "documentSymbol", filePath: rel },
      });
    }
    if (!options.noChunks) {
      const text = textForRange(lines, symbol.range ?? range, options.chunkLines);
      if (text.trim()) {
        chunks.push({
          nodeId,
          kind: "typescript-document-symbol",
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

function spawnServer(command) {
  const isWindows = process.platform === "win32";
  return isWindows
    ? spawn("cmd.exe", ["/d", "/s", "/c", command], { stdio: ["pipe", "pipe", "pipe"] })
    : spawn("bash", ["-lc", command], { stdio: ["pipe", "pipe", "pipe"] });
}

async function mapWithConcurrency(items, concurrency, worker) {
  let next = 0;
  async function runWorker() {
    while (true) {
      const index = next++;
      if (index >= items.length) {
        return;
      }
      await worker(items[index], index);
    }
  }
  await Promise.all(
    Array.from({ length: Math.min(concurrency, items.length) }, () => runWorker()),
  );
}

async function main() {
  const args = parseArgs(process.argv);
  const repo = path.resolve(args.repo);
  const out = path.resolve(args.out);
  const deadlineAt = Date.now() + args.timeoutSecs * 1000;

  const files = await discoverTsFiles(repo, args.maxFiles, args.excludeDirs);
  if (files.length === 0) {
    throw new Error(`no TypeScript/JavaScript files found in ${repo}`);
  }
  console.error(`[aka-typescript] repo=${repo} files=${files.length} out=${out} server=${args.server}`);

  const rpc = new JsonRpc(spawnServer(args.server), deadlineAt);
  const rootUri = pathToFileURL(repo.endsWith(path.sep) ? repo : `${repo}${path.sep}`).toString();
  let initResult;
  try {
    initResult = await rpc.request("initialize", {
      processId: process.pid,
      clientInfo: { name: "aka-typescript-lsp-adapter", version: "0.1" },
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
        hostInfo: "aka-typescript-lsp-adapter",
      },
    });
    rpc.notify("initialized", {});

    const toolVersion = args.toolVersion || initResult?.serverInfo?.version || "unknown";
    const nodesByFile = new Array(files.length);
    const edgesByFile = new Array(files.length);
    const chunksByFile = new Array(files.length);
    await mapWithConcurrency(files, args.concurrency, async (file, index) => {
      if (Date.now() >= deadlineAt) {
        throw new Error(`adapter timed out after ${args.timeoutSecs}s`);
      }
      const rel = relativePath(repo, file);
      if (index % args.logEvery === 0) {
        console.error(
          `[aka-typescript] documentSymbol ${index + 1}/${files.length} ${rel} concurrency=${args.concurrency}`,
        );
      }
      const text = await readFile(file, "utf8");
      const uri = pathToFileURL(file).toString();
      const fileNodeId = stableId("typescript:file", rel);
      const fileNodes = [{
        id: fileNodeId,
        label: "File",
        properties: {
          name: rel,
          path: rel,
          filePath: rel,
          language: languageIdForPath(file),
        },
      }];
      const fileEdges = [];
      const fileChunks = [];
      rpc.notify("textDocument/didOpen", {
        textDocument: {
          uri,
          languageId: languageIdForPath(file),
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
        fileNodes.push(...lowered.nodes);
        fileEdges.push(...lowered.edges);
        fileChunks.push(...lowered.chunks);
      } catch (error) {
        console.error(`[aka-typescript] skip ${rel}: ${error.message}`);
      } finally {
        rpc.notify("textDocument/didClose", { textDocument: { uri } });
      }
      nodesByFile[index] = fileNodes;
      edgesByFile[index] = fileEdges;
      chunksByFile[index] = fileChunks;
    });
    const nodes = nodesByFile.flatMap((value) => value ?? []);
    const edges = edgesByFile.flatMap((value) => value ?? []);
    const chunks = chunksByFile.flatMap((value) => value ?? []);
    if (nodes.length <= files.length) {
      throw new Error("typescript-language-server produced no document symbols");
    }
    const bundle = {
      analyzer: {
        analyzerId: "typescript-language-server",
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
      `[aka-typescript] wrote nodes=${nodes.length} edges=${edges.length} chunks=${chunks.length} toolVersion=${toolVersion}`,
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
