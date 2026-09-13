import type { ReactNode } from "react";
import { useCallback, useRef, useState } from "react";
import { useNavigate } from "@tanstack/react-router";
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
import { Button } from "@/components/ui/button";
import { InputOTP, InputOTPGroup, InputOTPSeparator, InputOTPSlot } from "@/components/ui/input-otp";
import { RadioGroupItem } from "@/components/ui/radio-group";
import { toast } from "@/components/ui/toast";
import { QuizGrade, type QuizQuestionResult } from "@/components/quiz-grade";
import { Layout } from "@/components/layout";
import { useCountdown } from "@/lib/use-countdown";

export interface QuizItem {
	name: string;
	title: string;
	description?: string;
	choices?: { value: string; label?: string }[];
	expected?: string;
	reference?: string;
}

interface QuizShellProps {
	id: string;
	loading: boolean;
	items: QuizItem[];
	renderAnswer: (item: QuizItem) => ReactNode;
	time?: number;
}

function formatClock(totalSeconds: number): string {
	const clamped = Math.max(0, totalSeconds);
	const hours = Math.min(Math.floor(clamped / 3600), 99);
	const minutes = Math.floor((clamped % 3600) / 60);
	const seconds = clamped % 60;
	return [hours, minutes, seconds].map((unit) => String(unit).padStart(2, "0")).join("");
}

function normalizeAnswer(value: string): string {
	return value.trim().toLowerCase();
}

const itemTitleClasses = "font-heading text-base font-semibold text-pretty";
const itemDescriptionClasses = "text-sm text-pretty text-muted-foreground";
const progressClasses = "min-h-[1lh] w-fit min-w-[14ch] text-xs font-medium text-muted-foreground tabular-nums";
export const quizInputClasses =
	"h-9 min-h-11 w-full min-w-0 rounded-md border border-input bg-transparent px-2.5 py-1 text-base shadow-xs transition-[color,box-shadow,background-color] outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 sm:min-h-0 md:text-sm dark:bg-input/30 selection:bg-primary selection:text-primary-foreground placeholder:text-muted-foreground";
export const quizTextareaClasses = `${quizInputClasses} min-h-28 resize-none py-2`;

export function QuizChoice({ option }: { option: { value: string; label?: string } }) {
	return (
		// biome-ignore lint/a11y/noLabelWithoutControl: RadioGroupItem renders an associated hidden radio input inside the label at runtime
		<label className="group/choice relative flex min-h-11 cursor-pointer items-start gap-3 rounded-md border border-input bg-transparent px-4 py-3.5 text-start text-sm shadow-xs transition-colors outline-none select-none hover:bg-muted/50 has-[button:focus-visible]:border-ring has-[button:focus-visible]:ring-3 has-[button:focus-visible]:ring-ring/50 has-[>input:checked]:border-primary/40 has-[>input:checked]:bg-muted dark:bg-input/20">
			<RadioGroupItem value={option.value} className="mt-0.5 focus-visible:ring-0" />
			<span className="flex min-w-0 flex-1 flex-col gap-1 leading-snug">
				<span className="font-medium">{option.label ?? option.value}</span>
			</span>
		</label>
	);
}

