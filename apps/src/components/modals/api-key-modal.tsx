"use client";

import { useState, useEffect } from "react";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button, buttonVariants } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useRuntimeCapabilities } from "@/hooks/useRuntimeCapabilities";
import { accountClient } from "@/lib/api/account-client";
import { useAppStore } from "@/lib/store/useAppStore";
import { copyTextToClipboard } from "@/lib/utils/clipboard";
import { toast } from "sonner";
import { useQueryClient, useQuery } from "@tanstack/react-query";
import { Key, Clipboard, ShieldCheck } from "lucide-react";
import { ApiKey } from "@/types";

const PROTOCOL_LABELS: Record<string, string> = {
  openai_compat: "OpenAI 兼容",
  azure_openai: "Azure OpenAI",
  anthropic_native: "Claude Code 兼容",
};

const REASONING_LABELS: Record<string, string> = {
  auto: "跟随请求",
  low: "低 (low)",
  medium: "中 (medium)",
  high: "高 (high)",
  xhigh: "极高 (xhigh)",
};

const SERVICE_TIER_LABELS: Record<string, string> = {
  auto: "跟随请求",
  fast: "Fast",
  flex: "Flex",
};

const ROTATION_STRATEGY_LABELS: Record<string, string> = {
  account_rotation: "账号轮转",
  aggregate_api_rotation: "聚合API轮转",
};

interface ApiKeyModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  apiKey?: ApiKey | null;
}

