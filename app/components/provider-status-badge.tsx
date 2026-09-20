import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import { isProviderReady, useProviderModels, useProviderStatus } from "@/data/provider";
import { openSettings } from "@/lib/settings-bus";
import { cn } from "@/lib/utils";
import { useEffect, useRef } from "react";

const openInference = () => openSettings("inference");

export function ProviderStatusBadge({ className }: { className?: string }) {
	const { data: status, isLoading } = useProviderStatus();
	const ready = isProviderReady(status);
	const { isError: healthFailed, isPending: healthPending } = useProviderModels(ready);
	const openedRef = useRef(false);

	useEffect(() => {
		if (openedRef.current) return;
		if (isLoading || !ready) return;
		if (healthPending) return;
		openedRef.current = true;
		if (healthFailed) openInference();
	}, [isLoading, ready, healthPending, healthFailed]);

	if (isLoading || healthPending) {
		return (
			<Badge variant="secondary" className={cn("gap-1.5", className)}>
				<Spinner className="size-3" />
				Checking
			</Badge>
		);
	}

	if (!ready) {
		return (
			<Badge variant="outline" className={cn("gap-1.5 cursor-pointer", className)} onClick={openInference}>
				<span className="size-1.5 rounded-full bg-amber-500" />
				Not configured
			</Badge>
		);
	}

	if (healthFailed) {
		return (
			<Badge variant="outline" className={cn("gap-1.5 cursor-pointer", className)} onClick={openInference}>
				<span className="size-1.5 rounded-full bg-destructive" />
				Unreachable
			</Badge>
		);
	}

	return (
		<Badge variant="outline" className={cn("gap-1.5 cursor-pointer", className)} onClick={openInference}>
			<span className="size-1.5 rounded-full bg-green-500" />
			Ready
		</Badge>
	);
}
