import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

import { SETTINGS_QUERY_KEY } from "./settings";

export type ProviderPreset = "openai" | "gemini" | "ollama" | "lmstudio" | "custom";

export interface PerPresetConfig {
	base_url: string;
	model: string;
	concurrency: number;
	/** Send `reasoning_effort: "none"` so thinking models answer directly. */
	disable_thinking: boolean;
}

export interface ProviderStatus {
	preset: string;
	config: PerPresetConfig;
	/** Whether an API key is stored in the OS keychain (never the key itself). */
	api_key_set: boolean;
	/** Keychain key presence per preset, for immediate verify-on-select. */
	keys_set: Record<string, boolean>;
}

export interface ProviderSettings {
	active: string;
	presets: Record<string, PerPresetConfig>;
}

export const PROVIDER_QUERY_KEY = "PROVIDER";

export interface SetProviderConfigInput {
	preset: string;
	/** Tauri v2 maps camelCase JS args to snake_case Rust params. */
	baseUrl: string;
	model: string;
	concurrency: number;
	/** `undefined`/empty keeps the stored key; empty string clears it. */
	apiKey?: string;
	/** `undefined` keeps the stored value. */
	disableThinking?: boolean;
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
		onSuccess: (status, input) => {
			const prev = queryClient.getQueryData<ProviderStatus>([PROVIDER_QUERY_KEY]);
			queryClient.setQueryData([PROVIDER_QUERY_KEY], status);
			queryClient.invalidateQueries({ queryKey: [SETTINGS_QUERY_KEY] });
			// Re-ping reachability only when the provider identity actually
			// changed. The inference form auto-persists unchanged values while
			// open; invalidating unconditionally would re-ping in a loop.
			const keyChanged = input.apiKey !== undefined && input.apiKey.trim() !== "";
			const identityChanged =
				!prev ||
				prev.preset !== status.preset ||
				prev.config.base_url !== status.config.base_url ||
				prev.config.model !== status.config.model ||
				prev.api_key_set !== status.api_key_set ||
				keyChanged;
			if (identityChanged) {
				queryClient.invalidateQueries({ queryKey: [PROVIDER_QUERY_KEY, "models"] });
			}
		},
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

/** Whether a provider is configured enough to attempt generation (base URL + model). */
export function isProviderReady(status: ProviderStatus | undefined): boolean {
	if (!status) return false;
	return Boolean(status.config.base_url) && Boolean(status.config.model);
}
