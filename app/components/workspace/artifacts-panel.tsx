import { IconFileSearch, IconSparkles } from "@tabler/icons-react";
import { useState } from "react";

import { useArtifacts } from "@/data/artifacts";
import { useFiles } from "@/data/files";
import { ARTIFACT_TYPE_OPTIONS, type ArtifactType } from "@/lib/artifact-types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@/components/ui/empty";
import { Skeleton } from "@/components/ui/skeleton";
import { ArtifactCard } from "./artifact-card";
import { GenerateDialog } from "./generate-dialog";

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
	const { data: files } = useFiles(worksheetId);
	const [filter, setFilter] = useState<Filter>("all");
	const [dialogOpen, setDialogOpen] = useState(false);

	const allArtifacts = artifacts ?? [];
	const hasParsedFiles = (files ?? []).some((file) => file.status === "parsed");
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
				<CardTitle className="flex items-center justify-between">
					<span>Generated artifacts</span>
					<Button
						size="sm"
						disabled={!hasParsedFiles}
						onClick={() => setDialogOpen(true)}
					>
						<IconSparkles />
						Generate
					</Button>
				</CardTitle>
			</CardHeader>
			<CardContent className="gap-4">
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
								{hasParsedFiles ? "No artifacts yet" : "No files processed"}
							</EmptyTitle>
							<EmptyDescription>
								{hasParsedFiles
									? "Generate your first artifact to get started."
									: "Process the worksheet's files before generating artifacts."}
							</EmptyDescription>
						</EmptyHeader>
						{hasParsedFiles && (
							<EmptyContent>
								<Button variant="secondary" onClick={() => setDialogOpen(true)}>
									<IconSparkles />
									Generate
								</Button>
							</EmptyContent>
						)}
					</Empty>
				) : (
					<div className="flex flex-col gap-3">
						{visible.map((artifact) => (
							<ArtifactCard key={artifact.id} artifact={artifact} />
						))}
					</div>
				)}
			</CardContent>

			<GenerateDialog
				worksheetId={worksheetId}
				open={dialogOpen}
				onOpenChange={setDialogOpen}
			/>
		</Card>
	);
}
