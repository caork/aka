const DEFAULT_LOCAL_API_BASE = "http://127.0.0.1:4111";

function configuredApiBase(): string | undefined {
  return import.meta.env.VITE_AKA_API_BASE?.trim() || undefined;
}

export function apiUrl(path: string): string {
  const base = configuredApiBase() ?? (import.meta.env.PROD ? "" : DEFAULT_LOCAL_API_BASE);
  return `${base}${path}`;
}

export function localServeUnavailable(): Error {
  return new Error(
    import.meta.env.PROD
      ? "无法连接 aka 服务"
      : "无法连接本地 aka serve（127.0.0.1:4111）",
  );
}
