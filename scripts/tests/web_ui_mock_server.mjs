import http from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { access, readFile, stat } from "node:fs/promises";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const defaultWebRoot = path.join(repoRoot, "apps", "out");

function parseArgs(argv) {
  const options = {
    port: 49681,
    mode: "supported",
    webRoot: defaultWebRoot,
  };

  for (let index = 0; index < argv.length; index += 1) {
    const current = argv[index];
    const next = argv[index + 1];
    if (current === "--port" && next) {
      options.port = Number.parseInt(next, 10) || options.port;
      index += 1;
      continue;
    }
    if (current === "--mode" && next) {
      options.mode = String(next || "").trim() || options.mode;
      index += 1;
      continue;
    }
    if (current === "--web-root" && next) {
      options.webRoot = path.resolve(repoRoot, next);
      index += 1;
    }
  }

  return options;
}

function createDefaultSettings() {
  return {
    updateAutoCheck: true,
    closeToTrayOnClose: false,
    closeToTraySupported: false,
    lowTransparency: false,
    lightweightModeOnCloseToTray: false,
    webAccessPasswordConfigured: false,
    serviceAddr: "localhost:48760",
    serviceListenMode: "loopback",
    serviceListenModeOptions: ["loopback", "all_interfaces"],
    routeStrategy: "ordered",
    routeStrategyOptions: ["ordered", "balanced"],
    freeAccountMaxModel: "auto",
    freeAccountMaxModelOptions: [
      "auto",
      "gpt-5",
      "gpt-5-codex",
      "gpt-5-codex-mini",
      "gpt-5.1",
      "gpt-5.1-codex",
      "gpt-5.1-codex-max",
      "gpt-5.1-codex-mini",
      "gpt-5.2",
      "gpt-5.2-codex",
      "gpt-5.3-codex",
      "gpt-5.4",
    ],
    requestCompressionEnabled: true,
    gatewayOriginator: "codex_cli_rs",
    gatewayUserAgentVersion: "0.118.0",
    gatewayResidencyRequirement: "",
    gatewayResidencyRequirementOptions: ["", "us"],
    upstreamProxyUrl: "",
    upstreamStreamTimeoutMs: 1_800_000,
    sseKeepaliveIntervalMs: 15_000,
    backgroundTasks: {
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
    },
    envOverrides: {},
    envOverrideCatalog: [
      {
        key: "CODEXMANAGER_WEB_ADDR",
        label: "Web 地址",
        defaultValue: "localhost:48761",
        scope: "web",
        applyMode: "restart",
      },
    ],
    envOverrideReservedKeys: [],
    envOverrideUnsupportedKeys: [],
    theme: "tech",
    appearancePreset: "classic",
  };
}

function createState() {
  const nowSeconds = Math.floor(Date.now() / 1000);
  const accounts = [
    {
      id: "acc-demo-001",
      name: "demo-primary@example.com",
      groupName: "TEAM",
      sort: 1,
      status: "active",
      statusReason: "",
    },
  ];
  const usageItems = [
    {
      accountId: "acc-demo-001",
      availabilityStatus: "ok",
      usedPercent: 20,
      windowMinutes: 300,
      resetsAt: nowSeconds + 7_200,
      secondaryUsedPercent: 35,
      secondaryWindowMinutes: 10_080,
      secondaryResetsAt: nowSeconds + 86_400,
      creditsJson: "",
      capturedAt: nowSeconds,
    },
  ];
  const apiKeys = [
    {
      id: "cm_key_demo_123456",
      name: "Web Smoke Key",
      protocolType: "openai_compat",
      modelSlug: "gpt-5.4",
      reasoningEffort: "medium",
      serviceTier: "flex",
      status: "enabled",
      upstreamBaseUrl: "",
      staticHeadersJson: "",
      createdAt: nowSeconds - 600,
      lastUsedAt: nowSeconds - 60,
    },
  ];
  const apiKeyUsageStats = [
    {
      keyId: "cm_key_demo_123456",
      totalTokens: 2560,
    },
  ];
  const models = [
    { slug: "gpt-5.4", displayName: "GPT-5.4" },
    { slug: "gpt-5.3-codex", displayName: "GPT-5.3 Codex" },
    { slug: "gpt-5-codex", displayName: "GPT-5 Codex" },
  ];
  const requestLogs = [
    {
      traceId: "trace-demo-001",
      keyId: "cm_key_demo_123456",
      accountId: "acc-demo-001",
      initialAccountId: "acc-demo-001",
      attemptedAccountIds: ["acc-demo-001"],
      requestPath: "/v1/responses",
      originalPath: "/v1/responses",
      adaptedPath: "/v1/responses",
      method: "POST",
      model: "gpt-5.4",
      reasoningEffort: "medium",
      responseAdapter: "responses",
      upstreamUrl: "https://api.openai.com/v1/responses",
      statusCode: 200,
      inputTokens: 800,
      cachedInputTokens: 120,
      outputTokens: 640,
      totalTokens: 1440,
      reasoningOutputTokens: 120,
      estimatedCostUsd: 0.12,
      durationMs: 820,
      error: "",
      createdAt: nowSeconds - 30,
    },
  ];

  return {
    settings: createDefaultSettings(),
    manualPreferredAccountId: "acc-demo-001",
    accounts,
    usageItems,
    apiKeys,
    apiKeyUsageStats,
    models,
    requestLogs,
  };
}

