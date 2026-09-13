import { IconFiles } from "@tabler/icons-react";
import { listen, TauriEvent } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useEffectEvent, type ComponentProps } from "react";

import { SUPPORTED_EXTENSIONS } from "@/lib/constants";
import { cn } from "@/lib/utils";
import { FileCard } from "./file-card";
import { Button } from "./ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "./ui/empty";

function isSupportedExtension(path: string): boolean {
	const extension = path.toLowerCase().split(".").pop() || "";
	return SUPPORTED_EXTENSIONS.includes(extension);
}

interface FilesUploaderProps {
	files: string[];
	onFiles: (files: string[]) => void;
	onRemoveFile: ComponentProps<typeof FileCard>["onRemove"];
	invalid?: boolean;
}

const DOCUMENT_FILTER = {
	name: "Documents",
	extensions: SUPPORTED_EXTENSIONS,
};

export function FilesUploader({ files, onFiles, onRemoveFile, invalid }: FilesUploaderProps) {
	const findFiles = async () => {
		const files = await open({
			multiple: true,
			filters: [DOCUMENT_FILTER],
		});
		if (files) {
			onFiles(files.filter(isSupportedExtension));
		}
	};

	const dropFiles = useEffectEvent(async () => {
		return listen<{ paths: string[] }>(TauriEvent.DRAG_DROP, (event) => {
			const supported = event.payload.paths.filter(isSupportedExtension);
			onFiles(supported);
		});
	});

	useEffect(() => {
		const unlisten = dropFiles();
		return () => {
			unlisten.then((fn) => fn());
		};
	}, []);

	if (files.length > 0) {
		return (
			<div className="flex flex-col h-80">
				{files.length > 0 && (
					<ul className="flex flex-col border rounded-lg mb-4 overflow-auto">
						{files.map((file) => (
							<FileCard key={file} path={file} onRemove={onRemoveFile} className="border-0 border-b last:border-b-0" />
						))}
					</ul>
				)}
				<div className="flex items-center justify-end">
					<Button variant="secondary" onClick={findFiles}>
						Add more files
					</Button>
				</div>
			</div>
		);
	}

	return (
		<Empty className={cn("relative w-full h-80 border-2 border-dashed rounded-sm", invalid && "border-destructive")}>
			<EmptyHeader>
				<EmptyMedia variant="icon" className="size-16 bg-card">
					<IconFiles className="size-12" />
				</EmptyMedia>
				<EmptyTitle className="text-2xl">Start a new worksheet</EmptyTitle>
				<EmptyDescription>Start a new worksheet by dragging files here or click to find files.</EmptyDescription>
			</EmptyHeader>
			<EmptyContent>
				<Button variant="secondary" onClick={findFiles}>
					Find files
				</Button>
			</EmptyContent>
		</Empty>
	);
}
