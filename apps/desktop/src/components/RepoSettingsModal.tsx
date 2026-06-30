import { motion } from "framer-motion";
import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { isDesktopRuntime } from "../desktop-api";
import { buildIndexLogText, indexLogLines } from "../index-log";
import {
  deleteRepo,
  setRepoSettings,
  updateRepo,
  updateZip,
} from "../repo-api";
import {
  refreshRepos,
  RENDER_MAX_DEFAULT,
  RENDER_MAX_LIMIT,
  RENDER_MAX_MIN,
  useAppStore,
  type Repo,
} from "../store";
import Modal, { ErrorBar } from "./Modal";

function clampRender(n: number): number {
  if (!Number.isFinite(n)) return RENDER_MAX_DEFAULT;
  return Math.min(RENDER_MAX_LIMIT, Math.max(RENDER_MAX_MIN, Math.round(n)));
}

/** 每仓库设置：embeddings 开关 / 图渲染节点上限 / 更新重建 / 移除。 */
export default function RepoSettingsModal({
  repo,
  onClose,
}: {
  repo: Repo | null;
  onClose(): void;
}) {
  const [embeddings, setEmbeddings] = useState(false);
  const [descriptionDraft, setDescriptionDraft] = useState("");
  const [renderDraft, setRenderDraft] = useState(RENDER_MAX_DEFAULT);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [logsCopied, setLogsCopied] = useState(false);
  const [busy, setBusy] = useState<
    "" | "toggle" | "description" | "render" | "update" | "zip" | "delete"
  >("");
  const [confirmDelete, setConfirmDelete] = useState(false);
  const zipRef = useRef<HTMLInputElement>(null);

  /* 弹窗目标变化时同步初始状态 */
  useEffect(() => {
    setEmbeddings(repo?.embeddings ?? false);
    setDescriptionDraft(repo?.description ?? "");
    setRenderDraft(repo?.renderMaxNodes ?? RENDER_MAX_DEFAULT);
    setError(null);
    setNotice(null);
    setLogsCopied(false);
    setBusy("");
    setConfirmDelete(false);
  }, [repo?.id, repo?.embeddings, repo?.description, repo?.renderMaxNodes]);

  if (!repo) return <Modal open={false} onClose={onClose} title="" children={null} />;

  const fail = (e: unknown, fallback: string) => {
    setError(e instanceof Error ? e.message : fallback);
  };

  const toggleEmbeddings = async () => {
    if (busy) return;
    const next = !embeddings;
    setEmbeddings(next); /* 乐观切换 */
    setBusy("toggle");
    setError(null);
    setNotice(null);
    try {
      await setRepoSettings(repo.name, {
        embeddingsEnabled: next,
      });
      void refreshRepos();
    } catch (e) {
      setEmbeddings(!next); /* 回滚 */
      fail(e, "设置失败");
    } finally {
      setBusy("");
    }
  };

  const savedRender = repo.renderMaxNodes ?? RENDER_MAX_DEFAULT;
  const renderDirty = clampRender(renderDraft) !== savedRender;
  const normalizeDescription = (value: string) => {
    const trimmed = value.trim();
    return trimmed.length > 0 ? trimmed : null;
  };
  const descriptionDirty =
    normalizeDescription(descriptionDraft) !== (repo.description ?? null);

  const saveDescription = async () => {
    if (busy || !descriptionDirty) return;
    setBusy("description");
    setError(null);
    setNotice(null);
    try {
      const description = normalizeDescription(descriptionDraft);
      await setRepoSettings(repo.name, {
        description,
      });
      if (description === null) setDescriptionDraft("");
      setNotice("仓库说明已保存——agent 的 list_repos 结果会带上这段说明");
      void refreshRepos();
    } catch (e) {
      fail(e, "设置失败");
    } finally {
      setBusy("");
    }
  };

  const saveRenderMax = async (value: number | null) => {
    if (busy) return;
    setBusy("render");
    setError(null);
    setNotice(null);
    try {
      await setRepoSettings(repo.name, {
        renderMaxNodes: value === null ? null : clampRender(value),
      });
      if (value === null) setRenderDraft(RENDER_MAX_DEFAULT);
      setNotice("渲染上限已保存——重新进入 Graph 视图生效");
      void refreshRepos();
    } catch (e) {
      fail(e, "设置失败");
    } finally {
      setBusy("");
    }
  };

  const runUpdate = async () => {
    if (busy) return;
    setBusy("update");
    setError(null);
    setNotice(null);
    try {
      await updateRepo(repo.name);
      setNotice("已开始更新并重建索引——侧栏将显示 indexing 进度");
      void refreshRepos();
    } catch (e) {
      fail(e, "更新失败");
    } finally {
      setBusy("");
    }
  };

  const runZipUpdate = async (fileOrPath: File | string | undefined | null) => {
    if (!fileOrPath || busy) return;
    const name =
      typeof fileOrPath === "string"
        ? fileOrPath.split(/[\\/]/).pop() ?? fileOrPath
        : fileOrPath.name;
    if (!name.toLowerCase().endsWith(".zip")) {
      setError("仅支持 .zip 文件");
      return;
    }
    setBusy("zip");
    setError(null);
    setNotice(null);
    try {
      await updateZip(repo.name, fileOrPath);
      setNotice("新 zip 已上传——侧栏将显示 indexing 进度");
      void refreshRepos();
    } catch (e) {
      fail(e, "上传失败");
    } finally {
      setBusy("");
    }
  };

  const pickZipUpdate = async () => {
    if (busy) return;
    if (!isDesktopRuntime()) {
      zipRef.current?.click();
      return;
    }
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
      if (typeof selected === "string") {
        void runZipUpdate(selected);
      }
    } catch (e) {
      fail(e, "选择 zip 失败");
    }
  };

  const runDelete = async () => {
    if (busy) return;
    if (!confirmDelete) {
      setConfirmDelete(true);
      return;
    }
    setBusy("delete");
    setError(null);
    try {
      await deleteRepo(repo.name);
      /* 若删的是当前选中仓库，refreshRepos 会自动回落到首个仓库 */
      onClose();
      void refreshRepos();
    } catch (e) {
      setBusy("");
      setConfirmDelete(false);
      fail(e, "移除失败");
    }
  };

  const sourceText =
    repo.source.kind === "git"
      ? (repo.source.url ?? "git 仓库")
      : repo.source.kind === "zip"
        ? "zip 导入"
        : repo.path;
  const logs = indexLogLines(repo);
  const logText = buildIndexLogText(repo);
  const copyLogs = () => {
    void navigator.clipboard
      ?.writeText(logText)
      .then(() => {
        setLogsCopied(true);
        window.setTimeout(() => setLogsCopied(false), 1200);
      })
      .catch(() => undefined);
  };

  return (
    <Modal
      open
      onClose={onClose}
      title={
        <span className="flex items-center gap-2">
          <span className="truncate">{repo.name}</span>
          <span
            className="rounded-[6px] px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-ink-2"
            style={{ background: "var(--subtle-fill)" }}
          >
            {repo.source.kind}
          </span>
        </span>
      }
    >
      {error && <ErrorBar message={error} />}
      {notice && (
        <div
          className="mb-3 rounded-[10px] px-3 py-2 text-[12px]"
          style={{
            background: "var(--success-fill)",
            color: "var(--success-ink)",
          }}
          data-testid="settings-notice"
        >
          {notice}
        </div>
      )}

      {/* source info */}
      <div
        className="mono mb-4 truncate rounded-[10px] px-3 py-2 text-[11.5px] text-ink-3"
        style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
        title={sourceText}
        data-testid="settings-source"
      >
        {sourceText}
      </div>

      {/* agent guidance */}
      <div className="themed-divider mb-4 border-t pt-4">
        <label htmlFor="repo-description" className="text-[13px] font-medium text-ink">
          仓库说明
        </label>
        <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
          写给 agent 的搜索提示：这个仓库负责什么、遇到什么问题时应该查它。
        </div>
        <textarea
          id="repo-description"
          value={descriptionDraft}
          onChange={(e) => setDescriptionDraft(e.target.value)}
          disabled={busy === "description"}
          rows={4}
          maxLength={1200}
          placeholder="例如：AKA 桌面端和插件包源码；需要代码图谱、MCP、Tauri、WebGL 渲染、仓库索引相关上下文时搜索。"
          className="cmd-input mt-3 min-h-[92px] w-full resize-y px-3 py-2 text-[12.5px] leading-relaxed text-ink"
          data-testid="repo-description-input"
        />
        <div className="mt-2 flex items-center justify-between gap-3">
          <span className="text-[11px] text-ink-3">
            {descriptionDraft.length.toLocaleString()} / 1,200
          </span>
          <button
            onClick={() => void saveDescription()}
            disabled={busy !== "" || !descriptionDirty}
            className={`focus-ring rounded-[9px] px-3 py-1.5 text-[12px] font-semibold transition-all duration-150 ease-out ${
              descriptionDirty ? "btn-primary" : "text-ink-3 opacity-60"
            }`}
            style={
              descriptionDirty
                ? undefined
                : { boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }
            }
            data-testid="repo-description-save"
          >
            {busy === "description" ? "保存中…" : "保存"}
          </button>
        </div>
      </div>

      {/* embeddings switch */}
      <div className="mb-4 flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="text-[13px] font-medium text-ink">Embeddings</div>
          <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
            开启后将下载本地模型并回填向量（后续版本启用计算）
          </div>
        </div>
        <GlassSwitch
          on={embeddings}
          disabled={busy === "toggle"}
          onToggle={() => void toggleEmbeddings()}
        />
      </div>

      {/* render budget */}
      <div className="themed-divider mb-4 border-t pt-4">
        <div className="text-[13px] font-medium text-ink">图渲染节点上限</div>
        <div className="mt-0.5 text-[11.5px] leading-relaxed text-ink-3">
          默认 {RENDER_MAX_DEFAULT.toLocaleString()} · 架构上限{" "}
          {RENDER_MAX_LIMIT.toLocaleString()}（数据层不设限，仅控制单视口渲染量）
        </div>
        <input
          type="range"
          min={RENDER_MAX_MIN}
          max={RENDER_MAX_LIMIT}
          step={1000}
          value={clampRender(renderDraft)}
          onChange={(e) => setRenderDraft(Number(e.target.value))}
          disabled={busy === "render"}
          className="mt-3 w-full"
          style={{ accentColor: "var(--accent)" }}
          aria-label="图渲染节点上限"
          data-testid="render-max-slider"
        />
        <div className="mt-2 flex items-center gap-2">
          <span className="cmd-input flex h-8 w-[110px] items-center px-2.5">
            <input
              type="number"
              min={RENDER_MAX_MIN}
              max={RENDER_MAX_LIMIT}
              step={1000}
              value={renderDraft}
              onChange={(e) => setRenderDraft(Number(e.target.value))}
              onBlur={() => setRenderDraft(clampRender(renderDraft))}
              disabled={busy === "render"}
              className="tabular h-full w-full text-[12.5px]"
              data-testid="render-max-input"
            />
          </span>
          <span className="text-[11.5px] text-ink-3">节点</span>
          {repo.renderMaxNodes !== null && (
            <button
              onClick={() => void saveRenderMax(null)}
              disabled={busy !== ""}
              className="focus-ring rounded-[8px] px-2 py-1 text-[11.5px] text-ink-3 transition-colors duration-150 ease-out hover:text-[var(--accent)]"
              data-testid="render-max-reset"
            >
              恢复默认
            </button>
          )}
          <button
            onClick={() => void saveRenderMax(renderDraft)}
            disabled={busy !== "" || !renderDirty}
            className={`focus-ring ml-auto rounded-[9px] px-3 py-1.5 text-[12px] font-semibold transition-all duration-150 ease-out ${
              renderDirty
                ? "btn-primary"
                : "text-ink-3 opacity-60"
            }`}
            style={
              renderDirty
                ? undefined
                : { boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }
            }
            data-testid="render-max-save"
          >
            {busy === "render" ? "保存中…" : "保存"}
          </button>
        </div>
      </div>

      {/* actions */}
      <div className="themed-divider mb-4 border-t pt-4">
        {repo.source.kind === "zip" ? (
          <>
            <button
              onClick={() => void pickZipUpdate()}
              disabled={busy !== ""}
              className="themed-hover focus-ring w-full rounded-[10px] px-3 py-2 text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:text-ink"
              style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }}
              data-testid="settings-update-zip"
            >
              {busy === "zip" ? "上传中…" : "上传新 zip 更新"}
            </button>
            <input
              ref={zipRef}
              type="file"
              accept=".zip"
              className="hidden"
              onChange={(e) => {
                void runZipUpdate(e.target.files?.[0]);
                e.target.value = "";
              }}
            />
          </>
        ) : (
          <button
            onClick={() => void runUpdate()}
            disabled={busy !== ""}
            className="themed-hover focus-ring w-full rounded-[10px] px-3 py-2 text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:text-ink"
            style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline-strong)" }}
            data-testid="settings-update"
          >
            {busy === "update" ? "请求中…" : "检查更新并重建索引"}
          </button>
        )}
      </div>

      <div className="themed-divider mb-4 border-t pt-4">
        <div className="mb-2 flex items-center justify-between gap-3">
          <div className="text-[13px] font-medium text-ink">Index logs</div>
          <button
            type="button"
            onClick={copyLogs}
            disabled={logText.trim().length === 0}
            className="focus-ring rounded-[8px] px-2 py-1 text-[11.5px] text-ink-3 transition-colors duration-150 ease-out hover:text-ink disabled:opacity-50"
            style={{ boxShadow: "inset 0 0 0 0.5px var(--hairline)" }}
            data-testid="settings-copy-index-logs"
          >
            {logsCopied ? "Copied" : "Copy"}
          </button>
        </div>
        <div className="scroll-area max-h-[180px] rounded-[10px] bg-[var(--subtle-fill-2)] px-3 py-2">
          {logs.slice(-80).map((line, idx) => (
            <div key={`${idx}-${line}`} className="mono py-0.5 text-[11px] leading-relaxed text-ink-2">
              {line}
            </div>
          ))}
        </div>
      </div>

      {/* danger zone */}
      <div className="themed-divider border-t pt-4">
        <button
          onClick={() => void runDelete()}
          disabled={busy !== "" && busy !== "delete"}
          className="focus-ring w-full rounded-[10px] px-3 py-2 text-[12.5px] font-semibold transition-colors duration-150 ease-out"
          style={
            confirmDelete
              ? {
                  background: "var(--danger)",
                  color: "white",
                  boxShadow:
                    "0 0 0 1px rgba(255,59,48,.3), 0 0 18px rgba(255,59,48,.18)",
                }
              : {
                  color: "var(--danger-ink)",
                  boxShadow: "inset 0 0 0 0.5px rgba(255,59,48,0.35)",
                }
          }
          data-testid="settings-delete"
        >
          {busy === "delete"
            ? "移除中…"
            : confirmDelete
              ? "确认移除？此操作不可恢复"
              : "移除仓库"}
        </button>
        {confirmDelete && busy !== "delete" && (
          <button
            onClick={() => setConfirmDelete(false)}
            className="focus-ring mt-1.5 w-full rounded-[10px] px-3 py-1.5 text-[12px] text-ink-3 hover:text-ink-2"
          >
            取消
          </button>
        )}
      </div>
    </Modal>
  );
}

