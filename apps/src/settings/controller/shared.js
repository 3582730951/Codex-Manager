import { appSettingsGet as defaultAppSettingsGet, appSettingsSet as defaultAppSettingsSet } from "../../api.js";
import { normalizeAddr as defaultNormalizeAddr } from "../../services/connection.js";
import {
  buildEnvOverrideDescription as defaultBuildEnvOverrideDescription,
  buildEnvOverrideOptionLabel as defaultBuildEnvOverrideOptionLabel,
  filterEnvOverrideCatalog as defaultFilterEnvOverrideCatalog,
  formatEnvOverrideDisplayValue as defaultFormatEnvOverrideDisplayValue,
  normalizeEnvOverrideCatalog as defaultNormalizeEnvOverrideCatalog,
  normalizeEnvOverrides as defaultNormalizeEnvOverrides,
  normalizeStringList as defaultNormalizeStringList,
} from "../../ui/env-overrides.js";
import { normalizeUpstreamProxyUrl as defaultNormalizeUpstreamProxyUrl } from "../../utils/upstream-proxy.js";

export const ROUTE_STRATEGY_ORDERED = "ordered";
export const ROUTE_STRATEGY_BALANCED = "balanced";
export const SERVICE_LISTEN_MODE_LOOPBACK = "loopback";
export const SERVICE_LISTEN_MODE_ALL_INTERFACES = "all_interfaces";
export const UI_LOW_TRANSPARENCY_BODY_CLASS = "cm-low-transparency";
export const UI_LOW_TRANSPARENCY_TOGGLE_ID = "lowTransparencyMode";
export const UI_LOW_TRANSPARENCY_CARD_ID = "settingsLowTransparencyCard";
export const UPSTREAM_PROXY_HINT_TEXT = "支持 http/https/socks5，留空直连，socks 会自动按 socks5h 处理。";
export const GATEWAY_TRANSPORT_HINT_TEXT = "SSE 保活建议 10000-30000 毫秒；上游流式超时填 0 表示不设上限。";

export const DEFAULT_BACKGROUND_TASKS_SETTINGS = {
  usagePollingEnabled: true,
  usagePollIntervalSecs: 600,
  gatewayKeepaliveEnabled: true,
  gatewayKeepaliveIntervalSecs: 180,
  tokenRefreshPollingEnabled: true,
  tokenRefreshPollIntervalSecs: 60,
  usageRefreshWorkers: 4,
  httpWorkerFactor: 4,
  httpWorkerMin: 8,
  httpStreamWorkerFactor: 1,
  httpStreamWorkerMin: 2,
};

export const DEFAULT_GATEWAY_TRANSPORT_SETTINGS = {
  sseKeepaliveIntervalMs: 15_000,
  upstreamStreamTimeoutMs: 1_800_000,
};

export const BACKGROUND_TASKS_RESTART_KEYS_DEFAULT = [
  "usageRefreshWorkers",
  "httpWorkerFactor",
  "httpWorkerMin",
  "httpStreamWorkerFactor",
  "httpStreamWorkerMin",
];

export const BACKGROUND_TASKS_RESTART_KEY_LABELS = {
  usageRefreshWorkers: "用量刷新并发线程数",
  httpWorkerFactor: "普通请求并发因子",
  httpWorkerMin: "普通请求最小并发",
  httpStreamWorkerFactor: "流式请求并发因子",
  httpStreamWorkerMin: "流式请求最小并发",
};

export function defaultNormalizeRouteStrategy(strategy) {
  const raw = String(strategy || "").trim().toLowerCase();
  if (["balanced", "round_robin", "round-robin", "rr"].includes(raw)) {
    return ROUTE_STRATEGY_BALANCED;
  }
  return ROUTE_STRATEGY_BALANCED;
}

export function defaultRouteStrategyLabel(strategy) {
  return defaultNormalizeRouteStrategy(strategy) === ROUTE_STRATEGY_BALANCED ? "均衡轮询" : "顺序优先";
}

export function defaultNormalizeServiceListenMode(value) {
  const raw = String(value || "").trim().toLowerCase();
  if (["all_interfaces", "all-interfaces", "all", "0.0.0.0"].includes(raw)) {
    return SERVICE_LISTEN_MODE_ALL_INTERFACES;
  }
  return SERVICE_LISTEN_MODE_LOOPBACK;
}

export function defaultServiceListenModeLabel(mode) {
  return defaultNormalizeServiceListenMode(mode) === SERVICE_LISTEN_MODE_ALL_INTERFACES
    ? "全部网卡（0.0.0.0）"
    : "仅本机（localhost / 127.0.0.1）";
}

export function defaultNormalizeCpaNoCookieHeaderMode(value) {
  if (typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    return value !== 0;
  }
  if (typeof value === "string") {
    const normalized = value.trim().toLowerCase();
    if (["1", "true", "yes", "on"].includes(normalized)) {
      return true;
    }
    if (["0", "false", "no", "off"].includes(normalized)) {
      return false;
    }
  }
  return false;
}

export function normalizeBooleanSetting(value, fallback = false) {
  if (value == null) {
    return Boolean(fallback);
  }
  if (typeof value === "boolean") {
    return value;
  }
  if (typeof value === "number") {
    return value !== 0;
  }
  if (typeof value === "string") {
    const normalized = value.trim().toLowerCase();
    if (["1", "true", "yes", "on"].includes(normalized)) {
      return true;
    }
    if (["0", "false", "no", "off"].includes(normalized)) {
      return false;
    }
  }
  return Boolean(fallback);
}

export function normalizePositiveInteger(value, fallback, min = 1) {
  const fallbackValue = Math.max(min, Math.floor(Number(fallback) || min));
  const numeric = Number(value);
  if (!Number.isFinite(numeric)) {
    return fallbackValue;
  }
  const intValue = Math.floor(numeric);
  if (intValue < min) {
    return min;
  }
  return intValue;
}

export function normalizeThemeSetting(value) {
  const normalized = String(value || "").trim().toLowerCase();
  return normalized || "tech";
}

export {
  defaultAppSettingsGet,
  defaultAppSettingsSet,
  defaultNormalizeAddr,
  defaultNormalizeUpstreamProxyUrl,
  defaultBuildEnvOverrideDescription,
  defaultBuildEnvOverrideOptionLabel,
  defaultFilterEnvOverrideCatalog,
  defaultFormatEnvOverrideDisplayValue,
  defaultNormalizeEnvOverrideCatalog,
  defaultNormalizeEnvOverrides,
  defaultNormalizeStringList,
};
