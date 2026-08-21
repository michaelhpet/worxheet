import type { Worksheet } from "@/data/worksheets";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { ArtifactTypeOption, QuizArtifactType } from "@/lib/types";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Label } from "./ui/label";
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

export function QuizTabContent({ worksheet, artifactType }: Props) {
	const navigate = useNavigate();
	const quizType = artifactType.value as QuizArtifactType;
	const available = worksheet.artifact_counts?.[quizType] ?? 0;
	const max = Math.max(available, 1);
	const defaultCount = Math.min(QUIZ_QUESTIONS_COUNT[quizType], max);
	const [count, setCount] = useState(defaultCount);

	return (
		<TabsContent key={artifactType.value} value={artifactType.value} className="grow">
			<div className="w-full h-full flex flex-col items-center justify-center gap-3">
				<artifactType.icon />
				<p className="text-lg font-medium">{artifactType.label}</p>
				<p className="max-w-80 text-center text-muted-foreground">{artifactType.description}</p>
				<Card className="w-full max-w-100">
					<CardContent>
						<div className="w-full flex flex-col">
							<div className="flex flex-col gap-2">
								<Label>Number of questions</Label>
								<div className="w-full flex items-center gap-3 select-none">
									<p className="text-lg">{count}</p>
									<Slider
										min={quizType === "EssayQuiz" ? 1 : 5}
										max={max}
										step={quizType === "EssayQuiz" ? 1 : 5}
										value={count}
										onValueChange={(value) => setCount(Number(value))}
									/>
								</div>
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
