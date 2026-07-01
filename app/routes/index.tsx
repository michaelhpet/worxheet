import { createFileRoute, redirect } from "@tanstack/react-router";
import { invoke } from "@tauri-apps/api/core";

export const Route = createFileRoute("/")({
	beforeLoad: async () => {
		const result: { total: number } = await invoke("get_worksheets", {
			page: 1,
			per_page: 1,
		});
		if (result.total > 0) {
			throw redirect({ to: "/worksheets" });
		}
		throw redirect({ to: "/worksheets/new" });
	},
});
