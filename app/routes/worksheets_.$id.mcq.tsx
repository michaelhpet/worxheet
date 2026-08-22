import { Layout } from "@/components/layout";
import {
	AlertDialog,
	AlertDialogAction,
	AlertDialogCancel,
	AlertDialogContent,
	AlertDialogDescription,
	AlertDialogFooter,
	AlertDialogHeader,
	AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { InputOTP, InputOTPGroup, InputOTPSeparator, InputOTPSlot } from "@/components/ui/input-otp";
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
import { toast } from "@/components/ui/toast";
import { useArtifacts } from "@/data/artifacts";
import { useWorksheet } from "@/data/worksheets";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { McqContent } from "@/lib/types";
import { useCountdown } from "@/lib/use-countdown";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useCallback, useMemo, useRef, useState } from "react";
import { z } from "zod";

const artifactTypeSearchSchema = z.object({
	count: z.number().default(10),
	time: z.number().optional(),
});

export const Route = createFileRoute("/worksheets_/$id/mcq")({
	validateSearch: (search) => artifactTypeSearchSchema.parse(search),
	component: ArtifactTypePage,
});

function formatClock(totalSeconds: number): string {
	const clamped = Math.max(0, totalSeconds);
	const hours = Math.min(Math.floor(clamped / 3600), 99);
	const minutes = Math.floor((clamped % 3600) / 60);
	const seconds = clamped % 60;
	return [hours, minutes, seconds].map((unit) => String(unit).padStart(2, "0")).join("");
}

function ArtifactTypePage() {
	const navigate = useNavigate();
	const { id } = Route.useParams();
	const { count, time } = Route.useSearch();
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

	const exitQuiz = () => {
		navigate({ to: "/worksheets/$id", params: { id } });
	};

	const [confirmOpen, setConfirmOpen] = useState(false);
	const [exitOpen, setExitOpen] = useState(false);

	const submitQuizRef = useRef(submitQuiz);
	submitQuizRef.current = submitQuiz;

	const handleTimeUp = useCallback(() => {
		toast.add({
			title: "Time's up!",
			description: "Your quiz has been submitted automatically.",
			type: "error",
		});
		setConfirmOpen(false);
		const form = document.getElementById("quiz-form") as HTMLFormElement | null;
		if (form) {
			submitQuizRef.current({
				preventDefault: () => {},
				currentTarget: form,
			} as unknown as React.SubmitEvent<HTMLFormElement>);
		}
	}, []);

	const remaining = useCountdown(time !== undefined && time > 0 ? time * 60 : undefined, handleTimeUp);

	const clock = remaining !== null ? formatClock(remaining) : null;
	const urgent = remaining !== null && remaining <= 60;

	if (loading) {
		return (
			<Layout onBack={exitQuiz}>
				<p className="text-muted-foreground">Loading...</p>
			</Layout>
		);
	}

	if (!worksheet) {
		return (
			<Layout onBack={exitQuiz}>
				<p className="text-destructive">Worksheet not found</p>
			</Layout>
		);
	}

	if (!questions?.length) {
		return (
			<Layout onBack={exitQuiz}>
				<p className="text-destructive">No questions not found</p>
			</Layout>
		);
	}

	return (
		<Layout
			onBack={exitQuiz}
			header={
				clock ? (
					<InputOTP readOnly value={clock} maxLength={6}>
						<InputOTPGroup>
							<InputOTPSlot index={0} aria-invalid={!!urgent} />
							<InputOTPSlot index={1} aria-invalid={!!urgent} />
						</InputOTPGroup>
						<InputOTPSeparator icon={<div className="w-5 flex items-center justify-center">:</div>} />
						<InputOTPGroup>
							<InputOTPSlot index={2} aria-invalid={!!urgent} />
							<InputOTPSlot index={3} aria-invalid={!!urgent} />
						</InputOTPGroup>
						<InputOTPSeparator icon={<div className="w-5 flex items-center justify-center">:</div>} />
						<InputOTPGroup>
							<InputOTPSlot index={4} aria-invalid={!!urgent} />
							<InputOTPSlot index={5} aria-invalid={!!urgent} />
						</InputOTPGroup>
					</InputOTP>
				) : undefined
			}
		>
			<div className="flex flex-col items-center mt-40">
				<Questionnaire
					id="quiz-form"
					items={questions}
					shortcuts="numbers"
					className="max-w-xl mx-auto"
					onSubmit={(event) => {
						event.preventDefault();
						setConfirmOpen(true);
					}}
				>
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
			<AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>Submit quiz?</AlertDialogTitle>
						<AlertDialogDescription>You won't be able to change your answers after submitting.</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter>
						<AlertDialogCancel>Don't submit</AlertDialogCancel>
						<AlertDialogAction
							onClick={() => {
								setConfirmOpen(false);
								const form = document.getElementById("quiz-form");
								if (form) {
									submitQuiz({
										preventDefault: () => {},
										currentTarget: form,
									} as unknown as React.SubmitEvent<HTMLFormElement>);
								}
							}}
						>
							Submit
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>
			<AlertDialog open={exitOpen} onOpenChange={setExitOpen}>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>Leave quiz?</AlertDialogTitle>
						<AlertDialogDescription>Your progress in this quiz will be lost.</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter>
						<AlertDialogCancel>Keep answering</AlertDialogCancel>
						<AlertDialogAction
							onClick={() => {
								setExitOpen(false);
								navigate({ to: "/worksheets/$id", params: { id } });
							}}
						>
							Leave
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>
		</Layout>
	);
}
