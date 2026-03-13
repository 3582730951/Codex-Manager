import test from "node:test";
import assert from "node:assert/strict";

import { createSettingsServiceSync } from "../service-sync.js";

function createServiceSync(overrides = {}) {
  const calls = {
    route: [],
    header: [],
    proxy: [],
    transport: [],
    background: [],
    toasts: [],
    toggles: 0,
  };
  const state = overrides.state || { serviceProbeId: 1 };
  let routeStrategyValue = "balanced";
  let cpaNoCookieHeaderModeValue = true;
  let upstreamProxyValue = "http://127.0.0.1:7890";
  let gatewayTransportValue = {
    sseKeepaliveIntervalMs: 15000,
    upstreamStreamTimeoutMs: 1800000,
  };
  let backgroundTasksValue = {
    usagePollingEnabled: true,
    usagePollIntervalSecs: 30,
    gatewayKeepaliveEnabled: false,
    gatewayKeepaliveIntervalSecs: 60,
    tokenRefreshPollingEnabled: false,
    tokenRefreshPollIntervalSecs: 60,
    usageRefreshWorkers: 4,
    httpWorkerFactor: 4,
    httpWorkerMin: 4,
    httpStreamWorkerFactor: 2,
    httpStreamWorkerMin: 2,
  };
  const sinks = {
    routeSaved: [],
    routeSelected: [],
    cpaSaved: [],
    cpaSelected: [],
    proxySaved: [],
    proxyInput: [],
    proxyHint: [],
    transportSaved: [],
    transportForm: [],
    transportHint: [],
    backgroundSaved: [],
    backgroundForm: [],
    backgroundHint: [],
  };

  const sync = createSettingsServiceSync({
    state,
    showToast: (message, level = "info") => {
      calls.toasts.push({ message, level });
    },
    normalizeErrorMessage: (err) => String(err?.message || err || ""),
    isTauriRuntime: () => true,
    ensureConnected: async () => true,
    serviceLifecycle: {
      updateServiceToggle: () => {
        calls.toggles += 1;
      },
    },
    serviceGatewayRouteStrategySet: async (strategy) => {
      calls.route.push(strategy);
      return { strategy };
    },
    serviceGatewayHeaderPolicySet: async (enabled) => {
      calls.header.push(enabled);
      return { enabled };
    },
    serviceGatewayUpstreamProxySet: async (proxyUrl) => {
      calls.proxy.push(proxyUrl);
      return { proxyUrl };
    },
    serviceGatewayTransportSet: async (settings) => {
      calls.transport.push(settings);
      return settings;
    },
    serviceGatewayBackgroundTasksSet: async (settings) => {
      calls.background.push(settings);
      return {
        ...settings,
        requiresRestartKeys: ["usagePollingEnabled"],
      };
    },
    readRouteStrategySetting: () => routeStrategyValue,
    saveRouteStrategySetting: (value) => {
      routeStrategyValue = value;
      sinks.routeSaved.push(value);
    },
    setRouteStrategySelect: (value) => {
      sinks.routeSelected.push(value);
    },
    normalizeRouteStrategy: (value) => String(value || "balanced"),
    routeStrategyLabel: (value) => String(value || ""),
    readCpaNoCookieHeaderModeSetting: () => cpaNoCookieHeaderModeValue,
    saveCpaNoCookieHeaderModeSetting: (value) => {
      cpaNoCookieHeaderModeValue = Boolean(value);
      sinks.cpaSaved.push(Boolean(value));
    },
    setCpaNoCookieHeaderModeToggle: (value) => {
      sinks.cpaSelected.push(Boolean(value));
    },
    normalizeCpaNoCookieHeaderMode: (value) => Boolean(value),
    readUpstreamProxyUrlSetting: () => upstreamProxyValue,
    saveUpstreamProxyUrlSetting: (value) => {
      upstreamProxyValue = value == null ? "" : String(value);
      sinks.proxySaved.push(upstreamProxyValue);
    },
    setUpstreamProxyInput: (value) => {
      sinks.proxyInput.push(value == null ? "" : String(value));
    },
    setUpstreamProxyHint: (value) => {
      sinks.proxyHint.push(String(value || ""));
    },
    normalizeUpstreamProxyUrl: (value) => String(value || "").trim(),
    upstreamProxyHintText: "proxy hint",
    readGatewayTransportSetting: () => gatewayTransportValue,
    saveGatewayTransportSetting: (value) => {
      gatewayTransportValue = { ...value };
      sinks.transportSaved.push(value);
    },
    setGatewayTransportForm: (value) => {
      sinks.transportForm.push(value);
    },
    normalizeGatewayTransportSettings: (value = {}) => ({
      sseKeepaliveIntervalMs: Number(value.sseKeepaliveIntervalMs || 0),
      upstreamStreamTimeoutMs: Number(value.upstreamStreamTimeoutMs ?? 0),
    }),
    setGatewayTransportHint: (value) => {
      sinks.transportHint.push(String(value || ""));
    },
    gatewayTransportHintText: "transport hint",
    readBackgroundTasksSetting: () => backgroundTasksValue,
    saveBackgroundTasksSetting: (value) => {
      backgroundTasksValue = { ...value };
      sinks.backgroundSaved.push(value);
    },
    setBackgroundTasksForm: (value) => {
      sinks.backgroundForm.push(value);
    },
    normalizeBackgroundTasksSettings: (value = {}) => ({
      usagePollingEnabled: Boolean(value.usagePollingEnabled),
      usagePollIntervalSecs: Number(value.usagePollIntervalSecs || 0),
      gatewayKeepaliveEnabled: Boolean(value.gatewayKeepaliveEnabled),
      gatewayKeepaliveIntervalSecs: Number(value.gatewayKeepaliveIntervalSecs || 0),
      tokenRefreshPollingEnabled: Boolean(value.tokenRefreshPollingEnabled),
      tokenRefreshPollIntervalSecs: Number(value.tokenRefreshPollIntervalSecs || 0),
      usageRefreshWorkers: Number(value.usageRefreshWorkers || 0),
      httpWorkerFactor: Number(value.httpWorkerFactor || 0),
      httpWorkerMin: Number(value.httpWorkerMin || 0),
      httpStreamWorkerFactor: Number(value.httpStreamWorkerFactor || 0),
      httpStreamWorkerMin: Number(value.httpStreamWorkerMin || 0),
    }),
    updateBackgroundTasksHint: (value) => {
      sinks.backgroundHint.push(value);
    },
    backgroundTasksRestartKeysDefault: ["default"],
    ...overrides,
  });

  return {
    sync,
    calls,
    sinks,
    state,
  };
}

