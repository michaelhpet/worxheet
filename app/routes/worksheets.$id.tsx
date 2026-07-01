import { useWorksheet } from "@/data/worksheets";
import { createFileRoute, Link } from "@tanstack/react-router";

export const Route = createFileRoute("/worksheets/$id")({
	component: WorksheetDetail,
});

function WorksheetDetail() {
	const { id } = Route.useParams();
	const { data: worksheet, isLoading, error } = useWorksheet(id);

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
		<main className="w-screen h-screen flex flex-col">
			<header className="flex items-center gap-4 px-6 py-4 border-b">
				<Link
					to="/worksheets"
					className="text-sm text-muted-foreground hover:text-foreground transition-colors"
				>
					&larr; Back
				</Link>
				<h1 className="text-xl font-semibold">{worksheet.name}</h1>
			</header>
			<div className="flex-1 flex items-center justify-center">
				<p className="text-muted-foreground">Workspace coming soon</p>
			</div>
		</main>
	);
}
