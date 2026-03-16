import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { fetchWithRetry, runWithControl, RequestOptions } from "../utils/request";
import { useAppStore } from "../store/useAppStore";

const WEB_RPC_TIMEOUT_MS = 60_000;
const WEB_SERVICE_CONTROL_TIMEOUT_MS = 15_000;

interface FileSystemWritableFileStreamLike {
  write(data: string | Blob): Promise<void>;
  close(): Promise<void>;
}

interface FileSystemFileHandleLike {
  createWritable(): Promise<FileSystemWritableFileStreamLike>;
}

interface FileSystemDirectoryHandleLike {
  name: string;
  getFileHandle(
    name: string,
    options?: { create?: boolean }
  ): Promise<FileSystemFileHandleLike>;
}

type BrowserWindow = Window &
  typeof globalThis & {
    showDirectoryPicker?: () => Promise<FileSystemDirectoryHandleLike>;
  };

interface ExportAccountFile {
  fileName: string;
  content: string;
}

interface WebRpcCommandConfig {
  rpcMethod: string;
  buildParams?: (
    params?: Record<string, unknown>
  ) => Record<string, unknown> | undefined;
  finalizeResult?: (
    result: unknown,
    params?: Record<string, unknown>,
    options?: RequestOptions
  ) => Promise<unknown> | unknown;
}

