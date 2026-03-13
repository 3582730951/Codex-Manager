import {
  BACKGROUND_TASKS_RESTART_KEYS_DEFAULT,
  BACKGROUND_TASKS_RESTART_KEY_LABELS,
  GATEWAY_TRANSPORT_HINT_TEXT,
  ROUTE_STRATEGY_BALANCED,
  SERVICE_LISTEN_MODE_ALL_INTERFACES,
  UPSTREAM_PROXY_HINT_TEXT,
  defaultNormalizeAddr,
} from "./shared.js";

export function createRuntimeSettingsController(deps = {}) {
  const {
    dom = {},
    state = {},
    showToast = () => {},
    normalizeErrorMessage = (err) => String(err?.message || err || ""),
    normalizeAddr = defaultNormalizeAddr,
    saveAppSettingsPatch,
    patchAppSettingsSnapshot,
    getAppSettingsSnapshot,
    normalizeServiceListenMode,
    serviceListenModeLabel,
    normalizeRouteStrategy,
    normalizeCpaNoCookieHeaderMode,
    normalizeUpstreamProxyUrl,
    normalizeGatewayTransportSettings,
    normalizeBackgroundTasksSettings,
  } = deps;

  let serviceListenModeSyncInFlight = null;

  function buildServiceListenModeHint(mode, requiresRestart = true) {
    const normalized = normalizeServiceListenMode(mode);
    const suffix = normalized === SERVICE_LISTEN_MODE_ALL_INTERFACES
      ? "局域网访问请使用本机实际 IP。"
      : "外部设备将无法直接访问。";
    if (requiresRestart) {
      return `已保存为${serviceListenModeLabel(normalized)}，重启服务后生效；${suffix}`;
    }
    return `当前为${serviceListenModeLabel(normalized)}；${suffix}`;
  }

  function setServiceListenModeSelect(mode) {
    if (!dom.serviceListenModeSelect) {
      return;
    }
    dom.serviceListenModeSelect.value = normalizeServiceListenMode(mode);
  }

  function setServiceListenModeHint(message) {
    if (!dom.serviceListenModeHint) {
      return;
    }
    dom.serviceListenModeHint.textContent = String(message || "").trim()
      || "保存后重启服务生效；局域网访问请使用本机实际 IP。";
  }

  function readServiceListenModeSetting() {
    return normalizeServiceListenMode(getAppSettingsSnapshot().serviceListenMode);
  }

  function initServiceListenModeSetting() {
    const mode = readServiceListenModeSetting();
    setServiceListenModeSelect(mode);
    setServiceListenModeHint(buildServiceListenModeHint(mode, true));
  }

  async function applyServiceListenModeToService(mode, { silent = true } = {}) {
    const normalized = normalizeServiceListenMode(mode);
    if (serviceListenModeSyncInFlight) {
      return serviceListenModeSyncInFlight;
    }
    serviceListenModeSyncInFlight = (async () => {
      const settings = await saveAppSettingsPatch({
        serviceListenMode: normalized,
      });
      const resolved = {
        mode: normalizeServiceListenMode(settings.serviceListenMode),
        requiresRestart: true,
      };
      setServiceListenModeSelect(resolved.mode);
      setServiceListenModeHint(buildServiceListenModeHint(resolved.mode, resolved.requiresRestart));
      if (!silent) {
        showToast(`监听模式已保存为${serviceListenModeLabel(resolved.mode)}，重启服务后生效`);
      }
      return true;
    })();

    try {
      return await serviceListenModeSyncInFlight;
    } catch (err) {
      if (!silent) {
        showToast(`保存失败：${normalizeErrorMessage(err)}`, "error");
        setServiceListenModeHint(`保存失败：${normalizeErrorMessage(err)}`);
      }
      return false;
    } finally {
      serviceListenModeSyncInFlight = null;
    }
  }

  async function syncServiceListenModeOnStartup() {
    initServiceListenModeSetting();
  }

  function updateRouteStrategyHint(strategy) {
    if (!dom.routeStrategyHint) {
      return;
    }
    let hintText = "按账号顺序优先请求，优先使用可用账号（不可用账号不会参与选路）。";
    if (normalizeRouteStrategy(strategy) === ROUTE_STRATEGY_BALANCED) {
      hintText = "按密钥 + 模型在所有可用账号间严格均衡轮询；不可用账号不会参与选路。";
    }
    dom.routeStrategyHint.title = hintText;
    dom.routeStrategyHint.setAttribute("aria-label", `网关选路策略说明：${hintText}`);
  }

  function readRouteStrategySetting() {
    return normalizeRouteStrategy(getAppSettingsSnapshot().routeStrategy);
  }

  function saveRouteStrategySetting(strategy) {
    patchAppSettingsSnapshot({
      routeStrategy: normalizeRouteStrategy(strategy),
    });
  }

  function setRouteStrategySelect(strategy) {
    const normalized = normalizeRouteStrategy(strategy);
    if (dom.routeStrategySelect) {
      dom.routeStrategySelect.value = normalized;
    }
    updateRouteStrategyHint(normalized);
  }

  function initRouteStrategySetting() {
    const mode = readRouteStrategySetting();
    setRouteStrategySelect(mode);
  }

  function readCpaNoCookieHeaderModeSetting() {
    return normalizeCpaNoCookieHeaderMode(getAppSettingsSnapshot().cpaNoCookieHeaderModeEnabled);
  }

  function saveCpaNoCookieHeaderModeSetting(enabled) {
    patchAppSettingsSnapshot({
      cpaNoCookieHeaderModeEnabled: normalizeCpaNoCookieHeaderMode(enabled),
    });
  }

  function setCpaNoCookieHeaderModeToggle(enabled) {
    if (dom.cpaNoCookieHeaderMode) {
      dom.cpaNoCookieHeaderMode.checked = Boolean(enabled);
    }
  }

  function initCpaNoCookieHeaderModeSetting() {
    const enabled = readCpaNoCookieHeaderModeSetting();
    setCpaNoCookieHeaderModeToggle(enabled);
  }

  function readUpstreamProxyUrlSetting() {
    return normalizeUpstreamProxyUrl(getAppSettingsSnapshot().upstreamProxyUrl);
  }

  function saveUpstreamProxyUrlSetting(value) {
    patchAppSettingsSnapshot({
      upstreamProxyUrl: normalizeUpstreamProxyUrl(value),
    });
  }

  function setUpstreamProxyInput(value) {
    if (!dom.upstreamProxyUrlInput) {
      return;
    }
    dom.upstreamProxyUrlInput.value = normalizeUpstreamProxyUrl(value);
  }

  function setUpstreamProxyHint(message) {
    if (!dom.upstreamProxyHint) {
      return;
    }
    dom.upstreamProxyHint.textContent = message;
  }

  function initUpstreamProxySetting() {
    const proxyUrl = readUpstreamProxyUrlSetting();
    setUpstreamProxyInput(proxyUrl);
    setUpstreamProxyHint(UPSTREAM_PROXY_HINT_TEXT);
  }

  function readGatewayTransportSetting() {
    return normalizeGatewayTransportSettings({
      sseKeepaliveIntervalMs: getAppSettingsSnapshot().sseKeepaliveIntervalMs,
      upstreamStreamTimeoutMs: getAppSettingsSnapshot().upstreamStreamTimeoutMs,
    });
  }

  function saveGatewayTransportSetting(settings) {
    patchAppSettingsSnapshot(normalizeGatewayTransportSettings(settings));
  }

  function setGatewayTransportForm(settings) {
    const normalized = normalizeGatewayTransportSettings(settings);
    if (dom.gatewayTransportSseKeepaliveIntervalMs) {
      dom.gatewayTransportSseKeepaliveIntervalMs.value = String(normalized.sseKeepaliveIntervalMs);
    }
    if (dom.gatewayTransportUpstreamStreamTimeoutMs) {
      dom.gatewayTransportUpstreamStreamTimeoutMs.value = String(normalized.upstreamStreamTimeoutMs);
    }
  }

  function readGatewayTransportForm() {
    const keepaliveRaw = dom.gatewayTransportSseKeepaliveIntervalMs
      ? String(dom.gatewayTransportSseKeepaliveIntervalMs.value || "").trim()
      : "";
    const keepalive = Number(keepaliveRaw);
    if (!Number.isFinite(keepalive) || keepalive <= 0 || Math.floor(keepalive) !== keepalive) {
      return { ok: false, error: "SSE 保活间隔需填写正整数" };
    }

    const timeoutRaw = dom.gatewayTransportUpstreamStreamTimeoutMs
      ? String(dom.gatewayTransportUpstreamStreamTimeoutMs.value || "").trim()
      : "";
    const timeout = Number(timeoutRaw);
    if (!Number.isFinite(timeout) || timeout < 0 || Math.floor(timeout) !== timeout) {
      return { ok: false, error: "上游流式超时需填写非负整数" };
    }

    return {
      ok: true,
      settings: normalizeGatewayTransportSettings({
        sseKeepaliveIntervalMs: keepalive,
        upstreamStreamTimeoutMs: timeout,
      }),
    };
  }

  function setGatewayTransportHint(message) {
    if (!dom.gatewayTransportHint) {
      return;
    }
    dom.gatewayTransportHint.textContent = String(message || "").trim() || GATEWAY_TRANSPORT_HINT_TEXT;
  }

  function initGatewayTransportSetting() {
    const settings = readGatewayTransportSetting();
    setGatewayTransportForm(settings);
    setGatewayTransportHint(GATEWAY_TRANSPORT_HINT_TEXT);
  }

  function readBackgroundTasksSetting() {
    return normalizeBackgroundTasksSettings(getAppSettingsSnapshot().backgroundTasks);
  }

  function saveBackgroundTasksSetting(settings) {
    patchAppSettingsSnapshot({
      backgroundTasks: normalizeBackgroundTasksSettings(settings),
    });
  }

  function setBackgroundTasksForm(settings) {
    const normalized = normalizeBackgroundTasksSettings(settings);
    if (dom.backgroundUsagePollingEnabled) {
      dom.backgroundUsagePollingEnabled.checked = normalized.usagePollingEnabled;
    }
    if (dom.backgroundUsagePollIntervalSecs) {
      dom.backgroundUsagePollIntervalSecs.value = String(normalized.usagePollIntervalSecs);
    }
    if (dom.backgroundGatewayKeepaliveEnabled) {
      dom.backgroundGatewayKeepaliveEnabled.checked = normalized.gatewayKeepaliveEnabled;
    }
    if (dom.backgroundGatewayKeepaliveIntervalSecs) {
      dom.backgroundGatewayKeepaliveIntervalSecs.value = String(normalized.gatewayKeepaliveIntervalSecs);
    }
    if (dom.backgroundTokenRefreshPollingEnabled) {
      dom.backgroundTokenRefreshPollingEnabled.checked = normalized.tokenRefreshPollingEnabled;
    }
    if (dom.backgroundTokenRefreshPollIntervalSecs) {
      dom.backgroundTokenRefreshPollIntervalSecs.value = String(normalized.tokenRefreshPollIntervalSecs);
    }
    if (dom.backgroundUsageRefreshWorkers) {
      dom.backgroundUsageRefreshWorkers.value = String(normalized.usageRefreshWorkers);
    }
    if (dom.backgroundHttpWorkerFactor) {
      dom.backgroundHttpWorkerFactor.value = String(normalized.httpWorkerFactor);
    }
    if (dom.backgroundHttpWorkerMin) {
      dom.backgroundHttpWorkerMin.value = String(normalized.httpWorkerMin);
    }
    if (dom.backgroundHttpStreamWorkerFactor) {
      dom.backgroundHttpStreamWorkerFactor.value = String(normalized.httpStreamWorkerFactor);
    }
    if (dom.backgroundHttpStreamWorkerMin) {
      dom.backgroundHttpStreamWorkerMin.value = String(normalized.httpStreamWorkerMin);
    }
  }

  function readBackgroundTasksForm() {
    const integerFields = [
      ["usagePollIntervalSecs", dom.backgroundUsagePollIntervalSecs, "用量轮询间隔"],
      ["gatewayKeepaliveIntervalSecs", dom.backgroundGatewayKeepaliveIntervalSecs, "网关保活间隔"],
      ["tokenRefreshPollIntervalSecs", dom.backgroundTokenRefreshPollIntervalSecs, "令牌刷新间隔"],
      ["usageRefreshWorkers", dom.backgroundUsageRefreshWorkers, "用量刷新线程数"],
      ["httpWorkerFactor", dom.backgroundHttpWorkerFactor, "普通请求线程因子"],
      ["httpWorkerMin", dom.backgroundHttpWorkerMin, "普通请求最小线程数"],
      ["httpStreamWorkerFactor", dom.backgroundHttpStreamWorkerFactor, "流式请求线程因子"],
      ["httpStreamWorkerMin", dom.backgroundHttpStreamWorkerMin, "流式请求最小线程数"],
    ];
    const numbers = {};
    for (const [key, input, label] of integerFields) {
      const raw = input ? String(input.value || "").trim() : "";
      const parsed = Number(raw);
      if (!Number.isFinite(parsed) || parsed <= 0 || Math.floor(parsed) !== parsed) {
        return { ok: false, error: `${label} 需填写正整数` };
      }
      numbers[key] = parsed;
    }

    const currentSettings = readBackgroundTasksSetting();
    return {
      ok: true,
      settings: normalizeBackgroundTasksSettings({
        usagePollingEnabled: dom.backgroundUsagePollingEnabled
          ? Boolean(dom.backgroundUsagePollingEnabled.checked)
          : currentSettings.usagePollingEnabled,
        usagePollIntervalSecs: numbers.usagePollIntervalSecs,
        gatewayKeepaliveEnabled: dom.backgroundGatewayKeepaliveEnabled
          ? Boolean(dom.backgroundGatewayKeepaliveEnabled.checked)
          : currentSettings.gatewayKeepaliveEnabled,
        gatewayKeepaliveIntervalSecs: numbers.gatewayKeepaliveIntervalSecs,
        tokenRefreshPollingEnabled: dom.backgroundTokenRefreshPollingEnabled
          ? Boolean(dom.backgroundTokenRefreshPollingEnabled.checked)
          : currentSettings.tokenRefreshPollingEnabled,
        tokenRefreshPollIntervalSecs: numbers.tokenRefreshPollIntervalSecs,
        usageRefreshWorkers: numbers.usageRefreshWorkers,
        httpWorkerFactor: numbers.httpWorkerFactor,
        httpWorkerMin: numbers.httpWorkerMin,
        httpStreamWorkerFactor: numbers.httpStreamWorkerFactor,
        httpStreamWorkerMin: numbers.httpStreamWorkerMin,
      }),
    };
  }

  function updateBackgroundTasksHint(requiresRestartKeys) {
    if (!dom.backgroundTasksHint) {
      return;
    }
    const keys = Array.isArray(requiresRestartKeys) ? requiresRestartKeys : [];
    if (keys.length === 0) {
      dom.backgroundTasksHint.textContent = "保存后立即生效。";
      return;
    }
    const labels = keys.map((key) => BACKGROUND_TASKS_RESTART_KEY_LABELS[key] || key);
    dom.backgroundTasksHint.textContent = `已保存。以下参数需重启服务生效：${labels.join("、")}。`;
  }

  function initBackgroundTasksSetting() {
    const settings = readBackgroundTasksSetting();
    setBackgroundTasksForm(settings);
    updateBackgroundTasksHint(BACKGROUND_TASKS_RESTART_KEYS_DEFAULT);
  }

  async function persistServiceAddrInput({ silent = true } = {}) {
    if (!dom.serviceAddrInput) {
      return false;
    }
    let normalized = "";
    try {
      normalized = normalizeAddr(dom.serviceAddrInput.value || "");
    } catch (err) {
      if (!silent) {
        showToast(`服务地址格式不正确：${normalizeErrorMessage(err)}`, "error");
      }
      return false;
    }
    dom.serviceAddrInput.value = normalized;
    state.serviceAddr = normalized;
    patchAppSettingsSnapshot({ serviceAddr: normalized });
    try {
      await saveAppSettingsPatch({
        serviceAddr: normalized,
      });
      return true;
    } catch (err) {
      if (!silent) {
        showToast(`保存服务地址失败：${normalizeErrorMessage(err)}`, "error");
      }
      return false;
    }
  }

  return {
    buildServiceListenModeHint,
    setServiceListenModeSelect,
    setServiceListenModeHint,
    readServiceListenModeSetting,
    initServiceListenModeSetting,
    applyServiceListenModeToService,
    syncServiceListenModeOnStartup,
    readRouteStrategySetting,
    saveRouteStrategySetting,
    setRouteStrategySelect,
    initRouteStrategySetting,
    readCpaNoCookieHeaderModeSetting,
    saveCpaNoCookieHeaderModeSetting,
    setCpaNoCookieHeaderModeToggle,
    initCpaNoCookieHeaderModeSetting,
    readUpstreamProxyUrlSetting,
    saveUpstreamProxyUrlSetting,
    setUpstreamProxyInput,
    setUpstreamProxyHint,
    initUpstreamProxySetting,
    readGatewayTransportSetting,
    saveGatewayTransportSetting,
    setGatewayTransportForm,
    readGatewayTransportForm,
    setGatewayTransportHint,
    initGatewayTransportSetting,
    readBackgroundTasksSetting,
    saveBackgroundTasksSetting,
    setBackgroundTasksForm,
    readBackgroundTasksForm,
    updateBackgroundTasksHint,
    initBackgroundTasksSetting,
    persistServiceAddrInput,
    upstreamProxyHintText: UPSTREAM_PROXY_HINT_TEXT,
    gatewayTransportHintText: GATEWAY_TRANSPORT_HINT_TEXT,
    backgroundTasksRestartKeysDefault: BACKGROUND_TASKS_RESTART_KEYS_DEFAULT,
  };
}
