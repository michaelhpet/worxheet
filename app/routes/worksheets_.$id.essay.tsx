import { quizTextareaClasses, QuizShell, type QuizItem } from "@/components/quiz-shell";
import { useArtifacts } from "@/data/artifacts";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { EssayContent } from "@/lib/types";
import { createFileRoute } from "@tanstack/react-router";
import { useMemo } from "react";
import { z } from "zod";

const searchSchema = z.object({
	count: z.number().default(10),
	time: z.number().optional(),
});

export const Route = createFileRoute("/worksheets_/$id/essay")({
	validateSearch: (search) => searchSchema.parse(search),
	component: EssayQuizPage,
});

function EssayQuizPage() {
	const { id } = Route.useParams();
	const { count, time } = Route.useSearch();
	const { data: artifacts, isLoading } = useArtifacts(id, ARTIFACT_TYPES.EssayQuiz, count);

	const questions: QuizItem[] = useMemo(() => {
		if (!artifacts) return [];
		return artifacts.map((artifact) => {
			const content = JSON.parse(artifact.content) as EssayContent;
			return {
				name: artifact.id,
				title: content.question,
				description: content.instructions,
				reference: content.model_answer,
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
				<textarea name={item.name} placeholder="Write your answer..." className={quizTextareaClasses} rows={4} />
			)}
		/>
	);
}
