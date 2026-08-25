import { IconCheck, IconX } from "@tabler/icons-react";
import { Button } from "@/components/ui/button";
import { Layout } from "@/components/layout";

export interface QuizQuestionResult {
	name: string;
	title: string;
	given: string | null;
	expected?: string;
	reference?: string;
	correct: boolean | null;
}

interface QuizGradeProps {
	result: QuizQuestionResult[];
	onBack: () => void;
}

function tierFor(percent: number): string {
	if (percent >= 80) return "Excellent";
	if (percent >= 50) return "Good job";
	return "Keep practicing";
}

export function QuizGrade({ result, onBack }: QuizGradeProps) {
	const graded = result.filter((entry) => entry.correct !== null);
	const correctCount = graded.filter((entry) => entry.correct).length;
	const percent = graded.length > 0 ? Math.round((correctCount / graded.length) * 100) : null;

	return (
		<Layout onBack={onBack}>
			<div className="mx-auto flex w-full max-w-xl flex-col gap-2 pb-10">
				<p className="font-medium text-muted-foreground">
					{percent === null ? "Submitted for review" : tierFor(percent)}
				</p>
				<h1 className="text-3xl font-semibold tabular-nums">{percent === null ? "—" : `${percent}/100`}</h1>
				<p className="tabular-nums text-muted-foreground">
					{correctCount}/{graded.length} correct
					{graded.length < result.length ? ` · ${result.length - graded.length} for review` : ""}
				</p>
				<ul className="mt-6 flex flex-col gap-3">
					{result.map((entry) => (
						<li key={entry.name} className="flex flex-col gap-2 rounded-md border p-4">
							<div className="flex items-center justify-between gap-3">
								<span className="text-sm font-medium text-balance">{entry.title}</span>
								{entry.correct === true ? (
									<IconCheck aria-label="Correct" className="size-5 shrink-0 text-green-500" />
								) : entry.correct === false ? (
									<IconX aria-label="Incorrect" className="size-5 shrink-0 text-destructive" />
								) : (
									<span className="shrink-0 rounded-full bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
										review
									</span>
								)}
							</div>
							<div className="flex flex-col gap-1 text-sm text-muted-foreground">
								<p>
									Your answer:{" "}
									<span className={entry.given ? "text-foreground" : undefined}>{entry.given ?? "Unanswered"}</span>
								</p>
								{entry.correct === false && entry.expected ? (
									<p>
										Correct answer: <span className="text-foreground">{entry.expected}</span>
									</p>
								) : null}
								{entry.correct === null && entry.reference ? (
									<p>
										Model answer: <span className="text-foreground">{entry.reference}</span>
									</p>
								) : null}
							</div>
						</li>
					))}
				</ul>
				<Button className="mt-8 self-start" onClick={onBack}>
					Back to worksheet
				</Button>
			</div>
		</Layout>
	);
}
