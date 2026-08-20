import { QuizTabContent } from "@/components/quiz-tab-content";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPE_OPTIONS, ARTIFACT_TYPES, type ArtifactType } from "@/lib/artifact-types";
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
	const [artifactType, setArtifactType] = useState<ArtifactType>("MultipleChoiceQuiz");

	if (isLoading) {
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

	return (
		<Tabs value={artifactType} onValueChange={setArtifactType}>
			<main className="w-screen h-screen flex flex-col">
				<header className="sticky top-0 w-full flex items-center justify-center gap-4 px-4 pb-4 bg-background">
					<Button size="icon" variant="secondary" className="absolute left-4" onClick={() => navigate({ to: "/" })}>
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
				{/* <div className="flex-1 overflow-auto">
					<div className="mx-auto flex max-w-6xl flex-col gap-6 p-6">
						<ArtifactsPanel worksheetId={id} />
					</div>
				</div> */}
			</main>
		</Tabs>
	);
}
