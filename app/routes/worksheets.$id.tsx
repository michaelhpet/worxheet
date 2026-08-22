import { QuizTabContent } from "@/components/quiz-tab-content";
import { Button } from "@/components/ui/button";
import { Item, ItemContent, ItemMedia, ItemTitle } from "@/components/ui/item";
import { Progress } from "@/components/ui/progress";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { usePipelineStatus } from "@/data/pipeline";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPE_OPTIONS, ARTIFACT_TYPES } from "@/lib/constants";
import type { ArtifactType } from "@/lib/types";
import { IconArrowLeft } from "@tabler/icons-react";
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

	if (isLoading || isStatusLoading) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-muted-foreground">Loading worksheet...</p>
			</main>
		);
	}

	if (error || !worksheet) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-destructive">Worksheet not found</p>
			</main>
		);
	}

	if (status && status.status === "running") {
		const artifactType = ARTIFACT_TYPE_OPTIONS.find((o) => o.value === status.artifact_type);
		return (
			<main className="w-screen h-screen flex flex-col">
				<header className="sticky top-0 w-full flex items-center justify-center gap-2 px-4 pb-4 bg-background">
					<Button size="icon" variant="secondary" onClick={() => navigate({ to: "/" })}>
						<IconArrowLeft />
					</Button>
				</header>
				<div className="grow w-full flex flex-col items-center justify-center">
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
			</main>
		);
	}

	return (
		<Tabs value={artifactType} onValueChange={setArtifactType}>
			<main className="w-screen flex flex-col">
				<header className="sticky top-0 w-full flex items-center justify-center gap-2 px-4 pb-4 bg-background">
					<Button size="icon" variant="secondary" onClick={() => navigate({ to: "/" })}>
						<IconArrowLeft />
					</Button>
					<TabsList>
						{ARTIFACT_TYPE_OPTIONS.map((type) => (
							<TabsTrigger key={type.value} value={type.value}>
								<type.icon />
								{type.label}
							</TabsTrigger>
						))}
					</TabsList>
				</header>
				{[ARTIFACT_TYPES.MultipleChoiceQuiz, ARTIFACT_TYPES.EssayQuiz, ARTIFACT_TYPES.CompletionQuiz].map((type) => {
					const artifactType = ARTIFACT_TYPE_OPTIONS.find((o) => o.value === type);

					if (!artifactType) return null;

					return <QuizTabContent key={type} worksheet={worksheet} artifactType={artifactType} />;
				})}
			</main>
		</Tabs>
	);
}
