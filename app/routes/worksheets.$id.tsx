import { Layout } from "@/components/layout";
import { QuizTabContent } from "@/components/quiz-tab-content";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { usePipelineStatus } from "@/data/pipeline";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPE_OPTIONS, ARTIFACT_TYPES } from "@/lib/constants";
import type { ArtifactType } from "@/lib/types";
import { IconFaceIdError } from "@tabler/icons-react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";

export const Route = createFileRoute("/worksheets/$id")({
	component: WorksheetDetail,
});

function WorksheetDetail() {
	const { id } = Route.useParams();
	const navigate = useNavigate();
	const { data: worksheet, isLoading, error } = useWorksheet(id);
	const { data: status, isLoading: isStatusLoading } = usePipelineStatus(id);
	const [artifactType, setArtifactType] = useState<ArtifactType>("MultipleChoiceQuiz");

	const exitWorksheet = () => {
		navigate({ to: "/" });
	};

	if (isLoading || isStatusLoading) {
		return (
			<Layout onBack={exitWorksheet}>
				<div className="grow flex flex-col items-center mt-40">
					<Spinner />
					<p className="text-muted-foreground">Loading worksheet...</p>
				</div>
			</Layout>
		);
	}

	if (error || !worksheet) {
		return (
			<Layout onBack={exitWorksheet}>
				<div className="grow flex flex-col items-center mt-40">
					<IconFaceIdError className="size-12 text-muted-foreground" />
					<p className="text-muted-foreground">Worksheet not found</p>
				</div>
			</Layout>
		);
	}

	if (status && status.status === "running") {
		const artifactType = ARTIFACT_TYPE_OPTIONS.find((o) => o.value === status.artifact_type);
		return (
			<Layout onBack={exitWorksheet}>
				<div className="grow w-full flex flex-col items-center gap-3 mt-40">
					{/* LOTTIE animation here */}
					<Item variant="muted" className="max-w-100">
						<ItemMedia>
							<Spinner />
						</ItemMedia>
						<ItemContent>
							<ItemTitle className="line-clamp-1 capitalize">{status.phase ?? "Preparing"}...</ItemTitle>
						</ItemContent>
						<ItemContent className="min-w-30 flex-none justify-end">
							{!!artifactType && (
								<>
									<span className="text-sm text-muted-foreground tabular-nums">{artifactType.label}</span>
									<Progress value={status.done} max={status.total} />
								</>
							)}
						</ItemContent>
					</Item>
				</div>
			</Layout>
		);
	}

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
				{[ARTIFACT_TYPES.MultipleChoiceQuiz, ARTIFACT_TYPES.EssayQuiz, ARTIFACT_TYPES.CompletionQuiz].map((type) => {
					const artifactType = ARTIFACT_TYPE_OPTIONS.find((o) => o.value === type);

					if (!artifactType) return null;

					return <QuizTabContent key={type} worksheet={worksheet} artifactType={artifactType} />;
				})}
			</Layout>
		</Tabs>
	);
}
