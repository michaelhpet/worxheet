import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

export interface TypeProgress {
	artifact_type: string;
	done: number;
	total: number;
}

export interface PipelineStatus {
	status: "idle" | "running" | "done" | "failed";
	phase: "ingesting" | "generating" | null;
	artifact_type: string | null;
	done: number;
	total: number;
	/** Per-artifact-type progress for the current phase. */
	types: TypeProgress[];
	types_done: number;
	types_total: number;
	/** Provider requests issued so far in this run. */
	requests_done?: number;
	/** Approximate prompt tokens observed (chars/4 heuristic). */
	tokens_in?: number;
	/** Approximate completion tokens observed. */
	tokens_out?: number;
	error: string | null;
}

export const PIPELINE_QUERY_KEY = "PIPELINE";

const POLL_INTERVAL_MS = 1200;

/**
 * Polls `get_pipeline_status` for a worksheet while its pipeline is running.
 * Stops polling once the pipeline reaches a terminal state (`done`/`failed`).
 */
export function usePipelineStatus(worksheetId: string) {
	return useQuery({
		queryKey: [PIPELINE_QUERY_KEY, worksheetId],
		queryFn: () => invoke<PipelineStatus>("get_pipeline_status", { worksheetId }),
		refetchInterval: (query) => {
			const status = query.state.data?.status;
			return status === "running" ? POLL_INTERVAL_MS : false;
		},
	});
}
