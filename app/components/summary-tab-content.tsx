import { ARTIFACTS_QUERY_KEY, useArtifacts } from "@/data/artifacts";
import { usePipelineStatus } from "@/data/pipeline";
import type { Worksheet } from "@/data/worksheets";
import type { ArtifactTypeOption, SummaryContent } from "@/lib/types";
import { useQueryClient } from "@tanstack/react-query";
import { IconAlertTriangle } from "@tabler/icons-react";
import { useEffect, useMemo } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { Alert, AlertDescription, AlertTitle } from "./ui/alert";
import { Badge } from "./ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "./ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "./ui/empty";
import { Separator } from "./ui/separator";
import { Skeleton } from "./ui/skeleton";
import { Spinner } from "./ui/spinner";
import { TabsContent } from "./ui/tabs";

interface Props {
	worksheet: Worksheet;
	artifactType: ArtifactTypeOption;
}

function parseSummary(raw: string): SummaryContent | null {
	try {
		const value: unknown = JSON.parse(raw);
		if (typeof value !== "object" || value === null) return null;
		const record = value as Record<string, unknown>;
		if (typeof record.title !== "string" || typeof record.summary !== "string") return null;
		if (!Array.isArray(record.key_points) || !record.key_points.every((point) => typeof point === "string")) {
			return null;
		}
		return { title: record.title, summary: record.summary, key_points: record.key_points };
	} catch {
		return null;
	}
}

export function SummaryTabContent({ worksheet, artifactType }: Props) {
	const queryClient = useQueryClient();
	const { data: artifacts, isPending } = useArtifacts(worksheet.id, "Summary");
	const { data: pipelineStatus } = usePipelineStatus(worksheet.id);

	useEffect(() => {
		if (pipelineStatus?.status === "done") {
			queryClient.invalidateQueries({ queryKey: [ARTIFACTS_QUERY_KEY, worksheet.id, "Summary"] });
		}
	}, [pipelineStatus?.status, queryClient, worksheet.id]);

	const summary = useMemo(() => {
		if (!artifacts || artifacts.length === 0) return null;
		return parseSummary(artifacts[0].content);
	}, [artifacts]);

	const malformed = artifacts && artifacts.length > 0 && summary === null;

	const generating = useMemo(() => {
		if (pipelineStatus?.status !== "running") return false;
		if (pipelineStatus.phase === "ingesting") return true;
		if (pipelineStatus.phase !== "generating") return false;
		const progress = pipelineStatus.types?.find((t) => t.artifact_type === "Summary");
		if (!progress) return (artifacts?.length ?? 0) === 0;
		return progress.done < progress.total;
	}, [pipelineStatus, artifacts]);

	return (
		<TabsContent key={artifactType.value} value={artifactType.value} className="grow-0">
			<div className="mx-auto flex w-full max-w-3xl flex-col gap-4 px-4 py-6">
				{isPending ? (
					<div className="flex flex-col gap-3">
						<Skeleton className="h-8 w-2/3" />
						<Skeleton className="h-40 w-full" />
						<Skeleton className="h-20 w-full" />
					</div>
				) : malformed ? (
					<Alert variant="destructive">
						<IconAlertTriangle />
						<AlertTitle>Couldn&apos;t display this summary</AlertTitle>
						<AlertDescription>
							The stored summary isn&apos;t valid JSON. Re-run the pipeline to regenerate it.
						</AlertDescription>
					</Alert>
				) : !summary ? (
					<Empty>
						<EmptyHeader>
							<EmptyMedia variant="icon">
								<artifactType.icon />
							</EmptyMedia>
							<EmptyTitle>
								{artifactType.label}&nbsp;
								<Badge variant="secondary">
									{generating && <Spinner />}
									{generating ? "Generating" : "None yet"}
								</Badge>
							</EmptyTitle>
							<EmptyDescription>{artifactType.description}</EmptyDescription>
						</EmptyHeader>
					</Empty>
				) : (
					<Card>
						<CardHeader>
							<CardTitle>{summary.title}</CardTitle>
							<CardDescription>{artifactType.description}</CardDescription>
						</CardHeader>
						<CardContent>
							<div className="typeset typeset-docs">
								<ReactMarkdown remarkPlugins={[remarkGfm]}>{summary.summary}</ReactMarkdown>
							</div>
							{summary.key_points.length > 0 && (
								<>
									<Separator />
									<div className="flex flex-col gap-2">
										<p className="font-medium text-sm">Key points</p>
										<ul className="flex list-disc flex-col gap-1 pl-6 text-sm text-muted-foreground">
											{summary.key_points.map((point) => (
												<li key={point}>{point}</li>
											))}
										</ul>
									</div>
								</>
							)}
						</CardContent>
					</Card>
				)}
			</div>
		</TabsContent>
	);
}
