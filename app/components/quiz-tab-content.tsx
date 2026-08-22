import type { Worksheet } from "@/data/worksheets";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { ArtifactType, ArtifactTypeOption, QuizArtifactType } from "@/lib/types";
import { IconAlarm } from "@tabler/icons-react";
import { useNavigate } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Label } from "./ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";
import { Slider } from "./ui/slider";
import { TabsContent } from "./ui/tabs";

interface Props {
	worksheet: Worksheet;
	artifactType: ArtifactTypeOption;
}

const QUIZ_QUESTIONS_COUNT: Record<QuizArtifactType, number> = {
	[ARTIFACT_TYPES.MultipleChoiceQuiz]: 10,
	[ARTIFACT_TYPES.EssayQuiz]: 1,
	[ARTIFACT_TYPES.CompletionQuiz]: 10,
};

const TIME_MULTIPLIERS = [0.25, 0.5, 0.75, 1, 1.5, 2, 2.5, 3];
const QUIZ_TYPE_MULTIPLIER: Partial<Record<ArtifactType, number>> = {
	MultipleChoiceQuiz: 1,
	CompletionQuiz: 1.5,
	EssayQuiz: 10,
};

export function QuizTabContent({ worksheet, artifactType }: Props) {
	const navigate = useNavigate();
	const quizType = artifactType.value as QuizArtifactType;
	const available = worksheet.artifact_counts?.[quizType] ?? 0;
	const max = Math.max(available, 1);
	const defaultCount = Math.min(QUIZ_QUESTIONS_COUNT[quizType], max);
	const [count, setCount] = useState(defaultCount);
	const [timed, setTimed] = useState<number | null>(null);

	const timeOptions = useMemo(() => {
		const times = TIME_MULTIPLIERS.map((t) => Math.ceil(t * count * (QUIZ_TYPE_MULTIPLIER[artifactType.value] ?? 1)));
		return [null, ...times].map((value) => ({
			value,
			label: value ? `${value} ${value === 1 ? "minute" : "minutes"}` : "Untimed",
		}));
	}, [artifactType, count]);

	return (
		<TabsContent key={artifactType.value} value={artifactType.value} className="grow">
			<div className="w-full h-full flex flex-col items-center gap-3 mt-40">
				<artifactType.icon />
				<p className="text-lg font-medium">{artifactType.label}</p>
				<p className="max-w-80 text-center text-muted-foreground">{artifactType.description}</p>
				<Card className="w-full max-w-100">
					<CardContent>
						<div className="w-full flex flex-col gap-4">
							<div className="flex flex-col gap-2">
								<Label>Number of questions</Label>
								<div className="w-full flex items-center gap-3 select-none">
									<p className="text-lg">{count}</p>
									<Slider
										min={quizType === "EssayQuiz" ? 1 : 5}
										max={max}
										step={quizType === "EssayQuiz" ? 1 : 5}
										value={count}
										onValueChange={(value) => {
											setCount(Number(value));
											setTimed(null);
										}}
									/>
								</div>
							</div>
							<div className="flex items-center gap-2">
								<IconAlarm />
								<Select items={timeOptions} value={timed} onValueChange={setTimed}>
									<SelectTrigger size="sm" className="w-full">
										<SelectValue />
									</SelectTrigger>
									<SelectContent>
										{timeOptions.map((item) => (
											<SelectItem key={item.value} value={item.value}>
												{item.label}
											</SelectItem>
										))}
									</SelectContent>
								</Select>
							</div>
						</div>
					</CardContent>
				</Card>
				<Button
					onClick={() =>
						navigate({
							to: "/worksheets/$id/mcq",
							params: { id: worksheet.id },
							search: { count },
						})
					}
				>
					Start quiz
				</Button>
			</div>
		</TabsContent>
	);
}