const WEB_RPC_COMMANDS: Record<string, WebRpcCommandConfig> = {
  service_initialize: {
    rpcMethod: "initialize",
  },
  service_startup_snapshot: {
    rpcMethod: "startup/snapshot",
    buildParams: (params) => {
      const requestLogLimit = asInteger(omitAddr(params).requestLogLimit);
      return requestLogLimit == null ? undefined : { requestLogLimit };
    },
  },
  service_account_list: {
    rpcMethod: "account/list",
    buildParams: (params) => {
      const source = omitAddr(params);
      const next: Record<string, unknown> = {};
      const page = asInteger(source.page);
      const pageSize = asInteger(source.pageSize);
      const query = asNonEmptyString(source.query);
      const filter = asNonEmptyString(source.filter);
      const groupFilter = asNonEmptyString(source.groupFilter);
      if (page != null) next.page = page;
      if (pageSize != null) next.pageSize = pageSize;
      if (query) next.query = query;
      if (filter) next.filter = filter;
      if (groupFilter && groupFilter !== "all") next.groupFilter = groupFilter;
      return isEmptyRecord(next) ? undefined : next;
    },
  },
  service_account_delete: {
    rpcMethod: "account/delete",
    buildParams: (params) => {
      const accountId = asNonEmptyString(omitAddr(params).accountId);
      return accountId ? { accountId } : undefined;
    },
  },
  service_account_delete_many: {
    rpcMethod: "account/deleteMany",
    buildParams: (params) => {
      const accountIds = asStringArray(omitAddr(params).accountIds);
      return { accountIds };
    },
  },
  service_account_delete_unavailable_free: {
    rpcMethod: "account/deleteUnavailableFree",
  },
  service_account_update: {
    rpcMethod: "account/update",
    buildParams: (params) => {
      const source = omitAddr(params);
      const accountId = asNonEmptyString(source.accountId);
      const sort = asInteger(source.sort);
      return {
        accountId,
        sort: sort ?? 0,
      };
    },
  },
  service_account_import: {
    rpcMethod: "account/import",
    buildParams: (params) => {
      const source = omitAddr(params);
      const contents = asStringArray(source.contents);
      const content = asNonEmptyString(source.content);
      if (content) {
        contents.push(content);
      }
      return { contents };
    },
  },
  service_usage_read: {
    rpcMethod: "account/usage/read",
    buildParams: (params) => {
      const accountId = asNonEmptyString(omitAddr(params).accountId);
      return accountId ? { accountId } : undefined;
    },
  },
  service_usage_list: {
    rpcMethod: "account/usage/list",
  },
  service_usage_aggregate: {
    rpcMethod: "account/usage/aggregate",
  },
  service_usage_refresh: {
    rpcMethod: "account/usage/refresh",
    buildParams: (params) => {
      const accountId = asNonEmptyString(omitAddr(params).accountId);
      return accountId ? { accountId } : undefined;
    },
  },
  service_requestlog_list: {
    rpcMethod: "requestlog/list",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        query: asString(source.query),
        statusFilter: asString(source.statusFilter) || "all",
        page: asInteger(source.page) ?? 1,
        pageSize: asInteger(source.pageSize) ?? 20,
      };
    },
  },
  service_requestlog_summary: {
    rpcMethod: "requestlog/summary",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        query: asString(source.query),
        statusFilter: asString(source.statusFilter) || "all",
      };
    },
  },
  service_requestlog_clear: {
    rpcMethod: "requestlog/clear",
  },
  service_requestlog_today_summary: {
    rpcMethod: "requestlog/today_summary",
  },
  service_gateway_transport_get: {
    rpcMethod: "gateway/transport/get",
  },
  service_gateway_transport_set: {
    rpcMethod: "gateway/transport/set",
    buildParams: (params) => {
      const source = omitAddr(params);
      const next: Record<string, unknown> = {};
      const sseKeepaliveIntervalMs = asInteger(source.sseKeepaliveIntervalMs);
      const upstreamStreamTimeoutMs = asInteger(source.upstreamStreamTimeoutMs);
      if (sseKeepaliveIntervalMs != null) {
        next.sseKeepaliveIntervalMs = sseKeepaliveIntervalMs;
      }
      if (upstreamStreamTimeoutMs != null) {
        next.upstreamStreamTimeoutMs = upstreamStreamTimeoutMs;
      }
      return isEmptyRecord(next) ? undefined : next;
    },
  },
  service_gateway_upstream_proxy_get: {
    rpcMethod: "gateway/upstreamProxy/get",
  },
  service_gateway_upstream_proxy_set: {
    rpcMethod: "gateway/upstreamProxy/set",
    buildParams: (params) => ({
      proxyUrl: asNullableString(omitAddr(params).proxyUrl),
    }),
  },
  service_gateway_route_strategy_get: {
    rpcMethod: "gateway/routeStrategy/get",
  },
  service_gateway_route_strategy_set: {
    rpcMethod: "gateway/routeStrategy/set",
    buildParams: (params) => ({
      strategy: asString(omitAddr(params).strategy),
    }),
  },
  service_gateway_manual_account_get: {
    rpcMethod: "gateway/manualAccount/get",
  },
  service_gateway_manual_account_set: {
    rpcMethod: "gateway/manualAccount/set",
    buildParams: (params) => ({
      accountId: asString(omitAddr(params).accountId),
    }),
  },
  service_gateway_manual_account_clear: {
    rpcMethod: "gateway/manualAccount/clear",
  },
  service_gateway_header_policy_get: {
    rpcMethod: "gateway/headerPolicy/get",
  },
  service_gateway_header_policy_set: {
    rpcMethod: "gateway/headerPolicy/set",
    buildParams: (params) => ({
      cpaNoCookieHeaderModeEnabled: Boolean(
        omitAddr(params).cpaNoCookieHeaderModeEnabled
      ),
    }),
  },
  service_gateway_background_tasks_get: {
    rpcMethod: "gateway/backgroundTasks/get",
  },
  service_gateway_background_tasks_set: {
    rpcMethod: "gateway/backgroundTasks/set",
    buildParams: (params) => {
      const source = omitAddr(params);
      const next: Record<string, unknown> = {};
      for (const key of [
        "usagePollingEnabled",
        "usagePollIntervalSecs",
        "gatewayKeepaliveEnabled",
        "gatewayKeepaliveIntervalSecs",
        "tokenRefreshPollingEnabled",
        "tokenRefreshPollIntervalSecs",
        "usageRefreshWorkers",
        "httpWorkerFactor",
        "httpWorkerMin",
        "httpStreamWorkerFactor",
        "httpStreamWorkerMin",
      ]) {
        if (source[key] !== undefined) {
          next[key] = source[key];
        }
      }
      return isEmptyRecord(next) ? undefined : next;
    },
  },
  service_listen_config_get: {
    rpcMethod: "service/listenConfig/get",
  },
  service_listen_config_set: {
    rpcMethod: "service/listenConfig/set",
    buildParams: (params) => ({
      mode: asString(omitAddr(params).mode),
    }),
  },
  service_login_start: {
    rpcMethod: "account/login/start",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        type: asString(source.loginType) || "chatgpt",
        openBrowser: false,
        note: asNullableString(source.note),
        tags: asNullableString(source.tags),
        groupName: asNullableString(source.groupName),
        workspaceId: asNullableString(source.workspaceId),
      };
    },
    finalizeResult: (result, params) => {
      const source = asRecord(result) ?? {};
      const openBrowser = params?.openBrowser !== false;
      const authUrl = asNonEmptyString(source.authUrl ?? source.auth_url);
      if (openBrowser && authUrl) {
        openBrowserWindow(authUrl);
      }
      return result;
    },
  },
  service_login_status: {
    rpcMethod: "account/login/status",
    buildParams: (params) => ({
      loginId: asString(omitAddr(params).loginId),
    }),
  },
  service_login_complete: {
    rpcMethod: "account/login/complete",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        state: asString(source.state),
        code: asString(source.code),
        redirectUri: asNullableString(source.redirectUri),
      };
    },
  },
  service_login_chatgpt_auth_tokens: {
    rpcMethod: "account/login/start",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        type: "chatgptAuthTokens",
        accessToken: asString(source.accessToken),
        refreshToken: asNullableString(source.refreshToken),
        idToken: asNullableString(source.idToken),
        chatgptAccountId: asNullableString(source.chatgptAccountId),
        workspaceId: asNullableString(source.workspaceId),
        chatgptPlanType: asNullableString(source.chatgptPlanType),
      };
    },
  },
  service_account_read: {
    rpcMethod: "account/read",
    buildParams: (params) => ({
      refreshToken: Boolean(omitAddr(params).refreshToken),
    }),
  },
  service_account_logout: {
    rpcMethod: "account/logout",
  },
  service_chatgpt_auth_tokens_refresh: {
    rpcMethod: "account/chatgptAuthTokens/refresh",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        reason: asString(source.reason) || "unauthorized",
        previousAccountId: asNullableString(source.previousAccountId),
      };
    },
  },
  service_apikey_list: {
    rpcMethod: "apikey/list",
  },
  service_apikey_read_secret: {
    rpcMethod: "apikey/readSecret",
    buildParams: (params) => ({
      id: asString(omitAddr(params).keyId),
    }),
  },
  service_apikey_create: {
    rpcMethod: "apikey/create",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        name: asNullableString(source.name),
        modelSlug: asNullableString(source.modelSlug),
        reasoningEffort: asNullableString(source.reasoningEffort),
        protocolType: asNullableString(source.protocolType),
        upstreamBaseUrl: asNullableString(source.upstreamBaseUrl),
        staticHeadersJson: asNullableString(source.staticHeadersJson),
      };
    },
  },
  service_apikey_models: {
    rpcMethod: "apikey/models",
    buildParams: (params) => {
      const refreshRemote = omitAddr(params).refreshRemote;
      return refreshRemote === undefined
        ? undefined
        : { refreshRemote: Boolean(refreshRemote) };
    },
  },
  service_apikey_usage_stats: {
    rpcMethod: "apikey/usageStats",
  },
  service_apikey_update_model: {
    rpcMethod: "apikey/updateModel",
    buildParams: (params) => {
      const source = omitAddr(params);
      return {
        id: asString(source.keyId),
        modelSlug: asNullableString(source.modelSlug),
        reasoningEffort: asNullableString(source.reasoningEffort),
        protocolType: asNullableString(source.protocolType),
        upstreamBaseUrl: asNullableString(source.upstreamBaseUrl),
        staticHeadersJson: asNullableString(source.staticHeadersJson),
      };
    },
  },
  service_apikey_delete: {
    rpcMethod: "apikey/delete",
    buildParams: (params) => ({
      id: asString(omitAddr(params).keyId),
    }),
  },
  service_apikey_disable: {
    rpcMethod: "apikey/disable",
    buildParams: (params) => ({
      id: asString(omitAddr(params).keyId),
    }),
  },
  service_apikey_enable: {
    rpcMethod: "apikey/enable",
    buildParams: (params) => ({
      id: asString(omitAddr(params).keyId),
    }),
  },
  app_settings_get: {
    rpcMethod: "appSettings/get",
    finalizeResult: (result) => normalizeWebAppSettingsPayload(result),
  },
  app_settings_set: {
    rpcMethod: "appSettings/set",
    buildParams: (params) => asRecord(omitAddr(params).patch) ?? {},
    finalizeResult: (result) => normalizeWebAppSettingsPayload(result),
  },
};

