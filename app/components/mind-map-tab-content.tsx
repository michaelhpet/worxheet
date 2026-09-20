import { ARTIFACTS_QUERY_KEY, useArtifacts } from "@/data/artifacts";
import { usePipelineStatus } from "@/data/pipeline";
import type { Worksheet } from "@/data/worksheets";
import type { ArtifactTypeOption, MindMapContent, MindMapNode } from "@/lib/types";
import { IconAlertTriangle } from "@tabler/icons-react";
import { useQueryClient } from "@tanstack/react-query";
import { Markmap } from "markmap-view";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Alert, AlertDescription, AlertTitle } from "./ui/alert";
import { Badge } from "./ui/badge";
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

interface PureNode {
	content: string;
	children: PureNode[];
}

function escapeHtml(text: string): string {
	return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function toPureNode(node: MindMapNode): PureNode {
	return {
		content: escapeHtml(node.label.trim() || "Untitled"),
		children: node.children.map(toPureNode),
	};
}

export function mindMapToTree(content: MindMapContent): PureNode {
	return {
		content: escapeHtml(content.topic.trim() || "Untitled"),
		children: content.branches.map(toPureNode),
	};
}

export function MindMapTabContent({ worksheet, artifactType }: Props) {
	const queryClient = useQueryClient();
	const { data: artifacts, isPending } = useArtifacts(worksheet.id, "MindMap");
	const { data: pipelineStatus } = usePipelineStatus(worksheet.id);
	const svgRef = useRef<SVGSVGElement | null>(null);
	const mmRef = useRef<Markmap | null>(null);
	const [settled, setSettled] = useState(false);
	const [visible, setVisible] = useState(true);
	const visibleRef = useRef(true);
	const settledRef = useRef(settled);
	settledRef.current = settled;
	const roRef = useRef<ResizeObserver | null>(null);
	const [renderError, setRenderError] = useState<string | null>(null);

	// Attaches exactly when the node attaches (and detaches on unmount), so no
	// effect dependencies are needed to track the element's lifetime.
	const attachSvg = useCallback((node: SVGSVGElement | null) => {
		svgRef.current = node;
		roRef.current?.disconnect();
		roRef.current = null;
		if (!node || typeof ResizeObserver === "undefined") return;
		const ro = new ResizeObserver((entries) => {
			const rect = entries[0]?.contentRect;
			const shown = !!rect && rect.width > 0 && rect.height > 0;
			if (shown === visibleRef.current) return;
			visibleRef.current = shown;
			setVisible(shown);
			const mm = mmRef.current;
			if (shown && mm && settledRef.current) {
				mm.setOptions({ duration: 0 });
				mm.fit().then(() => mmRef.current?.setOptions({ duration: 500 }));
			}
		});
		ro.observe(node);
		roRef.current = ro;
	}, []);

	useEffect(() => {
		if (pipelineStatus?.status === "done") {
			queryClient.invalidateQueries({ queryKey: [ARTIFACTS_QUERY_KEY, worksheet.id, "MindMap"] });
		}
	}, [pipelineStatus?.status, queryClient, worksheet.id]);

	const mindMap = useMemo(() => {
		if (!artifacts || artifacts.length === 0) return null;
		return parseMindMap(artifacts[0].content);
	}, [artifacts]);

	const data = useMemo(() => (mindMap ? mindMapToTree(mindMap) : null), [mindMap]);

	useEffect(() => {
		if (!svgRef.current || mmRef.current || !data) return;
		const mm = Markmap.create(svgRef.current, { autoFit: true, initialExpandLevel: 2, duration: 0 });
		mmRef.current = mm;
		let cancelled = false;
		(async () => {
			try {
				setSettled(false);
				setRenderError(null);
				await mm.setData(data);
				if (cancelled) return;
				await mm.fit();
				if (cancelled) return;
				setSettled(true);
				mm.setOptions({ duration: 500 });
			} catch (error) {
				setRenderError(error instanceof Error ? error.message : String(error));
				setSettled(true);
			}
		})();
		return () => {
			cancelled = true;
			mmRef.current?.destroy();
			mmRef.current = null;
		};
	}, [data]);

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
		<TabsContent
			key={artifactType.value}
			value={artifactType.value}
			keepMounted
			className="flex flex-1 flex-col select-none"
		>
			<div className="mx-auto flex w-full flex-1 flex-col px-4 py-6">
				{isPending ? (
					<div className="flex flex-col gap-3">
						<Skeleton className="h-8 w-2/3" />
						<Skeleton className="h-[60vh] w-full" />
					</div>
				) : malformed || renderError ? (
					<Alert variant="destructive" className="max-w-5xl mx-auto">
						<IconAlertTriangle />
						<AlertTitle>Couldn&apos;t render this mind map</AlertTitle>
						<AlertDescription>
							{renderError ?? "The stored mind map isn't valid JSON. Re-run the pipeline to regenerate it."}
						</AlertDescription>
					</Alert>
				) : !mindMap ? (
					<Empty className="flex-none">
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
					<div className="flex flex-1 flex-col gap-3">
						<svg
							role="img"
							ref={attachSvg}
							className={`markmap min-h-[60vh] w-full flex-1 transition-opacity duration-150 ${settled && visible ? "opacity-100" : "opacity-0"}`}
							aria-label={`Mind map of ${mindMap.topic}`}
						/>
					</div>
				)}
			</div>
		</TabsContent>
	);
}