export function ApiKeyModal({ open, onOpenChange, apiKey }: ApiKeyModalProps) {
  const serviceStatus = useAppStore((state) => state.serviceStatus);
  const { canAccessManagementRpc } = useRuntimeCapabilities();
  const [name, setName] = useState("");
  const [protocolType, setProtocolType] = useState("openai_compat");
  const [modelSlug, setModelSlug] = useState("");
  const [reasoningEffort, setReasoningEffort] = useState("");
  const [serviceTier, setServiceTier] = useState("");
  const [rotationStrategy, setRotationStrategy] = useState("account_rotation");
  const [upstreamBaseUrl, setUpstreamBaseUrl] = useState("");
  const [azureEndpoint, setAzureEndpoint] = useState("");
  const [azureApiKey, setAzureApiKey] = useState("");
  const [generatedKey, setGeneratedKey] = useState("");

  const [isLoading, setIsLoading] = useState(false);
  const queryClient = useQueryClient();
  const isServiceReady = canAccessManagementRpc && serviceStatus.connected;
  const unavailableMessage = canAccessManagementRpc
    ? "服务未连接，平台密钥与模型配置暂不可编辑；连接恢复后可继续操作。"
    : "当前运行环境暂不支持平台密钥管理。";

  const { data: models } = useQuery({
    queryKey: ["apikey-models"],
    queryFn: () => accountClient.listModels(false),
    enabled: open && isServiceReady,
  });

  const modelLabelMap = Object.fromEntries(
    (models || []).map((model) => [model.slug, model.displayName]),
  );

  useEffect(() => {
    if (!open) return;

    if (!apiKey) {
      setName("");
      setProtocolType("openai_compat");
      setModelSlug("");
      setReasoningEffort("");
      setServiceTier("");
      setRotationStrategy("account_rotation");
      setUpstreamBaseUrl("");
      setAzureEndpoint("");
      setAzureApiKey("");
      setGeneratedKey("");
      return;
    }

    setName(apiKey.name || "");
    setProtocolType(apiKey.protocol || "openai_compat");
    setModelSlug(apiKey.modelSlug || "");
    setReasoningEffort(apiKey.reasoningEffort || "");
    setServiceTier(apiKey.serviceTier || "");
    setRotationStrategy(apiKey.rotationStrategy || "account_rotation");
    setGeneratedKey("");

    if (apiKey.protocol === "azure_openai") {
      setAzureEndpoint(apiKey.upstreamBaseUrl || "");
      try {
        const headers = apiKey.staticHeadersJson
          ? JSON.parse(apiKey.staticHeadersJson)
          : {};
        setAzureApiKey(
          typeof headers["api-key"] === "string" ? headers["api-key"] : "",
        );
      } catch {
        setAzureApiKey("");
      }
      setUpstreamBaseUrl("");
    } else {
      setUpstreamBaseUrl(apiKey.upstreamBaseUrl || "");
      setAzureEndpoint("");
      setAzureApiKey("");
    }
  }, [apiKey, open]);

  const handleSave = async () => {
    if (!isServiceReady) {
      toast.info(
        canAccessManagementRpc
          ? "服务未连接，暂时无法保存平台密钥"
          : "当前运行环境暂不支持平台密钥管理",
      );
      return;
    }
    setIsLoading(true);
    try {
      const staticHeaders: Record<string, string> = {};
      if (protocolType === "azure_openai" && azureApiKey) {
        staticHeaders["api-key"] = azureApiKey;
      }

      const params = {
        name: name || null,
        modelSlug: !modelSlug || modelSlug === "auto" ? null : modelSlug,
        reasoningEffort:
          !reasoningEffort || reasoningEffort === "auto"
            ? null
            : reasoningEffort,
        serviceTier:
          !serviceTier || serviceTier === "auto" ? null : serviceTier,
        protocolType,
        upstreamBaseUrl:
          protocolType === "azure_openai"
            ? azureEndpoint
            : upstreamBaseUrl || null,
        staticHeadersJson:
          Object.keys(staticHeaders).length > 0
            ? JSON.stringify(staticHeaders)
            : null,
        rotationStrategy,
      };

      if (apiKey?.id) {
        await accountClient.updateApiKey(apiKey.id, params);
        toast.success("密钥配置已更新");
      } else {
        const result = await accountClient.createApiKey(params);
        setGeneratedKey(result.key);
        toast.success("平台密钥已创建");
      }

      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["apikeys"] }),
        queryClient.invalidateQueries({ queryKey: ["apikey-models"] }),
        queryClient.invalidateQueries({ queryKey: ["startup-snapshot"] }),
      ]);
      if (apiKey?.id) onOpenChange(false);
    } catch (err: unknown) {
      toast.error(
        `操作失败: ${err instanceof Error ? err.message : String(err)}`,
      );
    } finally {
      setIsLoading(false);
    }
  };

  const copyKey = async () => {
    try {
      await copyTextToClipboard(generatedKey);
      toast.success("密钥已复制");
    } catch (error: unknown) {
      toast.error(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[550px] glass-card border-none">
        <DialogHeader>
          <div className="flex items-center gap-3 mb-2">
            <div className="p-2 rounded-full bg-primary/10">
              <Key className="h-5 w-5 text-primary" />
            </div>
            <DialogTitle>
              {apiKey?.id ? "编辑平台密钥" : "创建平台密钥"}
            </DialogTitle>
          </div>
          <DialogDescription>
            配置网关访问凭据，您可以绑定特定模型、推理等级或自定义上游。
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-5 py-4">
          {!isServiceReady ? (
            <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-sm text-muted-foreground">
              {unavailableMessage}
            </div>
          ) : null}
          <div className="grid grid-cols-2 gap-4 items-start">
            <div className="grid gap-2 content-start">
              <Label htmlFor="name">密钥名称 (可选)</Label>
              <Input
                id="name"
                placeholder="例如：主机房 / 测试"
                value={name}
                disabled={!isServiceReady}
                onChange={(e) => setName(e.target.value)}
              />
            </div>
            <div className="grid gap-2 content-start">
              <Label>轮转策略</Label>
              <Select
                value={rotationStrategy}
                onValueChange={(val) => {
                  if (!val) return;
                  setRotationStrategy(val);
                }}
                disabled={!isServiceReady}
              >
                <SelectTrigger className="w-full">
                  <SelectValue>
                    {(value) =>
                      ROTATION_STRATEGY_LABELS[String(value || "")] ||
                      "账号轮转"
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectContent align="start">
                  <SelectItem value="account_rotation">账号轮转</SelectItem>
                  <SelectItem value="aggregate_api_rotation">
                    聚合API轮转
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
            <p className="col-span-2 -mt-1 text-[11px] text-muted-foreground">
              账号轮转保持现有路由逻辑；聚合API轮转会直接透传请求。
            </p>
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div className="grid gap-2 content-start">
              <Label>协议类型</Label>
              <Select
                value={protocolType}
                onValueChange={(val) => val && setProtocolType(val)}
                disabled={!isServiceReady}
              >
                <SelectTrigger className="w-full">
                  <SelectValue>
                    {(value) =>
                      PROTOCOL_LABELS[String(value || "")] || "OpenAI 兼容"
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectContent align="start">
                  <SelectItem value="openai_compat">OpenAI 兼容</SelectItem>
                  <SelectItem value="azure_openai">Azure OpenAI</SelectItem>
                  <SelectItem value="anthropic_native">
                    Claude Code 兼容
                  </SelectItem>
                </SelectContent>
              </Select>
              <p className="min-h-[32px] text-[11px] text-muted-foreground">
                决定认证头和请求协议改写方式。
              </p>
            </div>
            <div className="grid gap-2 content-start">
              <Label>绑定模型 (可选)</Label>
              <Select
                value={modelSlug}
                onValueChange={(val) => val && setModelSlug(val)}
                disabled={!isServiceReady}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="跟随请求">
                    {(value) => {
                      const nextValue = String(value || "").trim();
                      if (!nextValue || nextValue === "auto") return "跟随请求";
                      return modelLabelMap[nextValue] || nextValue;
                    }}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent align="start">
                  <SelectItem value="auto">跟随请求</SelectItem>
                  {models?.map((model) => (
                    <SelectItem key={model.slug} value={model.slug}>
                      {model.displayName}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-[11px] text-muted-foreground">
                选择“跟随请求”时，会使用请求体里的实际模型；请求日志展示的是最终生效模型。
              </p>
            </div>
          </div>

          <div className="grid grid-cols-2 gap-4">
            <div className="grid gap-2 content-start">
              <Label>推理等级 (可选)</Label>
              <Select
                value={reasoningEffort}
                onValueChange={(val) => val && setReasoningEffort(val)}
                disabled={!isServiceReady}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="跟随请求等级">
                    {(value) => {
                      const nextValue = String(value || "").trim();
                      if (!nextValue) return "跟随请求等级";
                      return REASONING_LABELS[nextValue] || nextValue;
                    }}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent align="start">
                  <SelectItem value="auto">跟随请求</SelectItem>
                  <SelectItem value="low">低 (low)</SelectItem>
                  <SelectItem value="medium">中 (medium)</SelectItem>
                  <SelectItem value="high">高 (high)</SelectItem>
                  <SelectItem value="xhigh">极高 (xhigh)</SelectItem>
                </SelectContent>
              </Select>
              <p className="min-h-[32px] text-[11px] text-muted-foreground">
                会覆盖请求里的 reasoning effort。
              </p>
            </div>
            <div className="grid gap-2 content-start">
              <Label>服务等级 (可选)</Label>
              <Select
                value={serviceTier}
                onValueChange={(val) => val && setServiceTier(val)}
                disabled={!isServiceReady}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="跟随请求">
                    {(value) => {
                      const nextValue = String(value || "").trim();
                      if (!nextValue) return "跟随请求";
                      return SERVICE_TIER_LABELS[nextValue] || nextValue;
                    }}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent align="start">
                  <SelectItem value="auto">跟随请求</SelectItem>
                  <SelectItem value="fast">Fast</SelectItem>
                  <SelectItem value="flex">Flex</SelectItem>
                </SelectContent>
              </Select>
              <p className="text-[11px] text-muted-foreground">
                Fast 会映射为上游 priority，Flex 会直传为 flex。
              </p>
            </div>
          </div>

          {protocolType === "azure_openai" ? (
            <div className="grid gap-4 p-4 rounded-xl bg-accent/20 border border-primary/10">
              <div className="grid gap-2">
                <Label className="text-xs">Azure 接入地址</Label>
                <Input
                  placeholder="https://your-resource.openai.azure.com"
                  value={azureEndpoint}
                  disabled={!isServiceReady}
                  onChange={(e) => setAzureEndpoint(e.target.value)}
                  className="h-9 font-mono text-xs"
                />
              </div>
              <div className="grid gap-2">
                <Label className="text-xs">Azure 接口密钥</Label>
                <Input
                  type="password"
                  placeholder="your-azure-key"
                  value={azureApiKey}
                  disabled={!isServiceReady}
                  onChange={(e) => setAzureApiKey(e.target.value)}
                  className="h-9 font-mono text-xs"
                />
              </div>
            </div>
          ) : null}

          {generatedKey && (
            <div className="space-y-2 pt-4 border-t">
              <Label className="text-xs text-primary flex items-center gap-1.5">
                <ShieldCheck className="h-3.5 w-3.5" /> 平台密钥已生成
              </Label>
              <div className="flex gap-2">
                <Input
                  value={generatedKey}
                  readOnly
                  className="font-mono text-sm bg-primary/5"
                />
                <Button
                  variant="outline"
                  onClick={() => void copyKey()}
                  disabled={!generatedKey}
                >
                  <Clipboard className="h-4 w-4" />
                </Button>
              </div>
            </div>
          )}
        </div>

        <DialogFooter>
          <DialogClose
            className={buttonVariants({ variant: "ghost" })}
            type="button"
          >
            {generatedKey ? "关闭" : "取消"}
          </DialogClose>
          {!generatedKey && (
            <Button
              onClick={handleSave}
              disabled={!isServiceReady || isLoading}
            >
              {isLoading ? "保存中..." : "完成"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
