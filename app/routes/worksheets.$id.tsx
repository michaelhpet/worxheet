import { Layout } from "@/components/layout";
import { QuizTabContent } from "@/components/quiz-tab-content";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { usePipelineStatus } from "@/data/pipeline";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPE_OPTIONS, ARTIFACT_TYPES } from "@/lib/constants";
import { toErrorMessage } from "@/lib/errors";
import type { ArtifactType } from "@/lib/types";
import { IconAlertTriangle } from "@tabler/icons-react";
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

function WorksheetDetail() {
	const { id } = Route.useParams();
	const navigate = useNavigate();
	const { data: status } = usePipelineStatus(id);
	const { data: worksheet, error } = useWorksheet(id, status?.status);
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
			>
				{errors.length > 0 && (
					<div className="px-4 pt-4">
						<Alert variant="destructive">
							<IconAlertTriangle />
							<AlertTitle>Something went wrong</AlertTitle>
							{errors.map((message) => (
								<AlertDescription key={message}>{message}</AlertDescription>
							))}
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
					<div className="grow flex flex-col items-center mt-40">
						<Spinner />
						<p className="text-muted-foreground">Loading worksheet...</p>
					</div>
				)}
			</Layout>
		</Tabs>
	);
}
