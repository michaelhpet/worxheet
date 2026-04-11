import { createFileRoute } from "@tanstack/react-router";
import { Button } from "../components/ui/button";

export const Route = createFileRoute("/")({
	component: Home,
});

function Home() {
	return (
		<main className="flex flex-col items-center gap-3">
			<p>Home page</p>
			<Button>Click something!</Button>
		</main>
	);
}
