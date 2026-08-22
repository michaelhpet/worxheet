import { Button } from "@/components/ui/button";
import {
	Questionnaire,
	QuestionnaireActions,
	QuestionnaireChoice,
	QuestionnaireChoices,
	QuestionnaireError,
	QuestionnaireItem,
	QuestionnaireNext,
	QuestionnairePrevious,
	QuestionnaireProgress,
	QuestionnaireSubmit,
	QuestionnaireTitle,
} from "@/components/ui/questionnaire";
import { useArtifacts } from "@/data/artifacts";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { McqContent } from "@/lib/types";
import { IconArrowLeft } from "@tabler/icons-react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useCallback, useMemo } from "react";
import { z } from "zod";

const artifactTypeSearchSchema = z.object({
	count: z.number().default(10),
});

export const Route = createFileRoute("/worksheets_/$id/mcq")({
	validateSearch: (search) => artifactTypeSearchSchema.parse(search),
	component: ArtifactTypePage,
});

function ArtifactTypePage() {
	const navigate = useNavigate();
	const { id } = Route.useParams();
	const { count } = Route.useSearch();
	const { data: worksheet, isLoading: worksheetLoading } = useWorksheet(id);
	const { data: artifacts, isLoading: artifactsLoading } = useArtifacts(id, ARTIFACT_TYPES.MultipleChoiceQuiz, count);

	const loading = worksheetLoading || artifactsLoading;

	const questions = useMemo(() => {
		if (!artifacts) return [];
		return artifacts.map((artifact) => {
			const question = JSON.parse(artifact.content) as McqContent;
			return {
				required: true,
				name: artifact.id,
				title: question.question,
				choices: question.options.map((option, index) => ({ name: artifact.id + index, label: option, value: option })),
			};
		});
	}, [artifacts]);

	const submitQuiz = useCallback(
		(event: React.SubmitEvent<HTMLFormElement>) => {
			event.preventDefault();
			const formData = new FormData(event.currentTarget);
			const answers = questions.map((q) => ({
				name: q.name,
				title: q.title,
				answer: formData.get(q.name),
			}));
			console.log("answers are", answers);
		},

		[questions],
	);

	if (loading) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-muted-foreground">Loading...</p>
			</main>
		);
	}

	if (!worksheet) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-destructive">Worksheet not found</p>
			</main>
		);
	}

	if (!questions?.length) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-destructive">No questions not found</p>
			</main>
		);
	}

	const exitQuiz = () => {
		navigate({ to: "/worksheets/$id", params: { id } });
	};

	return (
		<main className="w-screen h-screen flex flex-col">
			<header className="sticky top-0 w-full flex items-center justify-center gap-2 px-4 pb-4 bg-background">
				<Button size="icon" variant="secondary" onClick={exitQuiz}>
					<IconArrowLeft />
				</Button>
			</header>
			<div className="grow flex flex-col items-center mt-20">
				<Questionnaire items={questions} shortcuts="numbers" className="max-w-xl mx-auto" onSubmit={submitQuiz}>
					<QuestionnaireProgress />
					{questions.map((question) => (
						<QuestionnaireItem key={question.name} name={question.name} required={question.required}>
							<QuestionnaireTitle>{question.title}</QuestionnaireTitle>
							<QuestionnaireChoices>
								{question.choices.map((choice) => (
									<QuestionnaireChoice key={choice.name} value={choice.value}>
										<span className="font-medium">{choice.label}</span>
									</QuestionnaireChoice>
								))}
							</QuestionnaireChoices>
							<QuestionnaireError />
						</QuestionnaireItem>
					))}
					<QuestionnaireActions>
						<QuestionnairePrevious />
						<QuestionnaireNext>Next</QuestionnaireNext>
						<QuestionnaireSubmit>Submit</QuestionnaireSubmit>
					</QuestionnaireActions>
				</Questionnaire>
			</div>
		</main>
	);
}