const MIME_TYPES = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".js", "application/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".svg", "image/svg+xml"],
  [".txt", "text/plain; charset=utf-8"],
]);

function contentTypeFor(filePath) {
  return MIME_TYPES.get(path.extname(filePath).toLowerCase()) || "application/octet-stream";
}

async function fileExists(filePath) {
  try {
    await access(filePath);
    return true;
  } catch {
    return false;
  }
}

async function resolveStaticFile(webRoot, urlPath) {
  const rawPath = decodeURIComponent((urlPath || "/").split("?")[0] || "/");
  const normalizedPath = rawPath === "/" ? "/index.html" : rawPath;
  const directPath = path.join(webRoot, normalizedPath);
  if (await fileExists(directPath)) {
    const directStats = await stat(directPath);
    if (directStats.isFile()) {
      return directPath;
    }
    const nestedIndex = path.join(directPath, "index.html");
    if (await fileExists(nestedIndex)) {
      return nestedIndex;
    }
  }

  if (!path.extname(normalizedPath)) {
    const nestedIndex = path.join(webRoot, normalizedPath, "index.html");
    if (await fileExists(nestedIndex)) {
      return nestedIndex;
    }
    const htmlFile = path.join(webRoot, `${normalizedPath}.html`);
    if (await fileExists(htmlFile)) {
      return htmlFile;
    }
  }

  return null;
}

function matchesStatusFilter(log, statusFilter) {
  const code = Number(log.statusCode || 0);
  switch (statusFilter) {
    case "2xx":
      return code >= 200 && code < 300;
    case "4xx":
      return code >= 400 && code < 500;
    case "5xx":
      return code >= 500;
    case "all":
    default:
      return true;
  }
}

function filterLogs(logs, params = {}) {
  const query = String(params.query || "").trim().toLowerCase();
  const statusFilter = String(params.statusFilter || "all").trim();
  return logs.filter((log) => {
    if (!matchesStatusFilter(log, statusFilter)) {
      return false;
    }
    if (!query) {
      return true;
    }
    return [
      log.requestPath,
      log.accountId,
      log.keyId,
      log.model,
      log.error,
    ]
      .map((value) => String(value || "").toLowerCase())
      .some((value) => value.includes(query));
  });
}

function buildStartupSnapshot(state) {
  return {
    accounts: state.accounts,
    usageSnapshots: state.usageItems,
    usageAggregateSummary: {
      primaryBucketCount: 1,
      primaryKnownCount: 1,
      primaryUnknownCount: 0,
      primaryRemainPercent: 80,
      secondaryBucketCount: 1,
      secondaryKnownCount: 1,
      secondaryUnknownCount: 0,
      secondaryRemainPercent: 65,
    },
    apiKeys: state.apiKeys,
    apiModelOptions: state.models,
    manualPreferredAccountId: state.manualPreferredAccountId,
    requestLogTodaySummary: {
      inputTokens: 800,
      cachedInputTokens: 120,
      outputTokens: 640,
      reasoningOutputTokens: 120,
      todayTokens: 1320,
      estimatedCost: 0.12,
    },
    requestLogs: state.requestLogs,
  };
}

