import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

import type { ArtifactType } from "@/lib/artifact-types";
import { WORKSHEETS_QUERY_KEY } from "./worksheets";

export interface Artifact {
	id: string;
	worksheet_id: string;
	artifact_type: ArtifactType;
	source: string;
	content: string;
}

export interface GenerationParams {
	temperature?: number;
	top_p?: number;
	max_tokens?: number;
	seed?: number;
}

export interface GenerateArtifactsArgs {
	artifactType: ArtifactType;
	params?: GenerationParams;
}

export const ARTIFACTS_QUERY_KEY = "ARTIFACTS";

export function useArtifacts(worksheetId: string) {
	return useQuery({
		queryKey: [ARTIFACTS_QUERY_KEY, worksheetId],
		queryFn: () => invoke<Artifact[]>("get_artifacts", { worksheetId }),
	});
}

export function useGenerateArtifacts(worksheetId: string) {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: (args: GenerateArtifactsArgs) =>
			invoke("generate_artifacts", {
				worksheetId,
				artifactType: args.artifactType,
				params: args.params,
			}),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: [ARTIFACTS_QUERY_KEY, worksheetId],
			});
			queryClient.invalidateQueries({
				queryKey: [WORKSHEETS_QUERY_KEY, worksheetId],
			});
		},
	});
}
