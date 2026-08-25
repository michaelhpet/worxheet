import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

export type ProviderPreset = "openai" | "gemini" | "ollama" | "lmstudio" | "custom";

export interface ProviderConfig {
	preset: string;
	base_url: string;
	model: string;
	concurrency: number;
}

export interface ProviderStatus {
	config: ProviderConfig;
	/** Whether an API key is stored in the OS keychain (never the key itself). */
	api_key_set: boolean;
}

export interface ValidationResult {
	ok: boolean;
	error: string | null;
	models: string[];
}

export const PROVIDER_QUERY_KEY = "PROVIDER";

export interface SetProviderConfigInput {
	preset: string;
	base_url: string;
	model: string;
	concurrency: number;
	/** `undefined` keeps the stored key; empty string clears it. */
	api_key?: string;
}

export function useProviderStatus() {
	return useQuery({
		queryKey: [PROVIDER_QUERY_KEY],
		queryFn: () => invoke<ProviderStatus>("get_provider_status"),
	});
}

export function useSetProviderConfig() {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: (input: SetProviderConfigInput) => invoke<ProviderStatus>("set_provider_config", { ...input }),
		onSuccess: (status) => {
			queryClient.setQueryData([PROVIDER_QUERY_KEY], status);
		},
	});
}

export function useValidateProvider() {
	return useMutation({
		mutationFn: () => invoke<ValidationResult>("validate_provider"),
	});
}

export function useProviderModels(enabled: boolean) {
	return useQuery({
		queryKey: [PROVIDER_QUERY_KEY, "models"],
		queryFn: () => invoke<string[]>("list_provider_models"),
		enabled,
		staleTime: 60_000,
		retry: false,
	});
}

/** Whether the provider can generate right now (configured + key present). */
export function isProviderReady(status: ProviderStatus | undefined): boolean {
	if (!status) return false;
	const { config, api_key_set } = status;
	const needsKey = config.preset !== "ollama" && config.preset !== "lmstudio";
	return Boolean(config.base_url) && Boolean(config.model) && (!needsKey || api_key_set);
}