const WEB_SPECIAL_COMMANDS: Record<
  string,
  (
    params?: Record<string, unknown>,
    options?: RequestOptions
  ) => Promise<unknown>
> = {
  service_start: (params, options) =>
    callWebServiceControl("start", omitAddr(params), options),
  service_stop: (_params, options) =>
    callWebServiceControl("stop", undefined, options),
  service_account_import_by_file: () => pickImportFilesFromBrowser(false),
  service_account_import_by_directory: () => pickImportFilesFromBrowser(true),
  service_account_export_by_account_files: (_params, options) =>
    exportAccountsInBrowser(options),
  app_close_to_tray_on_close_get: async (_params, options) => {
    const payload = await callWebRpcMethod<unknown>(
      "appSettings/get",
      undefined,
      options
    );
    const source = asRecord(normalizeWebAppSettingsPayload(payload)) ?? {};
    return Boolean(source.closeToTrayOnClose) && Boolean(source.closeToTraySupported);
  },
  app_close_to_tray_on_close_set: async (params, options) => {
    const payload = await callWebRpcMethod<unknown>(
      "appSettings/set",
      {
        closeToTrayOnClose: Boolean(params?.enabled),
      },
      options
    );
    const source = asRecord(normalizeWebAppSettingsPayload(payload)) ?? {};
    return Boolean(source.closeToTrayOnClose) && Boolean(source.closeToTraySupported);
  },
  open_in_browser: async (params) => {
    const url = asNonEmptyString(params?.url);
    if (!url) {
      throw new Error("缺少链接地址");
    }
    openBrowserWindow(url);
    return { ok: true };
  },
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function asString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function asNonEmptyString(value: unknown): string {
  const normalized = asString(value);
  return normalized || "";
}

function asNullableString(value: unknown): string | null {
  const normalized = asString(value);
  return normalized ? normalized : null;
}

function asInteger(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) {
    return Math.trunc(value);
  }
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      return Math.trunc(parsed);
    }
  }
  return null;
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .map((item) => asString(item))
    .filter((item) => item.length > 0);
}

