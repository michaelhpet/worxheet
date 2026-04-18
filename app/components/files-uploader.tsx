import { IconFiles } from "@tabler/icons-react";
import { listen, TauriEvent } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { type DragEvent, useEffect, useEffectEvent, useState } from "react";
import { Button } from "./ui/button";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "./ui/empty";

interface FilesUploaderProps {
	onFiles: (files: string[]) => void;
}

export function FilesUploader(props: FilesUploaderProps) {
	const [dragging, setDragging] = useState(false);

	const findFiles = async () => {
		const files = await open({ multiple: true });
		if (files) {
			props.onFiles(files);
		}
	};

	const dropFiles = useEffectEvent(async () => {
		return listen<{ paths: string[] }>(TauriEvent.DRAG_DROP, (event) => {
			props.onFiles(event.payload.paths);
		});
	});

	const onDragOver = (e: DragEvent<HTMLDivElement>) => {
		e.preventDefault();
		setDragging(true);
	};

	const onDragLeave = (e: DragEvent<HTMLDivElement>) => {
		e.preventDefault();
		setDragging(false);
	};

	useEffect(() => {
		const unlisten = dropFiles();
		return () => {
			(async () => unlisten)();
		};
	}, []);

	return (
		<Empty
			className={clsx(
				"relative w-full h-full border-0 border-dashed rounded-none",
				dragging && "bg-card/80 border-2",
			)}
			onDragOver={onDragOver}
			onDragLeave={onDragLeave}
		>
			<EmptyHeader>
				<EmptyMedia variant="icon" className="size-16 bg-card">
					<IconFiles className="size-12" />
				</EmptyMedia>
				<EmptyTitle className="text-2xl">Start a new worxheet</EmptyTitle>
				<EmptyDescription>
					Start a new worxheet by dragging files here or click to find files.
				</EmptyDescription>
			</EmptyHeader>
			<EmptyContent>
				<Button variant="secondary" onClick={findFiles}>
					Find files
				</Button>
			</EmptyContent>
		</Empty>
	);
}
