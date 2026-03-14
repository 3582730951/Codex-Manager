"use client";

import { useMemo, useState } from "react";
import { useSearchParams } from "next/navigation";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, Search, Shield, Trash2, Zap } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { serviceClient } from "@/lib/api/service-client";
import { useAppStore } from "@/lib/store/useAppStore";
import { formatTsFromSeconds } from "@/lib/utils/usage";
import { cn } from "@/lib/utils";
import { RequestLog } from "@/types";

type StatusFilter = "all" | "2xx" | "4xx" | "5xx";

function getStatusBadge(statusCode: number | null) {
  if (statusCode == null) {
    return <Badge variant="secondary">-</Badge>;
  }
  if (statusCode >= 200 && statusCode < 300) {
    return <Badge className="border-green-500/20 bg-green-500/10 text-green-500">{statusCode}</Badge>;
  }
  if (statusCode >= 400 && statusCode < 500) {
    return <Badge className="border-yellow-500/20 bg-yellow-500/10 text-yellow-500">{statusCode}</Badge>;
  }
  return <Badge className="border-red-500/20 bg-red-500/10 text-red-500">{statusCode}</Badge>;
}

function formatDuration(value: number | null): string {
  if (value == null) return "-";
  if (value >= 10_000) return `${Math.round(value / 1000)}s`;
  if (value >= 1000) return `${(value / 1000).toFixed(1).replace(/\.0$/, "")}s`;
  return `${Math.round(value)}ms`;
}

