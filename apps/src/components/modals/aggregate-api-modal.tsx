"use client";

import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Clipboard, Database, ShieldCheck } from "lucide-react";
import { toast } from "sonner";
import {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
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
import { accountClient } from "@/lib/api/account-client";
import { copyTextToClipboard } from "@/lib/utils/clipboard";
import { useAppStore } from "@/lib/store/useAppStore";
import { useRuntimeCapabilities } from "@/hooks/useRuntimeCapabilities";
import { AggregateApi } from "@/types";

const AGGREGATE_API_PROVIDER_LABELS: Record<string, string> = {
  codex: "Codex",
  claude: "Claude",
};

const AGGREGATE_API_URL_PLACEHOLDERS: Record<string, string> = {
  codex: "例如：https://api.openai.com/v1",
  claude: "例如：https://api.anthropic.com/v1",
};

interface AggregateApiModalProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  aggregateApi?: AggregateApi | null;
  defaultSort?: number;
}

export function AggregateApiModal({
  open,
  onOpenChange,
  aggregateApi,
  defaultSort = 0,
}: AggregateApiModalProps) {
  const serviceStatus = useAppStore((state) => state.serviceStatus);
  const { canAccessManagementRpc } = useRuntimeCapabilities();
  const [providerType, setProviderType] = useState("codex");
  const [supplierName, setSupplierName] = useState("");
  const [sortDraft, setSortDraft] = useState("0");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [generatedKey, setGeneratedKey] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const queryClient = useQueryClient();
  const isServiceReady = canAccessManagementRpc && serviceStatus.connected;
  const unavailableMessage = canAccessManagementRpc
    ? "服务未连接，聚合 API 暂不可编辑；连接恢复后可继续操作。"
    : "当前运行环境暂不支持聚合 API 管理。";

  useEffect(() => {
    if (!open) return;
    const nextProviderType = aggregateApi?.providerType || "codex";
    setProviderType(nextProviderType);
    setSupplierName(aggregateApi?.supplierName || "");
    setSortDraft(String(aggregateApi?.sort ?? defaultSort));
    setUrl(aggregateApi?.url || "");
    setKey("");
    setGeneratedKey("");
  }, [aggregateApi, defaultSort, open]);

  const handleSave = async () => {
    if (!isServiceReady) {
      toast.info(
        canAccessManagementRpc
          ? "服务未连接，暂时无法保存聚合 API"
          : "当前运行环境暂不支持聚合 API 管理"
      );
      return;
    }
    if (!url.trim()) {
      toast.error("请输入聚合 API URL");
      return;
    }
    if (!supplierName.trim()) {
      toast.error("请输入供应商名称");
      return;
    }
    const rawSort = sortDraft.trim();
    if (!rawSort) {
      toast.error("请输入顺序值");
      return;
    }
    const parsedSort = Number(rawSort);
    if (!Number.isFinite(parsedSort)) {
      toast.error("顺序必须是数字");
      return;
    }
    if (!aggregateApi?.id && !key.trim()) {
      toast.error("请输入聚合 API 密钥");
      return;
    }

    setIsLoading(true);
    try {
      if (aggregateApi?.id) {
        await accountClient.updateAggregateApi(aggregateApi.id, {
          providerType,
          supplierName,
          sort: parsedSort,
          url,
          key: key || null,
        });
        toast.success("聚合 API 已更新");
        await Promise.all([
          queryClient.invalidateQueries({ queryKey: ["aggregate-apis"] }),
          queryClient.invalidateQueries({ queryKey: ["apikeys"] }),
          queryClient.invalidateQueries({ queryKey: ["startup-snapshot"] }),
        ]);
        onOpenChange(false);
        return;
      }

      const result = await accountClient.createAggregateApi({
        providerType,
        supplierName,
        sort: parsedSort,
        url,
        key,
      });
      setGeneratedKey(result.key);
      toast.success("聚合 API 已创建");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["aggregate-apis"] }),
        queryClient.invalidateQueries({ queryKey: ["apikeys"] }),
        queryClient.invalidateQueries({ queryKey: ["startup-snapshot"] }),
      ]);
      onOpenChange(false);
    } catch (error: unknown) {
      toast.error(
        `操作失败: ${error instanceof Error ? error.message : String(error)}`
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
      <DialogContent className="sm:max-w-[520px] glass-card border-none">
        <DialogHeader>
          <div className="mb-2 flex items-center gap-3">
            <div className="rounded-full bg-primary/10 p-2">
              <Database className="h-5 w-5 text-primary" />
            </div>
            <DialogTitle>
              {aggregateApi?.id ? "编辑聚合 API" : "创建聚合 API"}
            </DialogTitle>
          </div>
          <DialogDescription>
            配置一个最小转发上游，保存 URL 和密钥后即可用于平台密钥轮转。
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-5 py-4">
          {!isServiceReady ? (
            <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-sm text-muted-foreground">
              {unavailableMessage}
            </div>
          ) : null}

          <div className="grid gap-2">
            <Label htmlFor="aggregate-api-supplier-name">供应商名称 *</Label>
            <Input
              id="aggregate-api-supplier-name"
              placeholder="例如：官方中转、XX 供应商"
              value={supplierName}
              disabled={!isServiceReady}
              onChange={(event) => setSupplierName(event.target.value)}
            />
          </div>

          <div className="grid gap-2">
            <Label htmlFor="aggregate-api-sort">顺序值</Label>
            <Input
              id="aggregate-api-sort"
              type="number"
              min={0}
              step={1}
              value={sortDraft}
              disabled={!isServiceReady}
              onChange={(event) => setSortDraft(event.target.value)}
            />
            <p className="text-[11px] leading-4 text-muted-foreground">
              值越小越靠前，用于聚合 API 轮转优先级
            </p>
          </div>

          <div className="grid gap-2">
            <Label htmlFor="aggregate-api-provider">类型</Label>
            <Select
              value={providerType}
              disabled={!isServiceReady}
              onValueChange={(value) => {
                if (!value) return;
                setProviderType(value);
              }}
            >
              <SelectTrigger id="aggregate-api-provider" className="w-full">
                <SelectValue>
                  {(value) => AGGREGATE_API_PROVIDER_LABELS[String(value || "")] || "Codex"}
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="codex">Codex</SelectItem>
                <SelectItem value="claude">Claude</SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div className="grid gap-2">
            <Label htmlFor="aggregate-api-url">URL</Label>
            <Input
              id="aggregate-api-url"
              placeholder={AGGREGATE_API_URL_PLACEHOLDERS[providerType] || "请输入 URL"}
              value={url}
              disabled={!isServiceReady}
              onChange={(event) => setUrl(event.target.value)}
            />
          </div>

          <div className="grid gap-2">
            <Label htmlFor="aggregate-api-key">密钥</Label>
            <Input
              id="aggregate-api-key"
              type="password"
              placeholder={aggregateApi?.id ? "留空则保持原值" : "请输入密钥"}
              value={key}
              disabled={!isServiceReady}
              onChange={(event) => setKey(event.target.value)}
            />
          </div>

          {generatedKey ? (
            <div className="space-y-2 pt-2 border-t">
              <Label className="text-xs text-primary flex items-center gap-1.5">
                <ShieldCheck className="h-3.5 w-3.5" /> 新密钥已生成
              </Label>
              <div className="flex gap-2">
                <Input
                  value={generatedKey}
                  readOnly
                  className="bg-primary/5 font-mono text-sm"
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
          ) : null}
        </div>

        <DialogFooter>
          {!generatedKey ? (
            <DialogClose
              className={buttonVariants({ variant: "ghost" })}
              type="button"
            >
              取消
            </DialogClose>
          ) : null}
          {!generatedKey ? (
            <Button onClick={() => void handleSave()} disabled={!isServiceReady || isLoading}>
              {isLoading ? "保存中..." : "完成"}
            </Button>
          ) : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
