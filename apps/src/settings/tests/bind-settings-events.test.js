import test from "node:test";
import assert from "node:assert/strict";

import { bindSettingsEvents } from "../bind-settings-events.js";

class FakeElement {
  constructor() {
    this.dataset = {};
    this.handlers = new Map();
    this.checked = false;
    this.value = "";
    this.clickCount = 0;
  }

  addEventListener(type, handler) {
    if (!this.handlers.has(type)) {
      this.handlers.set(type, []);
    }
    this.handlers.get(type).push(handler);
  }

  dispatch(type, event = {}) {
    for (const handler of this.handlers.get(type) || []) {
      handler(event);
    }
  }

  click() {
    this.clickCount += 1;
  }
}

function createContext(overrides = {}) {
  return {
    dom: {},
    showToast: () => {},
    withButtonBusy: async (_element, _label, action) => action(),
    normalizeErrorMessage: (err) => String(err?.message || err || ""),
    saveAppSettingsPatch: async () => ({}),
    handleCheckUpdateClick: () => {},
    isTauriRuntime: () => false,
    readUpdateAutoCheckSetting: () => false,
    saveUpdateAutoCheckSetting: () => {},
    readCloseToTrayOnCloseSetting: () => false,
    saveCloseToTrayOnCloseSetting: () => {},
    setCloseToTrayOnCloseToggle: () => {},
    applyCloseToTrayOnCloseSetting: async () => false,
    readLightweightModeOnCloseToTraySetting: () => false,
    saveLightweightModeOnCloseToTraySetting: () => {},
    setLightweightModeOnCloseToTrayToggle: () => {},
    syncLightweightModeOnCloseToTrayAvailability: () => {},
    applyLightweightModeOnCloseToTraySetting: async () => false,
    readRouteStrategySetting: () => "balanced",
    normalizeRouteStrategy: (value) => value || "balanced",
    saveRouteStrategySetting: () => {},
    setRouteStrategySelect: () => {},
    applyRouteStrategyToService: async () => true,
    routeStrategyLabel: (value) => value,
    readServiceListenModeSetting: () => "loopback",
    normalizeServiceListenMode: (value) => value || "loopback",
    setServiceListenModeSelect: () => {},
    setServiceListenModeHint: () => {},
    buildServiceListenModeHint: () => "",
    applyServiceListenModeToService: async () => true,
    readCpaNoCookieHeaderModeSetting: () => false,
    saveCpaNoCookieHeaderModeSetting: () => {},
    setCpaNoCookieHeaderModeToggle: () => {},
    normalizeCpaNoCookieHeaderMode: (value) => Boolean(value),
    applyCpaNoCookieHeaderModeToService: async () => true,
    readUpstreamProxyUrlSetting: () => "",
    saveUpstreamProxyUrlSetting: () => {},
    setUpstreamProxyInput: () => {},
    setUpstreamProxyHint: () => {},
    normalizeUpstreamProxyUrl: (value) => String(value || ""),
    applyUpstreamProxyToService: async () => true,
    upstreamProxyHintText: "",
    readGatewayTransportSetting: () => ({
      sseKeepaliveIntervalMs: 15000,
      upstreamStreamTimeoutMs: 1800000,
    }),
    readGatewayTransportForm: () => ({
      ok: true,
      settings: {
        sseKeepaliveIntervalMs: 16000,
        upstreamStreamTimeoutMs: 0,
      },
    }),
    saveGatewayTransportSetting: () => {},
    setGatewayTransportForm: () => {},
    normalizeGatewayTransportSettings: (value) => value || {},
    setGatewayTransportHint: () => {},
    applyGatewayTransportToService: async () => true,
    gatewayTransportHintText: "",
    readBackgroundTasksSetting: () => ({}),
    readBackgroundTasksForm: () => ({ ok: true, settings: {} }),
    saveBackgroundTasksSetting: () => {},
    setBackgroundTasksForm: () => {},
    normalizeBackgroundTasksSettings: (value) => value || {},
    updateBackgroundTasksHint: () => {},
    applyBackgroundTasksToService: async () => true,
    backgroundTasksRestartKeysDefault: [],
    getEnvOverrideSelectedKey: () => "",
    findEnvOverrideCatalogItem: () => null,
    setEnvOverridesHint: () => {},
    readEnvOverridesSetting: () => ({}),
    buildEnvOverrideHint: () => "",
    normalizeEnvOverrides: (value) => value || {},
    normalizeEnvOverrideCatalog: (value) => value || [],
    saveEnvOverridesSetting: () => {},
    renderEnvOverrideEditor: () => {},
    persistServiceAddrInput: async () => true,
    uiLowTransparencyToggleId: "lowTransparencyToggle",
    readLowTransparencySetting: () => false,
    saveLowTransparencySetting: () => {},
    applyLowTransparencySetting: () => {},
    syncWebAccessPasswordInputs: () => {},
    saveWebAccessPassword: async () => true,
    clearWebAccessPassword: async () => true,
    openWebSecurityModal: () => {},
    closeWebSecurityModal: () => {},
    ...overrides,
  };
}

