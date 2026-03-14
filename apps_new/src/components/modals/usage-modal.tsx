"use client";

import {
  Calendar,
  Clock,
  Database,
  type LucideIcon,
  RefreshCw,
} from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";
import { formatTsFromSeconds } from "@/lib/utils/usage";
import { Account } from "@/types";

interface UsageModalProps {
  account: Account | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRefresh: (id: string) => void;
  isRefreshing: boolean;
}

interface UsageDetailRowProps {
  label: string;
  remainPercent: number | null;
  resetsAt: number | null | undefined;
  icon: LucideIcon;
}

function UsageDetailRow({
  label,
  remainPercent,
  resetsAt,
  icon: Icon,
}: UsageDetailRowProps) {
  const value = remainPercent ?? 0;

  return (
    <div className="space-y-3 rounded-2xl border border-primary/5 bg-accent/10 p-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <div className="rounded-lg bg-primary/10 p-1.5">
            <Icon className="h-4 w-4 text-primary" />
          </div>
          <span className="font-semibold">{label}</span>
        </div>
        <div className="text-right">
          <span className="text-lg font-bold">{remainPercent == null ? "--" : `${value}%`}</span>
          <span className="ml-1 text-xs text-muted-foreground">剩余</span>
        </div>
      </div>

      <Progress value={value} className="h-2" />

      <div className="flex items-center justify-between text-[10px] text-muted-foreground">
        <span>已使用 {remainPercent == null ? "--" : `${Math.max(0, 100 - value)}%`}</span>
        <span className="flex items-center gap-1">
          <Clock className="h-2.5 w-2.5" />
          重置时间: {formatTsFromSeconds(resetsAt, "未知")}
        </span>
      </div>
    </div>
  );
}

export default function UsageModal({
  account,
  open,
  onOpenChange,
  onRefresh,
  isRefreshing,
}: UsageModalProps) {
  if (!account) return null;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="glass-card border-none p-6 sm:max-w-[450px]">
        <DialogHeader>
          <div className="mb-2 flex items-center gap-3">
            <div className="rounded-full bg-primary/10 p-2 text-primary">
              <Database className="h-5 w-5" />
            </div>
            <DialogTitle>用量详情</DialogTitle>
          </div>
          <DialogDescription className="font-medium text-foreground/80">
            账号: {account.name} ({account.id.slice(0, 8)}...)
          </DialogDescription>
        </DialogHeader>

        <div className="grid gap-4 py-4">
          <UsageDetailRow
            label="5小时额度"
            remainPercent={account.primaryRemainPercent}
            resetsAt={account.usage?.resetsAt}
            icon={Clock}
          />

          <UsageDetailRow
            label="7天周期额度"
            remainPercent={account.secondaryRemainPercent}
            resetsAt={account.usage?.secondaryResetsAt}
            icon={Calendar}
          />

          <div className="text-center">
            <p className="text-[10px] italic text-muted-foreground">
              数据捕获于: {formatTsFromSeconds(account.lastRefreshAt, "未知时间")}
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            关闭
          </Button>
          <Button onClick={() => onRefresh(account.id)} disabled={isRefreshing} className="gap-2">
            <RefreshCw className={cn("h-4 w-4", isRefreshing && "animate-spin")} />
            {isRefreshing ? "正在刷新..." : "立即刷新"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
