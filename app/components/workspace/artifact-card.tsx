import type { Artifact } from "@/data/artifacts";
import { ARTIFACT_TYPE_OPTIONS } from "@/lib/artifact-types";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ArtifactContentView } from "./artifact-content";

export function ArtifactCard({ artifact }: { artifact: Artifact }) {
	const option = ARTIFACT_TYPE_OPTIONS.find(
		(o) => o.value === artifact.artifact_type,
	);

	return (
		<Card>
			<CardHeader>
				<CardTitle className="flex items-center gap-2">
					{option?.icon && (
						<option.icon className="size-4 text-muted-foreground" />
					)}
					{option?.label ?? artifact.artifact_type}
				</CardTitle>
			</CardHeader>
			<CardContent>
				<ArtifactContentView
					artifactType={artifact.artifact_type}
					content={artifact.content}
				/>
			</CardContent>
		</Card>
	);
}