test("bindSettingsEvents binds auto-check toggle once and persists the new value", async () => {
  const autoCheckUpdate = new FakeElement();
  autoCheckUpdate.checked = true;
  const calls = [];

  const context = createContext({
    dom: { autoCheckUpdate },
    saveUpdateAutoCheckSetting: (value) => {
      calls.push(["save", value]);
    },
    saveAppSettingsPatch: async (patch) => {
      calls.push(["patch", patch]);
      return patch;
    },
  });

  bindSettingsEvents(context);
  bindSettingsEvents(context);

  assert.equal(autoCheckUpdate.dataset.bound, "1");
  assert.equal((autoCheckUpdate.handlers.get("change") || []).length, 1);

  autoCheckUpdate.dispatch("change");
  await Promise.resolve();

  assert.deepEqual(calls, [
    ["save", true],
    ["patch", { updateAutoCheck: true }],
  ]);
});

test("bindSettingsEvents lets env override input submit on Enter", () => {
  const envOverrideValueInput = new FakeElement();
  const envOverridesSave = new FakeElement();
  let prevented = false;

  bindSettingsEvents(createContext({
    dom: {
      envOverrideValueInput,
      envOverridesSave,
    },
  }));

  envOverrideValueInput.dispatch("keydown", {
    key: "Enter",
    preventDefault() {
      prevented = true;
    },
  });

  assert.equal(prevented, true);
  assert.equal(envOverridesSave.clickCount, 1);
});

test("bindSettingsEvents saves gateway transport settings through app settings API", async () => {
  const gatewayTransportSave = new FakeElement();
  const gatewayTransportSseKeepaliveIntervalMs = new FakeElement();
  const gatewayTransportUpstreamStreamTimeoutMs = new FakeElement();
  gatewayTransportSseKeepaliveIntervalMs.value = "16000";
  gatewayTransportUpstreamStreamTimeoutMs.value = "0";
  const calls = [];

  bindSettingsEvents(createContext({
    dom: {
      gatewayTransportSave,
      gatewayTransportSseKeepaliveIntervalMs,
      gatewayTransportUpstreamStreamTimeoutMs,
    },
    saveGatewayTransportSetting: (value) => {
      calls.push(["save", value]);
    },
    setGatewayTransportForm: (value) => {
      calls.push(["form", value]);
    },
    setGatewayTransportHint: (value) => {
      calls.push(["hint", value]);
    },
    saveAppSettingsPatch: async (patch) => {
      calls.push(["patch", patch]);
      return patch;
    },
  }));

  gatewayTransportSave.dispatch("click");
  await Promise.resolve();
  await Promise.resolve();

  assert.deepEqual(calls, [
    ["save", { sseKeepaliveIntervalMs: 16000, upstreamStreamTimeoutMs: 0 }],
    ["form", { sseKeepaliveIntervalMs: 16000, upstreamStreamTimeoutMs: 0 }],
    ["patch", { sseKeepaliveIntervalMs: 16000, upstreamStreamTimeoutMs: 0 }],
    ["save", { sseKeepaliveIntervalMs: 16000, upstreamStreamTimeoutMs: 0 }],
    ["form", { sseKeepaliveIntervalMs: 16000, upstreamStreamTimeoutMs: 0 }],
    ["hint", ""],
  ]);
});
