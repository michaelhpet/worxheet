import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

import type { ProviderSettings } from "./provider";

export type ThemePreference = "dark" | "light" | "system";

export interface AppearanceSettings {
	theme: ThemePreference;
}

export interface ArtifactSettings {
	temperature: number;
	max_tokens: number;
	seed: number | null;
}

export interface AppSettings {
	appearance: AppearanceSettings;
	provider: ProviderSettings;
	artifacts: ArtifactSettings;
}

export interface AppearanceSettingsPatch {
	theme?: ThemePreference;
}

export interface ArtifactSettingsPatch {
	temperature?: number;
	max_tokens?: number;
	seed?: number | null;
}

export interface UpdateSettingsInput {
	appearance?: AppearanceSettingsPatch;
	artifacts?: ArtifactSettingsPatch;
}

export const SETTINGS_QUERY_KEY = "SETTINGS";

export function useSettings() {
	return useQuery({
		queryKey: [SETTINGS_QUERY_KEY],
		queryFn: () => invoke<AppSettings>("get_settings"),
	});
}

export function useUpdateSettings() {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: (input: UpdateSettingsInput) => invoke<AppSettings>("update_settings", { patch: input }),
		onMutate: (input) => {
			const previous = queryClient.getQueryData<AppSettings>([SETTINGS_QUERY_KEY]);
			queryClient.setQueryData<AppSettings>([SETTINGS_QUERY_KEY], (current) => applyPatch(current, input));
			return { previous };
		},
		onError: (_error, _input, context) => {
			if (context?.previous) {
				queryClient.setQueryData([SETTINGS_QUERY_KEY], context.previous);
			}
		},
		onSettled: () => {
			queryClient.invalidateQueries({ queryKey: [SETTINGS_QUERY_KEY] });
		},
	});
}

function applyPatch(current: AppSettings | undefined, input: UpdateSettingsInput): AppSettings | undefined {
	if (!current) {
		return current;
	}

	return {
		...current,
		appearance: input.appearance ? { theme: input.appearance.theme ?? current.appearance.theme } : current.appearance,
		artifacts: input.artifacts
			? {
					temperature: input.artifacts.temperature ?? current.artifacts.temperature,
					max_tokens: input.artifacts.max_tokens ?? current.artifacts.max_tokens,
					seed: input.artifacts.seed !== undefined ? input.artifacts.seed : current.artifacts.seed,
				}
			: current.artifacts,
	};
}