function applySettingsPatch(state, patch) {
  const nextPatch = patch && typeof patch === "object" && !Array.isArray(patch) ? { ...patch } : {};
  if (Object.prototype.hasOwnProperty.call(nextPatch, "webAccessPassword")) {
    state.settings.webAccessPasswordConfigured =
      String(nextPatch.webAccessPassword || "").trim().length > 0;
    delete nextPatch.webAccessPassword;
  }
  if (nextPatch.backgroundTasks && typeof nextPatch.backgroundTasks === "object") {
    state.settings.backgroundTasks = {
      ...state.settings.backgroundTasks,
      ...nextPatch.backgroundTasks,
    };
    delete nextPatch.backgroundTasks;
  }
  state.settings = {
    ...state.settings,
    ...nextPatch,
  };
  return state.settings;
}

function handleRpc(state, method, params) {
  switch (method) {
    case "appSettings/get":
      return state.settings;
    case "appSettings/set":
      return applySettingsPatch(state, params);
    case "initialize":
      return {
        serverName: "codexmanager-service",
        version: "0.1.13-mock",
        userAgent: "codexmanager-web-ui-smoke",
      };
    case "startup/snapshot":
      return buildStartupSnapshot(state);
    case "account/list":
      return {
        items: state.accounts,
        total: state.accounts.length,
        page: 1,
        pageSize: state.accounts.length || 20,
      };
    case "account/usage/list":
      return { items: state.usageItems };
    case "account/import":
      return {
        total: Array.isArray(params?.contents) ? params.contents.length : 0,
        created: Array.isArray(params?.contents) ? params.contents.length : 0,
        updated: 0,
        failed: 0,
      };
    case "account/login/start":
      return {
        authUrl: "https://example.invalid/auth/mock",
        loginId: "login-mock-001",
        loginType: "chatgpt",
        issuer: "mock",
        clientId: "mock-client",
        redirectUri: "https://example.invalid/callback",
        warning: "",
        device: null,
      };
    case "account/login/status":
      return {
        status: "pending",
        error: "",
      };
    case "account/login/complete":
      return { ok: true };
    case "account/logout":
      return { ok: true };
    case "gateway/manualAccount/get":
      return { accountId: state.manualPreferredAccountId };
    case "gateway/manualAccount/set":
      state.manualPreferredAccountId = String(params?.accountId || "").trim();
      return { ok: true };
    case "gateway/manualAccount/clear":
      state.manualPreferredAccountId = "";
      return { ok: true };
    case "apikey/list":
      return { items: state.apiKeys };
    case "apikey/models":
      return { items: state.models };
    case "apikey/usageStats":
      return { items: state.apiKeyUsageStats };
    case "apikey/create": {
      const nextId = `cm_key_mock_${state.apiKeys.length + 1}`;
      state.apiKeys = [
        {
          id: nextId,
          name: String(params?.name || "").trim() || `Mock Key ${state.apiKeys.length + 1}`,
          protocolType: String(params?.protocolType || "openai_compat"),
          modelSlug: String(params?.modelSlug || ""),
          reasoningEffort: String(params?.reasoningEffort || ""),
          serviceTier: String(params?.serviceTier || ""),
          status: "enabled",
          upstreamBaseUrl: String(params?.upstreamBaseUrl || ""),
          staticHeadersJson: String(params?.staticHeadersJson || ""),
          createdAt: Math.floor(Date.now() / 1000),
          lastUsedAt: null,
        },
        ...state.apiKeys,
      ];
      return {
        id: nextId,
        key: `${nextId}_secret`,
      };
    }
    case "apikey/updateModel":
      return { ok: true };
    case "apikey/delete":
      state.apiKeys = state.apiKeys.filter((item) => item.id !== String(params?.id || ""));
      return { ok: true };
    case "apikey/disable":
    case "apikey/enable":
      return { ok: true };
    case "apikey/readSecret":
      return { key: "cm_key_demo_live_secret" };
    case "requestlog/list": {
      const filtered = filterLogs(state.requestLogs, params);
      const page = Math.max(1, Number.parseInt(String(params?.page || "1"), 10) || 1);
      const pageSize = Math.max(
        1,
        Number.parseInt(String(params?.pageSize || "20"), 10) || 20
      );
      const start = (page - 1) * pageSize;
      return {
        items: filtered.slice(start, start + pageSize),
        total: filtered.length,
        page,
        pageSize,
      };
    }
    case "requestlog/summary": {
      const filtered = filterLogs(state.requestLogs, params);
      return {
        totalCount: state.requestLogs.length,
        filteredCount: filtered.length,
        successCount: filtered.filter((item) => Number(item.statusCode || 0) >= 200 && Number(item.statusCode || 0) < 300).length,
        errorCount: filtered.filter((item) => Number(item.statusCode || 0) >= 400 || String(item.error || "").trim()).length,
        totalTokens: filtered.reduce((sum, item) => sum + Number(item.totalTokens || 0), 0),
      };
    }
    case "requestlog/clear":
      state.requestLogs = [];
      return { ok: true };
    case "requestlog/today_summary":
      return {
        inputTokens: 800,
        cachedInputTokens: 120,
        outputTokens: 640,
        reasoningOutputTokens: 120,
        todayTokens: 1320,
        estimatedCost: 0.12,
      };
    default:
      throw new Error(`mock unsupported rpc method: ${method}`);
  }
}