test("applyRouteStrategyToService saves resolved strategy and updates UI state", async () => {
  const { sync, calls, sinks } = createServiceSync({
    state: { serviceProbeId: 7 },
  });

  const ok = await sync.applyRouteStrategyToService("balanced", { silent: false });

  assert.equal(ok, true);
  assert.deepEqual(calls.route, ["balanced"]);
  assert.equal(calls.toggles, 1);
  assert.deepEqual(sinks.routeSaved, ["balanced"]);
  assert.deepEqual(sinks.routeSelected, ["balanced"]);
  assert.equal(calls.toasts.at(-1)?.message, "已切换为balanced");
});

test("syncRuntimeSettingsForCurrentProbe only resyncs when service probe changes", async () => {
  const { sync, calls, state, sinks } = createServiceSync({
    state: { serviceProbeId: 1 },
  });

  await sync.syncRuntimeSettingsForCurrentProbe();
  await sync.syncRuntimeSettingsForCurrentProbe();

  assert.equal(calls.route.length, 1);
  assert.equal(calls.header.length, 1);
  assert.equal(calls.proxy.length, 1);
  assert.equal(calls.transport.length, 1);
  assert.equal(calls.background.length, 1);

  state.serviceProbeId = 2;
  await sync.syncRuntimeSettingsForCurrentProbe();

  assert.equal(calls.route.length, 2);
  assert.equal(calls.header.length, 2);
  assert.equal(calls.proxy.length, 2);
  assert.equal(calls.transport.length, 2);
  assert.equal(calls.background.length, 2);
  assert.deepEqual(sinks.proxyHint, ["proxy hint", "proxy hint"]);
  assert.deepEqual(sinks.transportHint, ["transport hint", "transport hint"]);
  assert.equal(sinks.backgroundHint.length, 2);
});
