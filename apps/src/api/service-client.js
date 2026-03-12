import {
  invoke,
  isTauriRuntime,
  rpcInvoke,
  withAddr,
} from "./transport.js";

export async function serviceStart(addr) {
  if (!isTauriRuntime()) {
    throw new Error("浏览器模式不支持启动/停止服务，请手动启动 codexmanager-service");
  }
  return invoke("service_start", { addr });
}

export async function serviceStop() {
  if (!isTauriRuntime()) {
    throw new Error("浏览器模式不支持启动/停止服务，请手动停止 codexmanager-service");
  }
  return invoke("service_stop", {});
}

export async function serviceInitialize() {
  if (!isTauriRuntime()) {
    return rpcInvoke("initialize");
  }
  return invoke("service_initialize", withAddr());
}

export async function serviceStartupSnapshot(options = {}) {
  const requestLogLimit = Number(options && options.requestLogLimit);
  const payload = Number.isFinite(requestLogLimit) && requestLogLimit > 0
    ? { requestLogLimit: Math.trunc(requestLogLimit) }
    : undefined;
  if (!isTauriRuntime()) {
    return rpcInvoke("startup/snapshot", payload);
  }
  return invoke("service_startup_snapshot", payload ? withAddr(payload) : withAddr());
}

export async function serviceListenConfigGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("service/listenConfig/get");
  }
  return invoke("service_listen_config_get", {});
}

export async function serviceListenConfigSet(mode) {
  const normalized = mode == null ? "" : String(mode);
  if (!isTauriRuntime()) {
    return rpcInvoke("service/listenConfig/set", { mode: normalized });
  }
  return invoke("service_listen_config_set", { mode: normalized });
}

export async function serviceRequestLogList(query, limit, options = {}) {
  if (!isTauriRuntime()) {
    return rpcInvoke("requestlog/list", { query, limit }, options);
  }
  return invoke("service_requestlog_list", withAddr({ query, limit }));
}

export async function serviceRequestLogClear() {
  if (!isTauriRuntime()) {
    return rpcInvoke("requestlog/clear");
  }
  return invoke("service_requestlog_clear", withAddr());
}

export async function serviceRequestLogTodaySummary() {
  if (!isTauriRuntime()) {
    return rpcInvoke("requestlog/today_summary");
  }
  return invoke("service_requestlog_today_summary", withAddr());
}

export async function serviceGatewayRouteStrategyGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/routeStrategy/get");
  }
  return invoke("service_gateway_route_strategy_get", withAddr());
}

export async function serviceGatewayRouteStrategySet(strategy) {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/routeStrategy/set", { strategy });
  }
  return invoke("service_gateway_route_strategy_set", withAddr({ strategy }));
}

export async function serviceGatewayManualAccountGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/manualAccount/get");
  }
  return invoke("service_gateway_manual_account_get", withAddr());
}

export async function serviceGatewayManualAccountSet(accountId) {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/manualAccount/set", { accountId });
  }
  return invoke("service_gateway_manual_account_set", withAddr({ accountId }));
}

export async function serviceGatewayManualAccountClear() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/manualAccount/clear");
  }
  return invoke("service_gateway_manual_account_clear", withAddr());
}

export async function serviceGatewayHeaderPolicyGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/headerPolicy/get");
  }
  return invoke("service_gateway_header_policy_get", withAddr());
}

export async function serviceGatewayHeaderPolicySet(cpaNoCookieHeaderModeEnabled) {
  const enabled = Boolean(cpaNoCookieHeaderModeEnabled);
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/headerPolicy/set", { cpaNoCookieHeaderModeEnabled: enabled });
  }
  return invoke(
    "service_gateway_header_policy_set",
    withAddr({ cpaNoCookieHeaderModeEnabled: enabled }),
  );
}

export async function serviceGatewayBackgroundTasksGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/backgroundTasks/get");
  }
  return invoke("service_gateway_background_tasks_get", withAddr());
}

export async function serviceGatewayBackgroundTasksSet(settings = {}) {
  const payload = settings && typeof settings === "object" ? settings : {};
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/backgroundTasks/set", payload);
  }
  return invoke("service_gateway_background_tasks_set", withAddr(payload));
}

export async function serviceGatewayUpstreamProxyGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/upstreamProxy/get");
  }
  return invoke("service_gateway_upstream_proxy_get", withAddr());
}

export async function serviceGatewayUpstreamProxySet(proxyUrl) {
  const normalized = proxyUrl == null ? null : String(proxyUrl);
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/upstreamProxy/set", { proxyUrl: normalized });
  }
  return invoke("service_gateway_upstream_proxy_set", withAddr({ proxyUrl: normalized }));
}

export async function serviceGatewayTransportGet() {
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/transport/get");
  }
  return invoke("service_gateway_transport_get", withAddr());
}

export async function serviceGatewayTransportSet(settings = {}) {
  const payload = settings && typeof settings === "object" ? settings : {};
  if (!isTauriRuntime()) {
    return rpcInvoke("gateway/transport/set", payload);
  }
  return invoke("service_gateway_transport_set", withAddr(payload));
}
