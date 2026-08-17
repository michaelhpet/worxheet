import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

import type { ArtifactType } from "@/lib/artifact-types";

export interface Artifact {
	id: string;
	worksheet_id: string;
	artifact_type: ArtifactType;
	source: string;
	content: string;
}

export const ARTIFACTS_QUERY_KEY = "ARTIFACTS";

export function useArtifacts(worksheetId: string) {
	return useQuery({
		queryKey: [ARTIFACTS_QUERY_KEY, worksheetId],
		queryFn: () => invoke<Artifact[]>("get_artifacts", { worksheetId }),
	});
}
