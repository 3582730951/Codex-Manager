"use client";

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { accountClient } from "@/lib/api/account-client";

type ApiKeyPayload = Parameters<typeof accountClient.createApiKey>[0];

function getErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error || "");
}

export function useApiKeys() {
  const queryClient = useQueryClient();

  const apiKeysQuery = useQuery({
    queryKey: ["apikeys"],
    queryFn: () => accountClient.listApiKeys(),
    retry: 1,
  });

  const modelsQuery = useQuery({
    queryKey: ["apikey-models"],
    queryFn: () => accountClient.listModels(false),
    retry: 1,
  });

  const invalidateAll = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["apikeys"] }),
      queryClient.invalidateQueries({ queryKey: ["apikey-models"] }),
      queryClient.invalidateQueries({ queryKey: ["apikey-usage-stats"] }),
      queryClient.invalidateQueries({ queryKey: ["startup-snapshot"] }),
    ]);
  };

  const createMutation = useMutation({
    mutationFn: (params: ApiKeyPayload) => accountClient.createApiKey(params),
    onSuccess: async () => {
      await invalidateAll();
      toast.success("密钥已创建");
    },
    onError: (error: unknown) => {
      toast.error(`创建失败: ${getErrorMessage(error)}`);
    },
  });

  const deleteMutation = useMutation({
    mutationFn: (id: string) => accountClient.deleteApiKey(id),
    onSuccess: async () => {
      await invalidateAll();
      toast.success("密钥已删除");
    },
    onError: (error: unknown) => {
      toast.error(`删除失败: ${getErrorMessage(error)}`);
    },
  });

  const updateMutation = useMutation({
    mutationFn: ({ id, params }: { id: string; params: ApiKeyPayload }) =>
      accountClient.updateApiKey(id, params),
    onSuccess: async () => {
      await invalidateAll();
      toast.success("密钥配置已更新");
    },
    onError: (error: unknown) => {
      toast.error(`更新失败: ${getErrorMessage(error)}`);
    },
  });

  const toggleStatusMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      enabled ? accountClient.enableApiKey(id) : accountClient.disableApiKey(id),
    onSuccess: async () => {
      await invalidateAll();
      toast.success("状态已更新");
    },
    onError: (error: unknown) => {
      toast.error(`更新状态失败: ${getErrorMessage(error)}`);
    },
  });

  const refreshModelsMutation = useMutation({
    mutationFn: (refreshRemote: boolean) => accountClient.listModels(refreshRemote),
    onSuccess: async (models) => {
      queryClient.setQueryData(["apikey-models"], models);
      await queryClient.invalidateQueries({ queryKey: ["startup-snapshot"] });
      toast.success("模型列表已刷新");
    },
    onError: (error: unknown) => {
      toast.error(`刷新模型失败: ${getErrorMessage(error)}`);
    },
  });

  const readSecretMutation = useMutation({
    mutationFn: (id: string) => accountClient.readApiKeySecret(id),
    onError: (error: unknown) => {
      toast.error(`读取密钥失败: ${getErrorMessage(error)}`);
    },
  });

  return {
    apiKeys: apiKeysQuery.data || [],
    models: modelsQuery.data || [],
    isLoading: apiKeysQuery.isLoading,
    isModelsLoading: modelsQuery.isLoading,
    createApiKey: createMutation.mutateAsync,
    deleteApiKey: deleteMutation.mutate,
    updateApiKey: updateMutation.mutateAsync,
    toggleApiKeyStatus: toggleStatusMutation.mutate,
    refreshModels: (refreshRemote = true) => refreshModelsMutation.mutate(refreshRemote),
    readApiKeySecret: (id: string) => readSecretMutation.mutateAsync(id),
    isToggling: toggleStatusMutation.isPending,
    isRefreshingModels: refreshModelsMutation.isPending,
    isReadingSecret: readSecretMutation.isPending,
  };
}
