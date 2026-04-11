import { createRootRoute, Outlet, useLocation } from "@tanstack/react-router";
import { ThemeProvider } from "../components/theme-provider";

export const Route = createRootRoute({
  component: () => 
      {
        const location = useLocation()
        console.log(location)
        return <ThemeProvider defaultTheme="system">
        <Outlet />
      </ThemeProvider>}
  
});