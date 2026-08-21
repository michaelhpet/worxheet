import { useQuery } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

interface UseFileMetadataParams {
	path: string;
}

export interface FileMetadata {
	name: string;
	size: number;
	extension: string;
}

export const FILE_METADATA_QUERY_KEY = "FILE_METADATA";

export function useFileMetadata(params: UseFileMetadataParams) {
	return useQuery({
		queryKey: [FILE_METADATA_QUERY_KEY, params],
		queryFn: () => invoke<FileMetadata>("get_file_metadata", { path: params.path }),
	});
}