export function QuizShell({ id, loading, items, renderAnswer, time }: QuizShellProps) {
	const navigate = useNavigate();
	const [step, setStep] = useState(0);
	const [confirmOpen, setConfirmOpen] = useState(false);
	const [exitOpen, setExitOpen] = useState(false);
	const [result, setResult] = useState<QuizQuestionResult[] | null>(null);

	const exitQuiz = () => {
		navigate({ to: "/worksheets/$id", params: { id } });
	};

	const gradeQuiz = () => {
		const form = document.getElementById("quiz-form") as HTMLFormElement | null;
		if (!form) return;
		const formData = new FormData(form);
		console.log(items.map((item) => ({ title: item.title, answer: formData.get(item.name) })));
		setResult(
			items.map((item) => {
				const raw = formData.get(item.name);
				const given = typeof raw === "string" && raw.trim().length > 0 ? raw : null;
				if (item.expected === undefined) {
					return { name: item.name, title: item.title, given, reference: item.reference, correct: null };
				}
				const correct =
					item.choices && item.choices.length > 0
						? given !== null && given === item.expected
						: given !== null && normalizeAnswer(given) === normalizeAnswer(item.expected);
				return { name: item.name, title: item.title, given, expected: item.expected, correct };
			}),
		);
	};

	const gradeQuizRef = useRef(gradeQuiz);
	gradeQuizRef.current = gradeQuiz;

	const handleTimeUp = useCallback(() => {
		toast.add({
			title: "Time's up!",
			description: "Your quiz has been submitted automatically.",
			type: "error",
		});
		setConfirmOpen(false);
		setExitOpen(false);
		gradeQuizRef.current();
	}, []);

	const remaining = useCountdown(time !== undefined && time > 0 ? time * 60 : undefined, handleTimeUp);

	const lastItemsRef = useRef(items);
	if (lastItemsRef.current !== items) {
		lastItemsRef.current = items;
		setStep(0);
	}

	const clock = remaining !== null ? formatClock(remaining) : null;
	const urgent = remaining !== null && remaining <= 60;

	if (loading) {
		return (
			<Layout onBack={exitQuiz}>
				<p className="text-muted-foreground">Loading...</p>
			</Layout>
		);
	}

	if (!items.length) {
		return (
			<Layout onBack={exitQuiz}>
				<p className="text-destructive">No questions available for this quiz.</p>
			</Layout>
		);
	}

	if (result) {
		return <QuizGrade result={result} onBack={exitQuiz} />;
	}

	const isFirst = step === 0;
	const isLast = step === items.length - 1;

	return (
		<Layout
			onBack={exitQuiz}
			header={
				clock ? (
					<InputOTP readOnly value={clock} maxLength={6}>
						<InputOTPGroup>
							<InputOTPSlot index={0} aria-invalid={urgent} />
							<InputOTPSlot index={1} aria-invalid={urgent} />
						</InputOTPGroup>
						<InputOTPSeparator icon={<div className="w-5 flex items-center justify-center">:</div>} />
						<InputOTPGroup>
							<InputOTPSlot index={2} aria-invalid={urgent} />
							<InputOTPSlot index={3} aria-invalid={urgent} />
						</InputOTPGroup>
						<InputOTPSeparator icon={<div className="w-5 flex items-center justify-center">:</div>} />
						<InputOTPGroup>
							<InputOTPSlot index={4} aria-invalid={urgent} />
							<InputOTPSlot index={5} aria-invalid={urgent} />
						</InputOTPGroup>
					</InputOTP>
				) : undefined
			}
		>
			<form id="quiz-form" onSubmit={(event) => event.preventDefault()}>
				<div className="mx-auto mt-[calc((100vh-436px)/4)] flex w-full max-w-xl flex-col items-center">
					<p className={progressClasses}>{`Question ${step + 1} of ${items.length}`}</p>
					<div className="mt-6 flex w-full min-w-0 flex-col gap-5">
						{items.map((item, index) => (
							<div key={item.name} hidden={index !== step} className="flex flex-col gap-2">
								<h2 className={itemTitleClasses}>{item.title}</h2>
								{item.description ? <p className={itemDescriptionClasses}>{item.description}</p> : null}
								{renderAnswer(item)}
							</div>
						))}
					</div>
					<div className="mt-3 grid min-h-11 w-full grid-cols-[minmax(0,1fr)_auto_auto] items-center gap-2 sm:min-h-9">
						<Button
							variant="outline"
							className="col-start-1 row-start-1 justify-self-start"
							disabled={isFirst}
							onClick={() => setStep((current) => Math.max(0, current - 1))}
						>
							Previous
						</Button>
						{!isLast ? (
							<Button className="col-start-3 row-start-1 justify-self-end" onClick={() => setStep(step + 1)}>
								Next
							</Button>
						) : (
							<Button className="col-start-3 row-start-1 justify-self-end" onClick={() => setConfirmOpen(true)}>
								Submit
							</Button>
						)}
					</div>
				</div>
			</form>
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
								gradeQuiz();
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
							className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
							onClick={() => {
								setExitOpen(false);
								exitQuiz();
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