async function serveStatic(webRoot, requestPath, response) {
  const filePath = await resolveStaticFile(webRoot, requestPath);
  if (!filePath) {
    response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
    response.end("Not Found");
    return;
  }

  const body = await readFile(filePath);
  response.writeHead(200, { "content-type": contentTypeFor(filePath) });
  response.end(body);
}

async function readJsonBody(request) {
  const chunks = [];
  for await (const chunk of request) {
    chunks.push(chunk);
  }
  const raw = Buffer.concat(chunks).toString("utf8");
  return raw ? JSON.parse(raw) : {};
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const state = createState();
  const server = http.createServer(async (request, response) => {
    try {
      const url = new URL(request.url || "/", `http://127.0.0.1:${options.port}`);

      if (url.pathname === "/__health") {
        response.writeHead(200, { "content-type": "application/json; charset=utf-8" });
        response.end(JSON.stringify({ ok: true, mode: options.mode }));
        return;
      }

      if (options.mode === "supported" && url.pathname === "/api/runtime") {
        response.writeHead(200, { "content-type": "application/json; charset=utf-8" });
        response.end(
          JSON.stringify({
            mode: "web-gateway",
            rpcBaseUrl: "/api/rpc",
            canManageService: false,
            canSelfUpdate: false,
            canCloseToTray: false,
            canOpenLocalDir: false,
            canUseBrowserFileImport: true,
            canUseBrowserDownloadExport: true,
          })
        );
        return;
      }

      if (options.mode === "supported" && url.pathname === "/api/rpc" && request.method === "POST") {
        const payload = await readJsonBody(request);
        const result = handleRpc(state, payload.method, payload.params || {});
        response.writeHead(200, { "content-type": "application/json; charset=utf-8" });
        response.end(
          JSON.stringify({
            jsonrpc: "2.0",
            id: payload.id ?? Date.now(),
            result,
          })
        );
        return;
      }

      await serveStatic(options.webRoot, url.pathname, response);
    } catch (error) {
      response.writeHead(500, { "content-type": "application/json; charset=utf-8" });
      response.end(
        JSON.stringify({
          error: error instanceof Error ? error.message : String(error),
        })
      );
    }
  });

  server.listen(options.port, "127.0.0.1", () => {
    process.stdout.write(
      `web_ui_mock_server listening on http://127.0.0.1:${options.port} mode=${options.mode}\n`
    );
  });

  const shutdown = () => {
    server.close(() => process.exit(0));
  };

  process.on("SIGINT", shutdown);
  process.on("SIGTERM", shutdown);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
