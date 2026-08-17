import {
	IconAlertTriangle,
	IconFileSearch,
	IconLoader,
} from "@tabler/icons-react";
import { useState } from "react";

import { useArtifacts } from "@/data/artifacts";
import {
	formatDownloadProgress,
	modelDownloadLabel,
	useModelDownload,
	type ModelDownload,
} from "@/data/model-downloads";
import { usePipelineStatus, type PipelineStatus } from "@/data/pipeline";
import { ARTIFACT_TYPE_OPTIONS, type ArtifactType } from "@/lib/artifact-types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
	Empty,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import { ArtifactCard } from "./artifact-card";

type Filter = "all" | ArtifactType;

const FILTER_OPTIONS: { value: Filter; label: string }[] = [
	{ value: "all", label: "All" },
	...ARTIFACT_TYPE_OPTIONS.map((option) => ({
		value: option.value as Filter,
		label: option.label,
	})),
];

interface ArtifactsPanelProps {
	worksheetId: string;
}

export function ArtifactsPanel({ worksheetId }: ArtifactsPanelProps) {
	const { data: artifacts, isLoading } = useArtifacts(worksheetId);
	const { data: status } = usePipelineStatus(worksheetId);
	const download = useModelDownload();
	const [filter, setFilter] = useState<Filter>("all");

	const allArtifacts = artifacts ?? [];
	const running = status?.status === "running";
	const failed = status?.status === "failed";
	const done = status?.status === "done";
	const visible =
		filter === "all"
			? allArtifacts
			: allArtifacts.filter((artifact) => artifact.artifact_type === filter);

	const countFor = (value: Filter) =>
		value === "all"
			? allArtifacts.length
			: allArtifacts.filter((artifact) => artifact.artifact_type === value)
					.length;

	return (
		<Card>
			<CardHeader>
				<CardTitle>Generated artifacts</CardTitle>
			</CardHeader>
			<CardContent className="gap-4">
				<StatusBanner status={status} download={download} />

				{running && <ProgressBar status={status} />}

				{failed && (
					<div className="flex items-start gap-2 rounded-lg border border-destructive/30 bg-destructive/5 p-3">
						<IconAlertTriangle className="mt-0.5 size-4 shrink-0 text-destructive" />
						<p className="text-sm text-destructive">
							{status?.error ?? "Pipeline failed"}
						</p>
					</div>
				)}

				<div className="flex flex-wrap items-center gap-1 rounded-lg border p-0.5">
					{FILTER_OPTIONS.map((option) => (
						<Button
							key={option.value}
							variant={filter === option.value ? "default" : "ghost"}
							size="sm"
							onClick={() => setFilter(option.value)}
						>
							{option.label}
							<Badge
								variant={filter === option.value ? "secondary" : "outline"}
							>
								{countFor(option.value)}
							</Badge>
						</Button>
					))}
				</div>

				{isLoading ? (
					<div className="flex flex-col gap-3">
						<Skeleton className="h-28" />
						<Skeleton className="h-28" />
					</div>
				) : visible.length === 0 ? (
					<Empty className="border-0">
						<EmptyHeader>
							<EmptyMedia variant="icon" className="size-12 bg-card">
								<IconFileSearch className="size-7" />
							</EmptyMedia>
							<EmptyTitle>
								{failed
									? "Generation failed"
									: running
										? "Generating artifacts"
										: done
											? "No artifacts yet"
											: "No artifacts yet"}
							</EmptyTitle>
							<EmptyDescription>
								{failed
									? "Fix the reported error and create the worksheet again."
									: running
										? "The worksheet is still being processed."
										: "Artifacts will appear here once the worksheet is processed."}
							</EmptyDescription>
						</EmptyHeader>
					</Empty>
				) : (
					<div className="flex flex-col gap-3">
						{visible.map((artifact) => (
							<ArtifactCard key={artifact.id} artifact={artifact} />
						))}
					</div>
				)}
			</CardContent>
		</Card>
	);
}

function StatusBanner({
	status,
	download,
}: {
	status?: PipelineStatus;
	download: ModelDownload | null;
}) {
	if (!status || status.status !== "running") return null;

	if (download?.active) {
		return (
			<div className="flex items-center gap-2 text-sm text-muted-foreground">
				<IconLoader className="size-4 animate-spin" />
				Downloading {modelDownloadLabel(download.kind)}
				<span className="tabular-nums">
					{formatDownloadProgress(download.done, download.total)}
				</span>
			</div>
		);
	}

	if (status.phase === "generating") {
		const label = ARTIFACT_TYPE_OPTIONS.find(
			(option) => option.value === status.artifact_type,
		)?.label;
		return (
			<div className="flex items-center gap-2 text-sm text-muted-foreground">
				<IconLoader className="size-4 animate-spin" />
				Generating {label ?? status.artifact_type}
			</div>
		);
	}

	return (
		<div className="flex items-center gap-2 text-sm text-muted-foreground">
			<IconLoader className="size-4 animate-spin" />
			Processing files
		</div>
	);
}

function ProgressBar({ status }: { status: PipelineStatus }) {
	if (status.phase === "generating") {
		const percent =
			status.total > 0 ? Math.round((status.done / status.total) * 100) : 0;
		return (
			<div className="flex flex-col gap-1">
				<div className="flex items-center justify-between text-xs text-muted-foreground">
					<span>
						Type {Math.min(status.types_done + 1, status.types_total)}/
						{status.types_total}
					</span>
					<span className="tabular-nums">
						{status.done}/{status.total}
					</span>
				</div>
				<Progress value={percent} />
			</div>
		);
	}

	const percent =
		status.total > 0 ? Math.round((status.done / status.total) * 100) : 0;
	return (
		<div className="flex flex-col gap-1">
			<div className="flex items-center justify-between text-xs text-muted-foreground">
				<span>Files</span>
				<span className="tabular-nums">
					{status.done}/{status.total}
				</span>
			</div>
			<Progress value={percent} />
		</div>
	);
}
