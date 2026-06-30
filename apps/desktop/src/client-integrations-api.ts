import {
  asDesktopError,
  invokeDesktop,
  isDesktopRuntime,
} from "./desktop-api";
import { apiUrl, localServeUnavailable } from "./api-base";

export type ClientIntegrationId = "claude-code" | "codex" | "opencode";

export interface ClientIntegrationPathStatus {
  label: string;
  path: string;
  exists: boolean;
}

export interface ClientIntegrationStatus {
  client: ClientIntegrationId;
  label: string;
  installed: boolean;
  available: boolean;
  health: string;
  summary: string;
  details: string[];
  version?: string | null;
  bundledVersion?: string | null;
  paths: ClientIntegrationPathStatus[];
}

export interface ClientIntegrationsStatus {
  mcpUrl: string;
  resourceDir?: string | null;
  clients: ClientIntegrationStatus[];
  lastAction?: string | null;
}

export interface InstallClientIntegrationRequest {
  client: ClientIntegrationId;
  reinstall?: boolean;
}

export async function getClientIntegrationsStatus(): Promise<ClientIntegrationsStatus> {
  if (isDesktopRuntime()) {
    try {
      return await invokeDesktop<ClientIntegrationsStatus>("client_integrations_status");
    } catch (e) {
      throw asDesktopError(e, "读取客户端插件状态失败");
    }
  }
  let r: Response;
  try {
    r = await fetch(apiUrl("/api/client-integrations"), {
      signal: AbortSignal.timeout(8000),
    });
  } catch {
    throw localServeUnavailable();
  }
  if (!r.ok) throw new Error(await responseMessage(r, "读取客户端插件状态失败"));
  return (await r.json()) as ClientIntegrationsStatus;
}

export async function installClientIntegration(
  request: InstallClientIntegrationRequest,
): Promise<ClientIntegrationsStatus> {
  if (isDesktopRuntime()) {
    try {
      return await invokeDesktop<ClientIntegrationsStatus>("install_client_integration", {
        request,
      });
    } catch (e) {
      throw asDesktopError(e, "安装客户端插件失败");
    }
  }
  let r: Response;
  try {
    r = await fetch(apiUrl("/api/client-integrations/install"), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(request),
      signal: AbortSignal.timeout(30_000),
    });
  } catch {
    throw localServeUnavailable();
  }
  if (!r.ok) throw new Error(await responseMessage(r, "安装客户端插件失败"));
  return (await r.json()) as ClientIntegrationsStatus;
}

async function responseMessage(response: Response, fallback: string): Promise<string> {
  try {
    const body = (await response.text()).trim();
    if (!body) return `${fallback}（HTTP ${response.status}）`;
    try {
      const json = JSON.parse(body) as { error?: unknown };
      if (typeof json.error === "string" && json.error.trim()) {
        return json.error.trim();
      }
    } catch {
      /* plain text */
    }
    return body.slice(0, 300);
  } catch {
    return `${fallback}（HTTP ${response.status}）`;
  }
}