function omitAddr(
  params?: Record<string, unknown>
): Record<string, unknown> {
  const source = asRecord(params) ?? {};
  const { addr: _addr, ...rest } = source;
  return rest;
}

function isEmptyRecord(value: Record<string, unknown>): boolean {
  return Object.keys(value).length === 0;
}

function getErrorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error || "");
}

function resolveRpcErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  const record = asRecord(error);
  if (record?.message && typeof record.message === "string") {
    return record.message;
  }
  return error ? JSON.stringify(error) : "RPC 请求失败";
}

function throwIfBusinessError(payload: unknown): void {
  const msg = resolveBusinessErrorMessage(payload);
  if (msg) throw new Error(msg);
}

function normalizeWebAppSettingsPayload(payload: unknown): unknown {
  const source = asRecord(payload);
  if (!source) return payload;
  return {
    ...source,
    closeToTraySupported: false,
  };
}

function readResponseErrorMessage(status: number, body: string): string {
  const trimmed = body.trim();
  if (!trimmed) {
    return `请求失败（HTTP ${status}）`;
  }

  try {
    const parsed = JSON.parse(trimmed) as unknown;
    const source = asRecord(parsed);
    if (source?.error && typeof source.error === "string") {
      return source.error;
    }
    if (source?.message && typeof source.message === "string") {
      return source.message;
    }
  } catch {
    // ignore invalid JSON and fall back to raw text
  }

  return trimmed;
}

async function callWebRpcMethod<T>(
  rpcMethod: string,
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  const response = await fetchWithRetry(
    "/api/rpc",
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: Date.now(),
        method: rpcMethod,
        params: params ?? {},
      }),
    },
    {
      timeoutMs: WEB_RPC_TIMEOUT_MS,
      ...options,
    }
  );

  if (!response.ok) {
    const text = await response.text();
    throw new Error(readResponseErrorMessage(response.status, text));
  }

  const payload = (await response.json()) as unknown;
  const responseRecord = asRecord(payload);
  if (responseRecord && "error" in responseRecord) {
    throw new Error(resolveRpcErrorMessage(responseRecord.error));
  }
  if (responseRecord && "result" in responseRecord) {
    const result = responseRecord.result as T;
    throwIfBusinessError(result);
    return result;
  }

  throwIfBusinessError(payload);
  return payload as T;
}

async function callWebServiceControl<T>(
  action: "start" | "stop",
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  const response = await fetchWithRetry(
    `/api/service/${action}`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(params ?? {}),
    },
    {
      timeoutMs: WEB_SERVICE_CONTROL_TIMEOUT_MS,
      ...options,
    }
  );

  const text = await response.text();
  if (!response.ok) {
    throw new Error(readResponseErrorMessage(response.status, text));
  }

  const payload = text ? (JSON.parse(text) as unknown) : {};
  throwIfBusinessError(payload);
  return payload as T;
}

