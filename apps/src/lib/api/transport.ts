import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { fetchWithRetry, runWithControl, RequestOptions } from "../utils/request";
import { useAppStore } from "../store/useAppStore";

const WEB_RPC_METHOD_MAP = {
  app_settings_get: "appSettings/get",
  app_settings_set: "appSettings/set",
} as const;

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

function buildWebRpcParams(
  method: keyof typeof WEB_RPC_METHOD_MAP,
  params?: Record<string, unknown>
): Record<string, unknown> {
  if (method === "app_settings_set") {
    return asRecord(asRecord(params)?.patch) ?? {};
  }
  return params ?? {};
}

async function invokeWebRpc<T>(
  method: keyof typeof WEB_RPC_METHOD_MAP,
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  const rpcMethod = WEB_RPC_METHOD_MAP[method];
  const response = await fetchWithRetry(
    "/api/rpc",
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        jsonrpc: "2.0",
        id: Date.now(),
        method: rpcMethod,
        params: buildWebRpcParams(method, params),
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
  params?: Record<string, unknown>,
  options: RequestOptions = {}
): Promise<T> {
  if (!isTauriRuntime()) {
    if (method in WEB_RPC_METHOD_MAP) {
      return invokeWebRpc(
        method as keyof typeof WEB_RPC_METHOD_MAP,
        params,
        options
      );
    }
    throw new Error("当前操作仅支持桌面端");
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
