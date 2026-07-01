import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import type { Paginated } from "@/lib/types";

export interface Worksheet {
	id: string;
	name: string;
	created_at: string;
	updated_at: string;
}

export const WORKSHEETS_QUERY_KEY = "WORKSHEETS";

export function useWorksheets(page?: number, perPage?: number) {
	return useQuery({
		queryKey: [WORKSHEETS_QUERY_KEY, { page, perPage }],
		queryFn: () =>
			invoke<Paginated<Worksheet>>("get_worksheets", { page, perPage }),
	});
}

export function useWorksheet(id: string) {
	return useQuery({
		queryKey: [WORKSHEETS_QUERY_KEY, id],
		queryFn: () => invoke<Worksheet>("get_worksheet", { id }),
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
