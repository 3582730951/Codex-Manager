import { invoke as tauriInvoke, isTauri as tauriIsTauri } from "@tauri-apps/api/core";
import { fetchWithRetry, runWithControl, RequestOptions } from "../utils/request";
import { useAppStore } from "../store/useAppStore";

type InvokeParams = Record<string, unknown>;

type WebCommandDescriptor = {
  rpcMethod?: string;
  mapParams?: (params?: InvokeParams) => InvokeParams;
  direct?: (params?: InvokeParams, options?: RequestOptions) => Promise<unknown>;
};

const WEB_COMMAND_MAP: Record<string, WebCommandDescriptor> = {
  app_settings_get: { rpcMethod: "appSettings/get" },
  app_settings_set: {
    rpcMethod: "appSettings/set",
    mapParams: (params) => asRecord(asRecord(params)?.patch) ?? {},
  },
  service_initialize: { rpcMethod: "initialize" },
  service_startup_snapshot: { rpcMethod: "startup/snapshot" },
  service_account_list: { rpcMethod: "account/list" },
  service_account_delete: { rpcMethod: "account/delete" },
  service_account_delete_many: { rpcMethod: "account/deleteMany" },
  service_account_delete_unavailable_free: {
    rpcMethod: "account/deleteUnavailableFree",
  },
  service_account_update: { rpcMethod: "account/update" },
  service_account_import: { rpcMethod: "account/import" },
  service_account_import_by_file: {
    direct: () => pickImportFilesFromBrowser(false),
  },
  service_account_import_by_directory: {
    direct: () => pickImportFilesFromBrowser(true),
  },
  service_account_export_by_account_files: {
    direct: (_params, options) => exportAccountsViaBrowser(options),
  },
  service_usage_read: { rpcMethod: "account/usage/read" },
  service_usage_list: { rpcMethod: "account/usage/list" },
  service_usage_refresh: { rpcMethod: "account/usage/refresh" },
  service_usage_aggregate: { rpcMethod: "account/usage/aggregate" },
  service_login_start: {
    rpcMethod: "account/login/start",
    mapParams: (params) => ({
      ...(params ?? {}),
      type:
        typeof params?.loginType === "string" && params.loginType.trim()
          ? params.loginType
          : "chatgpt",
      openBrowser: false,
    }),
  },
  service_login_status: { rpcMethod: "account/login/status" },
  service_login_complete: { rpcMethod: "account/login/complete" },
  service_login_chatgpt_auth_tokens: {
    rpcMethod: "account/login/start",
    mapParams: (params) => ({
      ...(params ?? {}),
      type: "chatgptAuthTokens",
    }),
  },
  service_account_read: { rpcMethod: "account/read" },
  service_account_logout: { rpcMethod: "account/logout" },
  service_chatgpt_auth_tokens_refresh: {
    rpcMethod: "account/chatgptAuthTokens/refresh",
  },
  service_apikey_list: { rpcMethod: "apikey/list" },
  service_apikey_create: { rpcMethod: "apikey/create" },
  service_apikey_usage_stats: { rpcMethod: "apikey/usageStats" },
  service_apikey_delete: {
    rpcMethod: "apikey/delete",
    mapParams: mapKeyIdToId,
  },
  service_apikey_update_model: {
    rpcMethod: "apikey/updateModel",
    mapParams: mapKeyIdToId,
  },
  service_apikey_disable: {
    rpcMethod: "apikey/disable",
    mapParams: mapKeyIdToId,
  },
  service_apikey_enable: {
    rpcMethod: "apikey/enable",
    mapParams: mapKeyIdToId,
  },
  service_apikey_models: { rpcMethod: "apikey/models" },
  service_apikey_read_secret: {
    rpcMethod: "apikey/readSecret",
    mapParams: mapKeyIdToId,
  },
  service_gateway_transport_get: { rpcMethod: "gateway/transport/get" },
  service_gateway_transport_set: { rpcMethod: "gateway/transport/set" },
  service_gateway_upstream_proxy_get: { rpcMethod: "gateway/upstreamProxy/get" },
  service_gateway_upstream_proxy_set: { rpcMethod: "gateway/upstreamProxy/set" },
  service_gateway_route_strategy_get: { rpcMethod: "gateway/routeStrategy/get" },
  service_gateway_route_strategy_set: { rpcMethod: "gateway/routeStrategy/set" },
  service_gateway_manual_account_get: { rpcMethod: "gateway/manualAccount/get" },
  service_gateway_manual_account_set: { rpcMethod: "gateway/manualAccount/set" },
  service_gateway_manual_account_clear: {
    rpcMethod: "gateway/manualAccount/clear",
  },
  service_gateway_background_tasks_get: {
    rpcMethod: "gateway/backgroundTasks/get",
  },
  service_gateway_background_tasks_set: {
    rpcMethod: "gateway/backgroundTasks/set",
  },
  service_requestlog_list: { rpcMethod: "requestlog/list" },
  service_requestlog_summary: { rpcMethod: "requestlog/summary" },
  service_requestlog_clear: { rpcMethod: "requestlog/clear" },
  service_requestlog_today_summary: { rpcMethod: "requestlog/today_summary" },
  service_listen_config_get: { rpcMethod: "service/listenConfig/get" },
  service_listen_config_set: { rpcMethod: "service/listenConfig/set" },
  open_in_browser: {
    direct: async (params) => {
      const url = typeof params?.url === "string" ? params.url.trim() : "";
      if (!url) {
        throw new Error("缺少浏览器跳转地址");
      }
      if (typeof window === "undefined") {
        throw new Error("当前环境不支持打开浏览器");
      }
      window.open(url, "_blank", "noopener,noreferrer");
      return { ok: true };
    },
  },
  open_in_file_manager: {
    direct: async () => {
      throw new Error("当前环境不支持打开本地目录");
    },
  },
  app_update_open_logs_dir: {
    direct: async () => {
      throw new Error("当前环境不支持打开更新日志目录");
    },
  },
};

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export function getAppErrorMessage(
  error: unknown,
  fallback = "操作失败"
): string {
  if (error instanceof Error) {
    const nested = getAppErrorMessage(error.message, "");
    return nested || fallback;
  }

  const businessMessage = resolveBusinessErrorMessage(error);
  if (businessMessage) return businessMessage;

  const rpcMessage = resolveRpcErrorMessage(error).trim();
  if (!rpcMessage || rpcMessage === "null" || rpcMessage === "undefined") {
    return fallback;
  }
  return rpcMessage;
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

async function invokeWebRpc<T>(
  method: string,
  params?: InvokeParams,
  options: RequestOptions = {}
): Promise<T> {
  const descriptor = WEB_COMMAND_MAP[method];
  if (!descriptor) {
    throw new Error("当前 Web / Docker 版暂不支持该操作");
  }
  if (descriptor.direct) {
    return (await descriptor.direct(params, options)) as T;
  }
  if (!descriptor.rpcMethod) {
    throw new Error("当前 Web / Docker 版暂不支持该操作");
  }
  return postWebRpc<T>(
    descriptor.rpcMethod,
    descriptor.mapParams ? descriptor.mapParams(params) : params ?? {},
    options
  );
}

async function postWebRpc<T>(
  rpcMethod: string,
  params?: InvokeParams,
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
    options
  );

  if (!response.ok) throw new Error(`RPC 请求失败（HTTP ${response.status}）`);

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

export function isTauriRuntime(): boolean {
  if (typeof window === "undefined") {
    return false;
  }

  const runtime = globalThis as typeof globalThis & {
    __TAURI__?: unknown;
    __TAURI_INTERNALS__?: { invoke?: unknown };
  };

  return (
    tauriIsTauri() ||
    Boolean(runtime.__TAURI_INTERNALS__?.invoke) ||
    Boolean(runtime.__TAURI__)
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
  const msg = getAppErrorMessage(err, "").toLowerCase();
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
  params?: InvokeParams,
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

function mapKeyIdToId(params?: InvokeParams): InvokeParams {
  const source = params ?? {};
  const keyId =
    typeof source.keyId === "string" && source.keyId.trim()
      ? source.keyId.trim()
      : undefined;
  if (!keyId) {
    return source;
  }
  return {
    ...source,
    id: keyId,
  };
}

async function pickImportFilesFromBrowser(directory: boolean): Promise<unknown> {
  if (typeof document === "undefined") {
    throw new Error("当前环境不支持浏览器文件选择");
  }

  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".json,.txt,application/json,text/plain";
  input.multiple = true;
  if (directory) {
    const directoryInput = input as HTMLInputElement & {
      directory?: boolean;
      webkitdirectory?: boolean;
    };
    directoryInput.directory = true;
    directoryInput.webkitdirectory = true;
  }
  input.style.display = "none";
  document.body.appendChild(input);

  return await new Promise<unknown>((resolve, reject) => {
    let finished = false;

    const cleanup = () => {
      input.removeEventListener("change", handleChange);
      input.removeEventListener("cancel", handleCancel as EventListener);
      input.remove();
    };

    const finish = (value: unknown) => {
      if (finished) return;
      finished = true;
      cleanup();
      resolve(value);
    };

    const fail = (error: unknown) => {
      if (finished) return;
      finished = true;
      cleanup();
      reject(error);
    };

    const handleCancel = () => {
      finish({
        ok: true,
        canceled: true,
      });
    };

    const handleChange = async () => {
      try {
        const files = Array.from(input.files ?? []);
        if (!files.length) {
          handleCancel();
          return;
        }

        const contents = await Promise.all(files.map((file) => file.text()));
        const filePaths = files.map((file) => {
          const relativePath =
            (file as File & { webkitRelativePath?: string }).webkitRelativePath ||
            file.name;
          return relativePath || file.name;
        });
        const directoryPath = directory
          ? filePaths[0]?.split("/")[0] || filePaths[0]?.split("\\")[0] || ""
          : "";

        finish({
          ok: true,
          canceled: false,
          directoryPath,
          fileCount: files.length,
          filePaths,
          contents,
        });
      } catch (error) {
        fail(error);
      }
    };

    input.addEventListener("change", handleChange);
    input.addEventListener("cancel", handleCancel as EventListener);
    input.click();
  });
}

async function exportAccountsViaBrowser(
  options: RequestOptions = {}
): Promise<unknown> {
  if (typeof document === "undefined") {
    throw new Error("当前环境不支持浏览器导出");
  }

  const payload =
    asRecord(await postWebRpc<unknown>("account/exportData", {}, options)) ?? {};
  const files = Array.isArray(payload.files)
    ? payload.files
        .map((item) => asRecord(item))
        .filter((item): item is Record<string, unknown> => item !== null)
    : [];

  for (const item of files) {
    const fileName =
      typeof item.fileName === "string" && item.fileName.trim()
        ? item.fileName.trim()
        : "account.json";
    const content = typeof item.content === "string" ? item.content : "";
    const blob = new Blob([content], {
      type: "application/json;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = fileName;
    anchor.style.display = "none";
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  return {
    ok: true,
    canceled: false,
    exported:
      typeof payload.exported === "number" ? payload.exported : files.length,
    outputDir: "browser-download",
  };
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
