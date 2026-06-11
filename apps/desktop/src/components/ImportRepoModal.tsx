import { motion } from "framer-motion";
import { useRef, useState } from "react";
import { importRepo, importZip } from "../repo-api";
import { refreshRepos } from "../store";
import Modal, { ErrorBar, Field, TextInput } from "./Modal";

type ImportKind = "git" | "local" | "zip";

const KINDS: { id: ImportKind; label: string }[] = [
  { id: "git", label: "Git" },
  { id: "local", label: "本地路径" },
  { id: "zip", label: "Zip" },
];

/** 导入新代码库 —— Git 链接 / 本地路径 / 上传 zip 三种来源。 */
export default function ImportRepoModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose(): void;
}) {
  const [kind, setKind] = useState<ImportKind>("git");
  const [gitUrl, setGitUrl] = useState("");
  const [gitName, setGitName] = useState("");
  const [localPath, setLocalPath] = useState("");
  const [zipName, setZipName] = useState("");
  const [zipFile, setZipFile] = useState<File | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const reset = () => {
    setGitUrl("");
    setGitName("");
    setLocalPath("");
    setZipName("");
    setZipFile(null);
    setError(null);
    setBusy(false);
  };

  const close = () => {
    if (busy) return;
    reset();
    onClose();
  };

  const pickZip = (file: File | null | undefined) => {
    if (!file) return;
    if (!file.name.toLowerCase().endsWith(".zip")) {
      setError("仅支持 .zip 文件");
      return;
    }
    setError(null);
    setZipFile(file);
    if (!zipName.trim()) {
      setZipName(file.name.replace(/\.zip$/i, ""));
    }
  };

  const canSubmit =
    !busy &&
    (kind === "git"
      ? gitUrl.trim().length > 0
      : kind === "local"
        ? localPath.trim().length > 0
        : zipFile !== null && zipName.trim().length > 0);

  const submit = async () => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      if (kind === "git") {
        await importRepo({
          kind: "git",
          url: gitUrl.trim(),
          name: gitName.trim() || undefined,
        });
      } else if (kind === "local") {
        await importRepo({ kind: "local", path: localPath.trim() });
      } else {
        await importZip(zipName.trim(), zipFile!);
      }
      reset();
      onClose();
      /* 侧栏立刻出现 indexing 项 + 自动轮询到 ready */
      void refreshRepos();
    } catch (e) {
      setBusy(false);
      setError(e instanceof Error ? e.message : "导入失败");
    }
  };

  return (
    <Modal open={open} onClose={close} title="Add repository">
      {/* source kind switcher */}
      <div
        className="glass-segmented mb-4 flex items-center gap-0.5 rounded-[10px] p-0.5"
        role="tablist"
        data-testid="import-kind-switcher"
      >
        {KINDS.map((k) => {
          const active = k.id === kind;
          return (
            <button
              key={k.id}
              role="tab"
              aria-selected={active}
              onClick={() => {
                setKind(k.id);
                setError(null);
              }}
              className="focus-ring relative flex-1 rounded-[8px] px-3 py-1.5 text-[12.5px] font-medium transition-colors duration-150 ease-out"
              style={{ color: active ? "#0f172a" : "#475569" }}
            >
              {active && (
                <motion.span
                  layoutId="import-kind-thumb"
                  transition={{ type: "spring", stiffness: 400, damping: 32 }}
                  className="glass-segment-thumb absolute inset-0 rounded-[8px]"
                />
              )}
              <span className="relative z-10">{k.label}</span>
            </button>
          );
        })}
      </div>

      {/* fixed-height zone so the modal never shifts when switching tabs */}
      <div className="min-h-[200px]">
        {error && <ErrorBar message={error} />}

      {kind === "git" && (
        <>
          <Field label="Git URL">
            <TextInput
              value={gitUrl}
              onChange={(e) => setGitUrl(e.target.value)}
              placeholder="https://github.com/user/repo.git"
              autoFocus
              data-testid="import-git-url"
            />
          </Field>
          <Field label="名称" hint="可选，默认取仓库名">
            <TextInput
              value={gitName}
              onChange={(e) => setGitName(e.target.value)}
              placeholder="repo"
              data-testid="import-git-name"
            />
          </Field>
        </>
      )}

      {kind === "local" && (
        <Field label="本地路径" hint="Tauri 环境后续接文件选择器">
          <TextInput
            value={localPath}
            onChange={(e) => setLocalPath(e.target.value)}
            placeholder="/path/to/repo"
            autoFocus
            data-testid="import-local-path"
          />
        </Field>
      )}

      {kind === "zip" && (
        <>
          <button
            onClick={() => fileRef.current?.click()}
            onDragOver={(e) => {
              e.preventDefault();
              setDragOver(true);
            }}
            onDragLeave={() => setDragOver(false)}
            onDrop={(e) => {
              e.preventDefault();
              setDragOver(false);
              pickZip(e.dataTransfer.files?.[0]);
            }}
            className="focus-ring mb-3 flex w-full flex-col items-center justify-center gap-1.5 rounded-[12px] px-4 py-6 text-center transition-colors duration-150 ease-out"
            style={{
              background: dragOver
                ? "rgba(46,124,246,0.07)"
                : "rgba(255,255,255,0.28)",
              backdropFilter: "blur(18px) saturate(170%)",
              WebkitBackdropFilter: "blur(18px) saturate(170%)",
              boxShadow: dragOver
                ? "inset 0 0 0 1.5px rgba(46,124,246,0.45)"
                : "inset 0 1px 0 rgba(255,255,255,0.7), inset 0 0 0 1px rgba(15,23,42,0.08)",
            }}
            data-testid="import-zip-drop"
          >
            {zipFile ? (
              <>
                <span className="mono max-w-full truncate text-[12.5px] font-semibold text-ink">
                  {zipFile.name}
                </span>
                <span className="text-[11px] text-ink-3">
                  {(zipFile.size / 1024 / 1024).toFixed(1)} MB · 点击重新选择
                </span>
              </>
            ) : (
              <>
                <span className="text-[12.5px] font-medium text-ink-2">
                  拖拽 .zip 到这里，或点击选择
                </span>
                <span className="text-[11px] text-ink-3">
                  压缩包内应为代码仓库根目录
                </span>
              </>
            )}
          </button>
          <input
            ref={fileRef}
            type="file"
            accept=".zip"
            className="hidden"
            onChange={(e) => {
              pickZip(e.target.files?.[0]);
              e.target.value = "";
            }}
            data-testid="import-zip-file"
          />
          <Field label="名称" hint="必填">
            <TextInput
              value={zipName}
              onChange={(e) => setZipName(e.target.value)}
              placeholder="repo"
              data-testid="import-zip-name"
            />
          </Field>
        </>
      )}

      </div>{/* end fixed-height zone */}

      <div className="mt-1 flex items-center justify-end gap-2">
        <button
          onClick={close}
          disabled={busy}
          className="focus-ring rounded-[10px] px-3.5 py-2 text-[12.5px] font-medium text-ink-2 transition-colors duration-150 ease-out hover:bg-[rgba(15,23,42,0.05)]"
        >
          取消
        </button>
        <button
          onClick={() => void submit()}
          disabled={!canSubmit}
          className="btn-primary focus-ring px-4 py-2 text-[12.5px] font-semibold disabled:cursor-not-allowed"
          style={{ opacity: canSubmit ? 1 : 0.45 }}
          data-testid="import-submit"
        >
          {busy ? "导入中…" : "导入"}
        </button>
      </div>
    </Modal>
  );
}