function openBrowserWindow(url: string): void {
  if (typeof window === "undefined") return;
  window.open(url, "_blank", "noopener,noreferrer");
}

function pickFilesFromBrowser(options: {
  directory?: boolean;
  accept?: string;
  multiple?: boolean;
}): Promise<File[]> {
  if (typeof document === "undefined" || typeof window === "undefined") {
    return Promise.resolve([]);
  }

  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = options.multiple ?? true;
    if (options.accept) {
      input.accept = options.accept;
    }
    if (options.directory) {
      input.setAttribute("webkitdirectory", "");
      (
        input as HTMLInputElement & {
          webkitdirectory?: boolean;
        }
      ).webkitdirectory = true;
    }
    input.style.position = "fixed";
    input.style.left = "-9999px";
    input.style.top = "0";
    document.body.appendChild(input);

    let settled = false;

    const cleanup = () => {
      window.removeEventListener("focus", handleFocus, true);
      input.remove();
    };

    const finish = (files: File[]) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(files);
    };

    const handleFocus = () => {
      window.setTimeout(() => {
        if (!settled) {
          finish([]);
        }
      }, 300);
    };

    input.addEventListener(
      "change",
      () => {
        finish(Array.from(input.files ?? []));
      },
      { once: true }
    );

    window.addEventListener("focus", handleFocus, true);
    input.click();
  });
}

async function readTrimmedFileContents(files: File[]): Promise<string[]> {
  const contents = await Promise.all(files.map((file) => file.text()));
  return contents
    .map((text) => String(text || "").trim())
    .filter((text) => text.length > 0);
}

async function pickImportFilesFromBrowser(
  directory: boolean
): Promise<Record<string, unknown>> {
  const files = await pickFilesFromBrowser({
    directory,
    multiple: true,
    accept: directory ? undefined : ".json,.txt,application/json,text/plain",
  });

  if (files.length === 0) {
    return {
      ok: true,
      canceled: true,
    };
  }

  const filteredFiles = directory
    ? files.filter((file) => file.name.toLowerCase().endsWith(".json"))
    : files;
  const contents = await readTrimmedFileContents(filteredFiles);
  const directoryPath =
    directory && filteredFiles.length > 0
      ? String(
          (
            filteredFiles[0] as File & {
              webkitRelativePath?: string;
            }
          ).webkitRelativePath || ""
        )
          .split("/")
          .filter(Boolean)[0] || ""
      : "";

  return {
    ok: true,
    canceled: false,
    directoryPath: directoryPath || undefined,
    fileCount: filteredFiles.length,
    filePaths: filteredFiles.map((file) => file.name),
    contents,
  };
}

function readExportFiles(payload: unknown): ExportAccountFile[] {
  const source = asRecord(payload) ?? {};
  const files = Array.isArray(source.files) ? source.files : [];
  return files.reduce<ExportAccountFile[]>((result, item) => {
    const current = asRecord(item);
    if (!current) return result;
    const fileName = asNonEmptyString(current.fileName ?? current.file_name);
    if (!fileName) return result;
    result.push({
      fileName,
      content: asString(current.content),
    });
    return result;
  }, []);
}

async function saveExportFilesWithDirectoryPicker(
  files: ExportAccountFile[]
): Promise<{ canceled: boolean; outputDir: string }> {
  const runtime = window as BrowserWindow;
  if (!runtime.showDirectoryPicker) {
    return downloadExportFiles(files);
  }

  try {
    const directoryHandle = await runtime.showDirectoryPicker();
    for (const file of files) {
      const fileHandle = await directoryHandle.getFileHandle(file.fileName, {
        create: true,
      });
      const writable = await fileHandle.createWritable();
      await writable.write(file.content);
      await writable.close();
    }
    return {
      canceled: false,
      outputDir: directoryHandle.name || "已选目录",
    };
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      return {
        canceled: true,
        outputDir: "",
      };
    }
    throw error;
  }
}