export default function LogsPage() {
  const searchParams = useSearchParams();
  const { serviceStatus } = useAppStore();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState(() => searchParams.get("query") || "");
  const [filter, setFilter] = useState<StatusFilter>("all");

  const { data: logs = [], isLoading } = useQuery({
    queryKey: ["logs", search],
    queryFn: () => serviceClient.listRequestLogs(search, 100),
    enabled: serviceStatus.connected,
    refetchInterval: 5000,
    retry: 1,
  });

  const clearMutation = useMutation({
    mutationFn: () => serviceClient.clearRequestLogs(),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["logs"] }),
        queryClient.invalidateQueries({ queryKey: ["today-summary"] }),
        queryClient.invalidateQueries({ queryKey: ["startup-snapshot"] }),
      ]);
      toast.success("日志已清空");
    },
    onError: (error: unknown) => {
      toast.error(error instanceof Error ? error.message : String(error));
    },
  });

  const filteredLogs = useMemo(() => {
    return logs.filter((log: RequestLog) => {
      if (filter === "all") return true;
      const statusCode = log.statusCode ?? 0;
      if (filter === "2xx") return statusCode >= 200 && statusCode < 300;
      if (filter === "4xx") return statusCode >= 400 && statusCode < 500;
      if (filter === "5xx") return statusCode >= 500;
      return true;
    });
  }, [filter, logs]);

  return (
    <div className="space-y-6 animate-in fade-in duration-500">
      <div className="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
        <div className="flex max-w-md flex-1 items-center gap-2">
          <div className="relative w-full">
            <Search className="absolute left-2.5 top-2.5 h-4 w-4 text-muted-foreground" />
            <Input
              placeholder="搜索路径、账号或密钥..."
              className="glass-card h-10 pl-9"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          </div>
          <div className="flex rounded-lg border bg-muted/30 p-1">
            {["all", "2xx", "4xx", "5xx"].map((item) => (
              <button
                key={item}
                onClick={() => setFilter(item as StatusFilter)}
                className={cn(
                  "rounded-md px-3 py-1 text-[10px] font-bold transition-all",
                  filter === item
                    ? "bg-background text-foreground shadow-sm"
                    : "text-muted-foreground hover:bg-muted"
                )}
              >
                {item.toUpperCase()}
              </button>
            ))}
          </div>
        </div>

        <div className="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            className="glass-card"
            onClick={() => queryClient.invalidateQueries({ queryKey: ["logs"] })}
          >
            <RefreshCw className="mr-2 h-4 w-4" /> 刷新
          </Button>
          <Button
            variant="destructive"
            size="sm"
            onClick={() => clearMutation.mutate()}
            disabled={clearMutation.isPending}
          >
            <Trash2 className="mr-2 h-4 w-4" /> 清空日志
          </Button>
        </div>
      </div>

      <Card className="glass-card overflow-hidden border-none shadow-xl backdrop-blur-md">
        <CardContent className="p-0">
          <Table>
            <TableHeader className="bg-muted/30">
              <TableRow>
                <TableHead className="w-[180px]">时间</TableHead>
                <TableHead>方法 / 路径</TableHead>
                <TableHead>账号 / 密钥</TableHead>
                <TableHead>模型</TableHead>
                <TableHead>状态</TableHead>
                <TableHead>请求时长</TableHead>
                <TableHead>令牌</TableHead>
                <TableHead>上游 / 错误</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {isLoading ? (
                Array.from({ length: 10 }).map((_, index) => (
                  <TableRow key={index}>
                    <TableCell><Skeleton className="h-4 w-32" /></TableCell>
                    <TableCell><Skeleton className="h-4 w-40" /></TableCell>
                    <TableCell><Skeleton className="h-4 w-32" /></TableCell>
                    <TableCell><Skeleton className="h-4 w-24" /></TableCell>
                    <TableCell><Skeleton className="h-6 w-12 rounded-full" /></TableCell>
                    <TableCell><Skeleton className="h-4 w-12" /></TableCell>
                    <TableCell><Skeleton className="h-4 w-20" /></TableCell>
                    <TableCell><Skeleton className="h-4 w-full" /></TableCell>
                  </TableRow>
                ))
              ) : filteredLogs.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={8} className="h-48 text-center text-muted-foreground">
                    {!serviceStatus.connected ? "服务未连接，无法获取日志" : "暂无请求日志"}
                  </TableCell>
                </TableRow>
              ) : (
                filteredLogs.map((log) => (
                  <TableRow key={log.id} className="group text-[11px] hover:bg-muted/30">
                    <TableCell className="font-mono text-muted-foreground">
                      {formatTsFromSeconds(log.createdAt, "未知时间")}
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-col">
                        <span className="font-bold text-primary">{log.method || "-"}</span>
                        <span className="max-w-[220px] truncate text-muted-foreground">
                          {log.path || log.requestPath || "-"}
                        </span>
                      </div>
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-col gap-0.5 opacity-80">
                        <div className="flex items-center gap-1">
                          <Zap className="h-3 w-3 text-yellow-500" />
                          <span className="max-w-[120px] truncate">{log.accountId || "-"}</span>
                        </div>
                        <div className="flex items-center gap-1 text-[9px] text-muted-foreground">
                          <Shield className="h-2.5 w-2.5" />
                          <span className="font-mono">
                            {log.keyId ? `gk_${log.keyId.slice(0, 6)}` : "-"}
                          </span>
                        </div>
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge variant="secondary" className="bg-accent/30 text-[9px] font-normal">
                        {log.model || "-"}
                      </Badge>
                    </TableCell>
                    <TableCell>{getStatusBadge(log.statusCode)}</TableCell>
                    <TableCell className="font-mono text-primary">
                      {formatDuration(log.durationMs)}
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-col text-[9px] text-muted-foreground">
                        <span>总 {log.totalTokens?.toLocaleString() || 0}</span>
                        <span>输入 {log.inputTokens?.toLocaleString() || 0}</span>
                        <span className="opacity-60">缓存 {log.cachedInputTokens?.toLocaleString() || 0}</span>
                      </div>
                    </TableCell>
                    <TableCell
                      className={cn(
                        "max-w-[240px] truncate font-medium",
                        log.error ? "text-red-400" : "text-muted-foreground"
                      )}
                      title={log.error || log.upstreamUrl}
                    >
                      {log.error || log.upstreamUrl || "-"}
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </div>
  );
}
