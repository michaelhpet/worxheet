import { QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, Outlet } from "@tanstack/react-router";
import { queryClient } from "@/data/query-client";
import { ThemeProvider } from "../components/theme-provider";

export const Route = createRootRoute({
	component: () => {
		return (
			<ThemeProvider defaultTheme="system">
				<QueryClientProvider client={queryClient}>
					<Outlet />
				</QueryClientProvider>
			</ThemeProvider>
		);
	},
});
