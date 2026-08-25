import { quizInputClasses, QuizShell, type QuizItem } from "@/components/quiz-shell";
import { useArtifacts } from "@/data/artifacts";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { CompletionContent } from "@/lib/types";
import { createFileRoute } from "@tanstack/react-router";
import { useMemo } from "react";
import { z } from "zod";

const searchSchema = z.object({
	count: z.number().default(10),
	time: z.number().optional(),
});

export const Route = createFileRoute("/worksheets_/$id/completion")({
	validateSearch: (search) => searchSchema.parse(search),
	component: CompletionQuizPage,
});

function CompletionQuizPage() {
	const { id } = Route.useParams();
	const { count, time } = Route.useSearch();
	const { data: artifacts, isLoading } = useArtifacts(id, ARTIFACT_TYPES.CompletionQuiz, count);

	const questions: QuizItem[] = useMemo(() => {
		if (!artifacts) return [];
		return artifacts.map((artifact) => {
			const content = JSON.parse(artifact.content) as CompletionContent;
			return {
				name: artifact.id,
				title: content.sentence,
				description: `Hint: ${content.hint}`,
				expected: content.answer,
			};
		});
	}, [artifacts]);

	return (
		<QuizShell
			id={id}
			loading={isLoading}
			items={questions}
			time={time}
			renderAnswer={(item) => (
				<input
					type="text"
					name={item.name}
					autoComplete="off"
					placeholder="Type your answer..."
					className={quizInputClasses}
				/>
			)}
		/>
	);
}
