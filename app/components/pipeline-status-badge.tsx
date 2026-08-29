import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import type { Worksheet } from "@/data/worksheets";
import { cn } from "@/lib/utils";

export function PipelineStatusBadge({
	worksheet,
	className,
}: {
	worksheet: Worksheet;
	className?: string;
}) {
	const status = worksheet.pipeline_status;

	if (status === "running") {
		return (
			<Badge variant="secondary" className={cn("gap-1.5", className)}>
				<Spinner className="size-3" />
				Processing
			</Badge>
		);
	}

	if (status === "failed") {
		return (
			<Badge variant="destructive" className={className}>
				Failed
			</Badge>
		);
	}

	if (status === "done") {
		return (
			<Badge variant="outline" className={className}>
				Done
			</Badge>
		);
	}

	return (
		<Badge variant="ghost" className={className}>
			Ready
		</Badge>
	);
}
