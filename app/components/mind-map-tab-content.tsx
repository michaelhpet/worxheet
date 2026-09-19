import { ARTIFACTS_QUERY_KEY, useArtifacts } from "@/data/artifacts";
import { usePipelineStatus } from "@/data/pipeline";
import type { Worksheet } from "@/data/worksheets";
import type { ArtifactTypeOption, MindMapContent, MindMapNode } from "@/lib/types";
import { useQueryClient } from "@tanstack/react-query";
import { IconAlertTriangle } from "@tabler/icons-react";
import { Transformer } from "markmap-lib";
import { Markmap } from "markmap-view";
import { useEffect, useMemo, useRef } from "react";
import { Alert, AlertDescription, AlertTitle } from "./ui/alert";
import { Badge } from "./ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "./ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "./ui/empty";
import { Skeleton } from "./ui/skeleton";
import { Spinner } from "./ui/spinner";
import { TabsContent } from "./ui/tabs";

interface Props {
	worksheet: Worksheet;
	artifactType: ArtifactTypeOption;
}

function parseMindMap(raw: string): MindMapContent | null {
	try {
		const value: unknown = JSON.parse(raw);
		if (typeof value !== "object" || value === null) return null;
		const record = value as Record<string, unknown>;
		if (typeof record.topic !== "string" || !Array.isArray(record.branches)) return null;
		const validNode = (node: unknown): node is MindMapNode => {
			if (typeof node !== "object" || node === null) return false;
			const entry = node as Record<string, unknown>;
			return typeof entry.label === "string" && Array.isArray(entry.children) && entry.children.every(validNode);
		};
		if (!record.branches.every(validNode)) return null;
		return { topic: record.topic, branches: record.branches };
	} catch {
		return null;
	}
}

/** Strip heading markers so a label can't break out of the map structure. */
function cleanLabel(label: string): string {
	return label.replace(/^#+\s*/, "").trim() || "Untitled";
}

function appendNode(lines: string[], node: MindMapNode, depth: number) {
	if (depth === 0) {
		lines.push(`## ${cleanLabel(node.label)}`);
	} else {
		lines.push(`${"  ".repeat(depth - 1)}- ${cleanLabel(node.label)}`);
	}
	for (const child of node.children) {
		appendNode(lines, child, depth + 1);
	}
}

export function mindMapToMarkdown(content: MindMapContent): string {
	const lines = [`# ${cleanLabel(content.topic)}`, ""];
	for (const branch of content.branches) {
		appendNode(lines, branch, 0);
	}
	return lines.join("\n");
}

const transformer = new Transformer();

export function MindMapTabContent({ worksheet, artifactType }: Props) {
	const queryClient = useQueryClient();
	const { data: artifacts, isPending } = useArtifacts(worksheet.id, "MindMap");
	const { data: pipelineStatus } = usePipelineStatus(worksheet.id);
	const svgRef = useRef<SVGSVGElement>(null);
	const mapRef = useRef<Markmap | null>(null);

	useEffect(() => {
		if (pipelineStatus?.status === "done") {
			queryClient.invalidateQueries({ queryKey: [ARTIFACTS_QUERY_KEY, worksheet.id, "MindMap"] });
		}
	}, [pipelineStatus?.status, queryClient, worksheet.id]);

	const mindMap = useMemo(() => {
		if (!artifacts || artifacts.length === 0) return null;
		return parseMindMap(artifacts[0].content);
	}, [artifacts]);

	const markdown = useMemo(() => (mindMap ? mindMapToMarkdown(mindMap) : null), [mindMap]);

	useEffect(() => {
		if (!markdown || !svgRef.current) return;
		const { root } = transformer.transform(markdown);
		if (mapRef.current) {
			mapRef.current.setData(root);
		} else {
			mapRef.current = Markmap.create(svgRef.current, undefined, root);
		}
		mapRef.current.fit();
	}, [markdown]);

	useEffect(() => {
		return () => {
			mapRef.current?.destroy();
			mapRef.current = null;
		};
	}, []);

	const malformed = artifacts && artifacts.length > 0 && mindMap === null;

	const generating = useMemo(() => {
		if (pipelineStatus?.status !== "running") return false;
		if (pipelineStatus.phase === "ingesting") return true;
		if (pipelineStatus.phase !== "generating") return false;
		const progress = pipelineStatus.types?.find((t) => t.artifact_type === "MindMap");
		if (!progress) return (artifacts?.length ?? 0) === 0;
		return progress.done < progress.total;
	}, [pipelineStatus, artifacts]);

	return (
		<TabsContent key={artifactType.value} value={artifactType.value} className="grow-0">
			<div className="mx-auto flex w-full max-w-5xl flex-col gap-4 px-4 py-6">
				{isPending ? (
					<div className="flex flex-col gap-3">
						<Skeleton className="h-8 w-2/3" />
						<Skeleton className="h-[60vh] w-full" />
					</div>
				) : malformed ? (
					<Alert variant="destructive">
						<IconAlertTriangle />
						<AlertTitle>Couldn&apos;t display this mind map</AlertTitle>
						<AlertDescription>
							The stored mind map isn&apos;t valid JSON. Re-run the pipeline to regenerate it.
						</AlertDescription>
					</Alert>
				) : !mindMap ? (
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
							<CardTitle>{mindMap.topic}</CardTitle>
							<CardDescription>{artifactType.description}</CardDescription>
						</CardHeader>
						<CardContent>
							<svg ref={svgRef} className="h-[60vh] w-full" role="img" aria-label={`Mind map of ${mindMap.topic}`} />
						</CardContent>
					</Card>
				)}
			</div>
		</TabsContent>
	);
}