/** iOS 风格玻璃开关，开 = 蓝发光。 */
function GlassSwitch({
  on,
  disabled,
  onToggle,
}: {
  on: boolean;
  disabled?: boolean;
  onToggle(): void;
}) {
  return (
    <button
      role="switch"
      aria-checked={on}
      disabled={disabled}
      onClick={onToggle}
      className="focus-ring relative mt-0.5 h-[24px] w-[40px] flex-none rounded-full transition-colors duration-150 ease-out"
      style={{
        background: on ? "var(--accent)" : "var(--subtle-fill)",
        boxShadow: on
          ? "0 0 0 1px rgba(46,124,246,.28), 0 0 20px rgba(46,124,246,.18)"
          : "inset 0 0 0 0.5px var(--hairline)",
        opacity: disabled ? 0.6 : 1,
      }}
      data-testid="embeddings-switch"
    >
      <motion.span
        className="absolute top-[2px] h-[20px] w-[20px] rounded-full"
        style={{
          background: "color-mix(in srgb, white 92%, var(--canvas))",
          boxShadow: "0 1px 3px rgba(16,24,40,.25)",
        }}
        animate={{ left: on ? 18 : 2 }}
        transition={{ type: "spring", stiffness: 400, damping: 32 }}
      />
    </button>
  );
}

/** 当前选中仓库的便捷取值（设置弹窗常用）。 */
export function useRepoById(id: string | null): Repo | null {
  const repos = useAppStore((s) => s.repos);
  if (!id) return null;
  return repos.find((r) => r.id === id) ?? null;
}
