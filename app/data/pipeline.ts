import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { useEffect } from "react";
import { WORKSHEETS_QUERY_KEY } from "@/data/worksheets";

export interface TypeProgress {
	artifact_type: string;
	done: number;
	total: number;
}

export interface PipelineStatus {
	status: "idle" | "running" | "done" | "failed" | "cancelled";
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

/** Payload of the `pipeline-progress` Tauri event emitted by the Rust core. */
export interface PipelineProgressEvent {
	worksheet_id: string;
	status: "idle" | "running" | "done" | "failed" | "cancelled";
	phase: string | null;
	artifact_type: string | null;
	done: number;
	total: number;
	types: TypeProgress[];
	types_done: number;
	types_total: number;
	requests_done?: number;
	tokens_in?: number;
	tokens_out?: number;
	error: string | null;
}

/**
 * Subscribes to the backend `pipeline-progress` events so queries can refresh
 * when a pipeline reaches a terminal state. Progress ticks (`running`) are
 * ignored: the persisted worksheet status only changes at completion.
 */
export function usePipelineProgressListener() {
	const queryClient = useQueryClient();

	useEffect(() => {
		const unlisten = listen<PipelineProgressEvent>("pipeline-progress", (event) => {
			if (event.payload.status === "running") {
				return;
			}
			queryClient.invalidateQueries({ queryKey: [WORKSHEETS_QUERY_KEY] });
			queryClient.invalidateQueries({ queryKey: [PIPELINE_QUERY_KEY, event.payload.worksheet_id] });
		});
		return () => {
			unlisten.then((fn) => fn());
		};
	}, [queryClient]);
}

/**
 * Polls `get_pipeline_status` for a worksheet while its pipeline is running.
 * Stops polling once the pipeline reaches a terminal state
 * (`done`/`failed`/`cancelled`).
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

export function useRetryPipeline(worksheetId: string) {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: () => invoke<void>("retry_pipeline", { worksheetId }),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: [PIPELINE_QUERY_KEY, worksheetId] });
			queryClient.invalidateQueries({ queryKey: [WORKSHEETS_QUERY_KEY] });
		},
	});
}

export function useStopPipeline(worksheetId: string) {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: () => invoke<boolean>("stop_pipeline", { worksheetId }),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: [PIPELINE_QUERY_KEY, worksheetId] });
			queryClient.invalidateQueries({ queryKey: [WORKSHEETS_QUERY_KEY] });
		},
	});
}
