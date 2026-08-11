import {
	IconFile,
	IconFilePlus,
	IconLoader,
	IconRefresh,
} from "@tabler/icons-react";

import { useFiles, useProcessFiles, type FileStatus } from "@/data/files";
import {
	formatDownloadProgress,
	modelDownloadLabel,
	useModelDownload,
} from "@/data/model-downloads";
import { usePipelineProgress } from "@/data/progress";
import { FILE_TYPES } from "@/lib/constants";
import { toErrorMessage } from "@/lib/errors";
import { cn, formatBytes } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	Card,
	CardContent,
	CardFooter,
	CardHeader,
	CardTitle,
} from "@/components/ui/card";
import {
	Item,
	ItemActions,
	ItemContent,
	ItemDescription,
	ItemMedia,
	ItemTitle,
} from "@/components/ui/item";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";

interface FilesPanelProps {
	worksheetId: string;
}

const STATUS_BADGE: Record<
	FileStatus,
	{
		label: string;
		variant: "default" | "secondary" | "outline";
		className?: string;
	}
> = {
	uploaded: { label: "Uploaded", variant: "secondary" },
	parsing: {
		label: "Parsing…",
		variant: "outline",
		className: "animate-pulse",
	},
	parsed: { label: "Parsed", variant: "default" },
};

export function FilesPanel({ worksheetId }: FilesPanelProps) {
	const { data: files, isLoading } = useFiles(worksheetId);
	const processFiles = useProcessFiles(worksheetId);
	const progress = usePipelineProgress("ingestion-progress", worksheetId);
	const download = useModelDownload();

	const allFiles = files ?? [];
	const unparsed = allFiles.filter((file) => file.status !== "parsed");
	const processing = processFiles.isPending;
	const percent =
		progress && progress.total > 0
			? Math.round((progress.done / progress.total) * 100)
			: 0;

	const handleProcess = () => {
		const ids = unparsed.map((file) => file.id);
		if (ids.length > 0) {
			processFiles.mutate(ids);
		}
	};

	return (
		<Card>
			<CardHeader>
				<CardTitle>Files</CardTitle>
			</CardHeader>
			<CardContent className="gap-2">
				{isLoading ? (
					<div className="flex flex-col gap-2">
						<Skeleton className="h-10" />
						<Skeleton className="h-10" />
					</div>
				) : allFiles.length === 0 ? (
					<p className="text-sm text-muted-foreground">
						No files attached to this worksheet.
					</p>
				) : (
					<ul className="flex flex-col">
						{allFiles.map((file) => {
							const fileType = FILE_TYPES[file.extension] || {
								icon: IconFile,
								class: "text-muted-foreground",
							};
							const status = STATUS_BADGE[file.status] ?? STATUS_BADGE.uploaded;
							return (
								<Item
									key={file.id}
									size="sm"
									className="rounded-none border-b border-border last:border-b-0"
								>
									<ItemMedia variant="icon">
										<fileType.icon className={cn("size-4", fileType.class)} />
									</ItemMedia>
									<ItemContent>
										<ItemTitle className="break-all">{file.name}</ItemTitle>
										<ItemDescription>{formatBytes(file.size)}</ItemDescription>
									</ItemContent>
									<ItemActions>
										<Badge
											variant={status.variant}
											className={status.className}
										>
											{status.label}
										</Badge>
									</ItemActions>
								</Item>
							);
						})}
					</ul>
				)}
			</CardContent>
			<CardFooter className="flex-col items-stretch gap-2">
				{processing && download?.active && (
					<div className="flex flex-col gap-1">
						<div className="flex items-center justify-between text-xs text-muted-foreground">
							<span className="flex items-center gap-1.5">
								<IconLoader className="size-3.5 animate-spin" />
								Downloading {modelDownloadLabel(download.kind)}
							</span>
							<span className="tabular-nums">
								{formatDownloadProgress(download.done, download.total)}
							</span>
						</div>
						<Progress
							value={
								download.total > 0
									? Math.round((download.done / download.total) * 100)
									: 0
							}
						/>
					</div>
				)}
				{processing && !download?.active && (
					<div className="flex flex-col gap-1">
						<div className="flex items-center justify-between text-xs text-muted-foreground">
							<span className="flex items-center gap-1.5">
								<IconLoader className="size-3.5 animate-spin" />
								Processing files
							</span>
							<span className="tabular-nums">
								{progress?.done ?? 0}/{progress?.total ?? 0}
							</span>
						</div>
						<Progress value={percent} />
					</div>
				)}
				{unparsed.length > 0 ? (
					<Button
						variant="secondary"
						disabled={processing}
						onClick={handleProcess}
					>
						{processing ? (
							<IconLoader className="animate-spin" />
						) : (
							<IconFilePlus />
						)}
						Process {unparsed.length} {unparsed.length === 1 ? "file" : "files"}
					</Button>
				) : (
					<Button variant="ghost" size="sm" disabled={allFiles.length === 0}>
						<IconRefresh />
						All files processed
					</Button>
				)}
				{processFiles.error && (
					<p className="text-sm text-destructive">
						{toErrorMessage(processFiles.error)}
					</p>
				)}
			</CardFooter>
		</Card>
	);
}
