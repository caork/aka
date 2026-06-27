import {
  asDesktopError,
  invokeDesktop,
  isDesktopRuntime,
} from "./desktop-api";

export type ClientIntegrationId = "claude-code" | "opencode";

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
  if (!isDesktopRuntime()) {
    throw new Error("客户端插件安装管理仅在 AKA 桌面端可用");
  }
  try {
    return await invokeDesktop<ClientIntegrationsStatus>("client_integrations_status");
  } catch (e) {
    throw asDesktopError(e, "读取客户端插件状态失败");
  }
}

export async function installClientIntegration(
  request: InstallClientIntegrationRequest,
): Promise<ClientIntegrationsStatus> {
  if (!isDesktopRuntime()) {
    throw new Error("客户端插件安装管理仅在 AKA 桌面端可用");
  }
  try {
    return await invokeDesktop<ClientIntegrationsStatus>("install_client_integration", {
      request,
    });
  } catch (e) {
    throw asDesktopError(e, "安装客户端插件失败");
  }
}