function downloadExportFiles(
  files: ExportAccountFile[]
): { canceled: boolean; outputDir: string } {
  for (const file of files) {
    const blob = new Blob([file.content], {
      type: "application/json;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = file.fileName;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }
  return {
    canceled: false,
    outputDir: "浏览器下载",
  };
}

async function exportAccountsInBrowser(
  options: RequestOptions = {}
): Promise<Record<string, unknown>> {
  const payload = await callWebRpcMethod<unknown>(
    "account/exportData",
    undefined,
    options
  );
  const source = asRecord(payload) ?? {};
  const files = readExportFiles(payload);
  const exported = asInteger(source.exported) ?? files.length;

  if (files.length === 0) {
    return {
      ok: true,
      canceled: false,
      exported,
      outputDir: "",
    };
  }

  const saved = await saveExportFilesWithDirectoryPicker(files);
  return {
    ok: true,
    canceled: saved.canceled,
    exported,
    outputDir: saved.outputDir,
  };
}

async function invokeWebRpc<T>(
  method: string,
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  const specialHandler = WEB_SPECIAL_COMMANDS[method];
  if (specialHandler) {
    return (await specialHandler(params, options)) as T;
  }

  const config = WEB_RPC_COMMANDS[method];
  if (!config) {
    throw new Error("当前操作仅支持桌面端");
  }

  const payload = await callWebRpcMethod<unknown>(
    config.rpcMethod,
    config.buildParams?.(params),
    options
  );
  const finalized = config.finalizeResult
    ? await config.finalizeResult(payload, params, options)
    : payload;
  return finalized as T;
}

export function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    Boolean((window as typeof window & { __TAURI__?: unknown }).__TAURI__)
  );
}

export function withAddr(
  params: Record<string, unknown> = {}
): Record<string, unknown> {
  const addr = useAppStore.getState().serviceStatus.addr;
  return {
    addr: addr || null,
    ...params,
  };
}

export function isCommandMissingError(err: unknown): boolean {
  const msg = getErrorMessage(err).toLowerCase();
  return (
    msg.includes("unknown command") ||
    msg.includes("not found") ||
    msg.includes("is not a registered")
  );
}

export async function invokeFirst<T>(
  methods: string[],
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  let lastErr: unknown;
  for (const method of methods) {
    try {
      return await invoke<T>(method, params, options);
    } catch (err) {
      lastErr = err;
      if (!isCommandMissingError(err)) {
        throw err;
      }
    }
  }
  throw lastErr || new Error("未配置可用命令");
}

export async function invoke<T>(
  method: string,
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  if (!isTauriRuntime()) {
    return invokeWebRpc(method, params, options);
  }

  const response = await runWithControl<unknown>(
    () => tauriInvoke(method, params || {}),
    options
  );

  const responseRecord = asRecord(response);
  if (responseRecord && "error" in responseRecord) {
    const error = responseRecord.error;
    throw new Error(
      typeof error === "string"
        ? error
        : asRecord(error)?.message
          ? String(asRecord(error)?.message)
          : JSON.stringify(error)
    );
  }

  if (responseRecord && "result" in responseRecord) {
    const payload = responseRecord.result as T;
    throwIfBusinessError(payload);
    return payload;
  }
  
  throwIfBusinessError(response);
  return response as T;
}

function resolveBusinessErrorMessage(payload: unknown): string {
  const source = asRecord(payload);
  if (!source) return "";
  const error = source.error;
  if (source.ok === false) {
    return typeof error === "string"
      ? error
      : asRecord(error)?.message
        ? String(asRecord(error)?.message)
        : "操作失败";
  }
  if (error) {
    return typeof error === "string"
      ? error
      : asRecord(error)?.message
        ? String(asRecord(error)?.message)
        : "";
  }
  return "";
}

export async function requestlogListViaHttpRpc<T>(
  params: {
    query?: string;
    statusFilter?: string;
    page?: number;
    pageSize?: number;
  },
  addr: string,
  options: RequestOptions = {}
): Promise<T> {
  // Desktop environment should use Tauri invoke for reliability
  if (isTauriRuntime()) {
    return invoke<T>(
      "service_requestlog_list",
      {
        query: params.query || "",
        statusFilter: params.statusFilter || "all",
        page: params.page ?? 1,
        pageSize: params.pageSize ?? 20,
        addr,
      },
      options
    );
  }

  // Fallback for web mode if needed (though not primary for this app)
  const body = JSON.stringify({
    jsonrpc: "2.0",
    id: Date.now(),
    method: "requestlog/list",
    params: {
      query: params.query || "",
      statusFilter: params.statusFilter || "all",
      page: params.page ?? 1,
      pageSize: params.pageSize ?? 20,
    },
  });

  const response = await fetchWithRetry(
    `http://${addr}/rpc`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
    },
    options
  );

  if (!response.ok) throw new Error(`RPC 请求失败（HTTP ${response.status}）`);
  const payload = (await response.json()) as Record<string, unknown>;
  return ((payload.result ?? payload) as T);
}
