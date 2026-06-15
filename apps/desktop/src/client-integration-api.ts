import { invokeDesktop, isDesktopRuntime } from "./desktop-api";

export const CLIENT_SYNC_AFTER_RESTART_KEY =
  "aka.syncClientIntegrationsAfterRestart";

export interface ClientIntegrationAction {
  client: string;
  detail: string;
}

export interface ClientIntegrationSyncResult {
  synced: ClientIntegrationAction[];
  skipped: ClientIntegrationAction[];
}

export interface ClientIntegrationSyncOptions {
  runCli?: boolean;
  createMissing?: boolean;
}

export async function syncClientIntegrations({
  runCli = false,
  createMissing = false,
}: ClientIntegrationSyncOptions = {}): Promise<ClientIntegrationSyncResult> {
  if (!isDesktopRuntime()) {
    return { synced: [], skipped: [] };
  }
  return invokeDesktop<ClientIntegrationSyncResult>("sync_client_integrations", {
    request: { runCli, createMissing },
  });
}

export function markClientIntegrationSyncAfterRestart(): void {
  try {
    window.localStorage.setItem(CLIENT_SYNC_AFTER_RESTART_KEY, "1");
  } catch {
    /* localStorage may be unavailable in tests/previews. */
  }
}

export function consumeClientIntegrationSyncAfterRestart(): boolean {
  try {
    const pending =
      window.localStorage.getItem(CLIENT_SYNC_AFTER_RESTART_KEY) === "1";
    if (pending) window.localStorage.removeItem(CLIENT_SYNC_AFTER_RESTART_KEY);
    return pending;
  } catch {
    return false;
  }
}
