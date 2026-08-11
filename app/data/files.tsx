import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

import { WORKSHEETS_QUERY_KEY } from "./worksheets";

export type FileStatus = "uploaded" | "parsing" | "parsed";

export interface FileInfo {
	id: string;
	name: string;
	extension: string;
	size: number;
	status: FileStatus;
}

export const FILES_QUERY_KEY = "FILES";

export function useFiles(worksheetId: string) {
	return useQuery({
		queryKey: [FILES_QUERY_KEY, worksheetId],
		queryFn: () => invoke<FileInfo[]>("get_files", { worksheetId }),
	});
}

export function useProcessFiles(worksheetId: string) {
	const queryClient = useQueryClient();

	return useMutation({
		mutationFn: (fileIds: string[]) =>
			invoke("process_files", { worksheetId, fileIds }),
		onSuccess: () => {
			queryClient.invalidateQueries({
				queryKey: [FILES_QUERY_KEY, worksheetId],
			});
			queryClient.invalidateQueries({
				queryKey: [WORKSHEETS_QUERY_KEY, worksheetId],
			});
		},
	});
}
