import { Button } from "@/components/ui/button";
import { useArtifacts } from "@/data/artifacts";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPES } from "@/lib/constants";
import { IconArrowLeft } from "@tabler/icons-react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMemo } from "react";
import { z } from "zod";

const artifactTypeSearchSchema = z.object({
	count: z.number().default(10),
});

export const Route = createFileRoute("/worksheets_/$id/mcq")({
	validateSearch: (search) => artifactTypeSearchSchema.parse(search),
	component: ArtifactTypePage,
});

function ArtifactTypePage() {
	const navigate = useNavigate();
	const { id } = Route.useParams();
	const { count } = Route.useSearch();
	const { data: worksheet, isLoading: worksheetLoading } = useWorksheet(id);
	const { data: artifacts, isLoading: artifactsLoading } = useArtifacts(id, ARTIFACT_TYPES.MultipleChoiceQuiz, count);

	const loading = worksheetLoading || artifactsLoading;

	const questions = useMemo(() => {
		return artifacts?.map((artifact) => JSON.parse(artifact.content));
	}, [artifacts]);

	if (loading) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-muted-foreground">Loading...</p>
			</main>
		);
	}

	if (!worksheet) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-destructive">Worksheet not found</p>
			</main>
		);
	}

	if (!questions?.length) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-destructive">No questions not found</p>
			</main>
		);
	}

	console.log("questions are", questions);

	const exitQuiz = () => {
		navigate({ to: "/worksheets/$id", params: { id } });
	};

	return (
		<main className="w-screen h-screen flex flex-col">
			<header className="sticky top-0 w-full flex items-center justify-center gap-2 px-4 pb-4 bg-background">
				<Button size="icon" variant="secondary" onClick={exitQuiz}>
					<IconArrowLeft />
				</Button>
			</header>
		</main>
	);
}
