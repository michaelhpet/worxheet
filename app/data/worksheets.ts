import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import type { ArtifactType, Paginated, QuizArtifactType } from "@/lib/types";

export interface Worksheet {
	id: string;
	name: string;
	created_at: string;
	updated_at: string;
	pipeline_status: "idle" | "running" | "done" | "failed" | "cancelled";
	pipeline_error?: string | null;
	file_count: number;
	file_extensions: string[];
	artifact_counts?: Partial<Record<ArtifactType, number>>;
	quiz_counts?: Record<QuizArtifactType, number>;
}

export const WORKSHEETS_QUERY_KEY = "WORKSHEETS";

export function useWorksheets(page?: number, perPage?: number) {
	return useQuery({
		queryKey: [WORKSHEETS_QUERY_KEY, { page, perPage }],
		queryFn: () => invoke<Paginated<Worksheet>>("get_worksheets", { page, perPage }),
		refetchInterval: (query) => {
			const anyRunning = query.state.data?.items.some((worksheet) => worksheet.pipeline_status === "running");
			return anyRunning ? 1500 : false;
		},
	});
}

export function useWorksheet(id: string, pipelineStatus?: string) {
	return useQuery({
		queryKey: [WORKSHEETS_QUERY_KEY, id],
		queryFn: () => invoke<Worksheet>("get_worksheet", { id }),
		refetchInterval: pipelineStatus === "running" ? 1500 : false,
	});
}

export function useDeleteWorksheet() {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: (id: string) => invoke("delete_worksheet", { id }),
		onSuccess: () => {
			queryClient.invalidateQueries({ queryKey: [WORKSHEETS_QUERY_KEY] });
		},
	});
}
