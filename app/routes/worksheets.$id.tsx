import { Layout } from "@/components/layout";
import { QuizTabContent } from "@/components/quiz-tab-content";
import { Alert, AlertAction, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { usePipelineStatus, useRetryPipeline, useStopPipeline } from "@/data/pipeline";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPE_OPTIONS, ARTIFACT_TYPES } from "@/lib/constants";
import { toErrorMessage } from "@/lib/errors";
import type { ArtifactType } from "@/lib/types";
import { IconAlertTriangle, IconLoader, IconPlayerStop, IconRotateClockwise } from "@tabler/icons-react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";

export const Route = createFileRoute("/worksheets/$id")({
	component: WorksheetDetail,
});

const QUIZ_TYPES: ArtifactType[] = [
	ARTIFACT_TYPES.MultipleChoiceQuiz,
	ARTIFACT_TYPES.EssayQuiz,
	ARTIFACT_TYPES.CompletionQuiz,
];

function WorksheetStatusBadge({
	worksheetId,
	liveStatus,
	persistedStatus,
	phase,
}: {
	worksheetId: string;
	liveStatus?: "idle" | "running" | "done" | "failed" | "cancelled";
	persistedStatus?: string;
	phase: "ingesting" | "generating" | null;
}) {
	const stopPipeline = useStopPipeline(worksheetId);
	const retryPipeline = useRetryPipeline(worksheetId);
	const current = liveStatus ?? persistedStatus;

	if (!current) {
		return (
			<Badge variant="secondary" className="gap-1.5">
				<Spinner className="size-3" />
				Loading
			</Badge>
		);
	}

	if (current === "running") {
		return (
			<DropdownMenu>
				<DropdownMenuTrigger
					render={
						<Badge variant="secondary" className="gap-1.5 cursor-pointer">
							<Spinner className="size-3" />
							{phase === "ingesting" ? "Ingesting" : phase === "generating" ? "Generating" : "Running"}
						</Badge>
					}
				/>
				<DropdownMenuContent align="end">
					<DropdownMenuItem disabled={stopPipeline.isPending} onClick={() => stopPipeline.mutate()}>
						{stopPipeline.isPending ? <IconLoader className="animate-spin" /> : <IconPlayerStop />}
						Stop
					</DropdownMenuItem>
				</DropdownMenuContent>
			</DropdownMenu>
		);
	}

	if (current === "failed" || current === "cancelled") {
		const failed = current === "failed";
		return (
			<DropdownMenu>
				<DropdownMenuTrigger
					render={
						failed ? (
							<Badge variant="destructive" className="gap-1.5 cursor-pointer">
								<span className="size-1.5 rounded-full bg-destructive" />
								Failed
							</Badge>
						) : (
							<Badge variant="outline" className="gap-1.5 cursor-pointer">
								<span className="size-1.5 rounded-full bg-amber-500" />
								Cancelled
							</Badge>
						)
					}
				/>
				<DropdownMenuContent align="end">
					<DropdownMenuItem disabled={retryPipeline.isPending} onClick={() => retryPipeline.mutate()}>
						{retryPipeline.isPending ? <IconLoader className="animate-spin" /> : <IconRotateClockwise />}
						Retry
					</DropdownMenuItem>
				</DropdownMenuContent>
			</DropdownMenu>
		);
	}

	if (current === "done") {
		return (
			<Badge variant="outline" className="gap-1.5">
				<span className="size-1.5 rounded-full bg-green-500" />
				Done
			</Badge>
		);
	}

	return (
		<Badge variant="ghost" className="gap-1.5">
			<span className="size-1.5 rounded-full bg-green-500" />
			Ready
		</Badge>
	);
}

function WorksheetDetail() {
	const { id } = Route.useParams();
	const navigate = useNavigate();
	const { data: status } = usePipelineStatus(id);
	const { data: worksheet, error } = useWorksheet(id, status?.status);
	const { isPending: isRetryingPipeline, mutate: retryPipeline } = useRetryPipeline(id);
	const [artifactType, setArtifactType] = useState<ArtifactType>("MultipleChoiceQuiz");

	const exitWorksheet = () => {
		navigate({ to: "/" });
	};

	const errors = [
		error ? toErrorMessage(error) : null,
		status?.status === "failed" ? (status.error ?? "Pipeline failed.") : null,
	].filter((message): message is string => Boolean(message));

	return (
		<Tabs value={artifactType} onValueChange={setArtifactType}>
			<Layout
				onBack={exitWorksheet}
				header={
					<TabsList>
						{ARTIFACT_TYPE_OPTIONS.map((type) => (
							<TabsTrigger key={type.value} value={type.value}>
								<type.icon />
								{type.label}
							</TabsTrigger>
						))}
					</TabsList>
				}
				extra={
					<WorksheetStatusBadge
						worksheetId={id}
						liveStatus={status?.status}
						persistedStatus={worksheet?.pipeline_status}
						phase={status?.phase ?? null}
					/>
				}
			>
				{errors.length > 0 && (
					<div className="w-143 mx-auto">
						<Alert variant="destructive">
							<IconAlertTriangle />
							<AlertTitle>Something went wrong</AlertTitle>
							{errors.map((message) => (
								<AlertDescription key={message}>{message}</AlertDescription>
							))}
							{status?.status === "failed" && (
								<AlertAction>
									<Button variant="outline" size="xs" onClick={() => retryPipeline()} disabled={isRetryingPipeline}>
										Retry
									</Button>
								</AlertAction>
							)}
						</Alert>
					</div>
				)}
				{worksheet ? (
					QUIZ_TYPES.map((type) => {
						const quizType = ARTIFACT_TYPE_OPTIONS.find((option) => option.value === type);
						if (!quizType) return null;
						return <QuizTabContent key={type} worksheet={worksheet} artifactType={quizType} />;
					})
				) : (
					<div className="grow flex flex-col items-center mt-[calc((100vh-436px)/4)]">
						<Spinner />
						<p className="text-muted-foreground">Loading worksheet...</p>
					</div>
				)}
			</Layout>
		</Tabs>
	);
}
