import { Button } from "@/components/ui/button";
import { useArtifacts } from "@/data/artifacts";
import { useWorksheet } from "@/data/worksheets";
import { IconArrowLeft } from "@tabler/icons-react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { z } from "zod";

const artifactTypeSearchSchema = z.object({
	count: z.number().default(10),
});

export const Route = createFileRoute("/worksheets_/$id/mcq")({
	validateSearch: (search) => artifactTypeSearchSchema.parse(search),
	component: ArtifactTypePage,
});

function ArtifactTypePage() {
	const { id } = Route.useParams();
	const { data: worksheet, isLoading: worksheetLoading } = useWorksheet(id);
	const { isLoading: artifactsLoading } = useArtifacts(id);

	const loading = worksheetLoading || artifactsLoading;

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

	return (
		<main className="w-screen h-screen flex flex-col">
			<header className="sticky top-0 w-full flex items-center gap-4 border-b px-4 py-3 bg-background">
				<Link to="/worksheets/$id" params={{ id }}>
					<Button variant="secondary" size="icon">
						<IconArrowLeft />
					</Button>
				</Link>
				<h1 className="text-lg font-medium">{worksheet.name}</h1>
			</header>
		</main>
	);
}
