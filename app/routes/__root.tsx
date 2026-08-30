import { QueryClientProvider } from "@tanstack/react-query";
import { createRootRoute, Outlet } from "@tanstack/react-router";
import { ProviderSetupDialog } from "@/components/provider-setup-dialog";
import { SettingsDialog } from "@/components/settings-dialog";
import { queryClient } from "@/data/query-client";
import { ThemeProvider } from "../components/theme-provider";

export const Route = createRootRoute({
	component: () => {
		return (
			<ThemeProvider defaultTheme="system">
				<QueryClientProvider client={queryClient}>
					<Outlet />
					<SettingsDialog />
					<ProviderSetupDialog />
				</QueryClientProvider>
			</ThemeProvider>
		);
	},
});
