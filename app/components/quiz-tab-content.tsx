import { usePipelineStatus } from "@/data/pipeline";
import type { Worksheet } from "@/data/worksheets";
import { ARTIFACT_TYPES } from "@/lib/constants";
import type { ArtifactTypeOption, QuizArtifactType } from "@/lib/types";
import { useForm } from "@tanstack/react-form";
import { useNavigate } from "@tanstack/react-router";
import { useMemo } from "react";
import { z } from "zod";
import { Badge } from "./ui/badge";
import { Button } from "./ui/button";
import { Card, CardContent } from "./ui/card";
import { Field, FieldContent, FieldError, FieldLabel } from "./ui/field";
import { Input } from "./ui/input";
import { Spinner } from "./ui/spinner";
import { TabsContent } from "./ui/tabs";

interface Props {
	worksheet: Worksheet;
	artifactType: ArtifactTypeOption;
}

const QUIZ_ROUTES: Record<
	QuizArtifactType,
	"/worksheets/$id/mcq" | "/worksheets/$id/essay" | "/worksheets/$id/completion"
> = {
	[ARTIFACT_TYPES.MultipleChoiceQuiz]: "/worksheets/$id/mcq",
	[ARTIFACT_TYPES.EssayQuiz]: "/worksheets/$id/essay",
	[ARTIFACT_TYPES.CompletionQuiz]: "/worksheets/$id/completion",
};

const quizSchema = z.object({
	count: z
		.string()
		.refine(
			(v) => Number.isInteger(Number(v.trim())) && Number(v.trim()) >= 1,
			"Number of questions must be at least 1.",
		),
	time: z
		.string()
		.refine(
			(v) => v.trim() === "" || (Number.isInteger(Number(v)) && Number(v) >= 1),
			"Time limit must be at least 1 minute.",
		),
});

export function QuizTabContent({ worksheet, artifactType }: Props) {
	const navigate = useNavigate();
	const quizType = artifactType.value as QuizArtifactType;
	const available = worksheet.artifact_counts?.[quizType] ?? 0;

	const { data: pipelineStatus } = usePipelineStatus(worksheet.id);

	const generating = useMemo(() => {
		if (pipelineStatus?.status !== "running") return false;
		if (pipelineStatus.phase === "ingesting") return true;
		if (pipelineStatus.phase !== "generating") return false;
		const progress = pipelineStatus.types?.find((t) => t.artifact_type === artifactType.value);
		return (progress?.done ?? 0) < (progress?.total ?? 0);
	}, [pipelineStatus, artifactType]);

	const form = useForm({
		defaultValues: { count: "", time: "" },
		validators: { onChange: quizSchema },
		onSubmit: async ({ value }) => {
			if (available === 0) return;
			const count = Number(value.count.trim());
			const time = value.time.trim() === "" ? undefined : Number(value.time);
			navigate({
				to: QUIZ_ROUTES[quizType],
				params: { id: worksheet.id },
				search: { count, time },
			});
		},
	});

	return (
		<TabsContent key={artifactType.value} value={artifactType.value} className="grow-0">
			<div className="w-full h-full flex flex-col items-center gap-3 mt-[calc((100vh-436px)/4)]">
				<artifactType.icon />
				<p className="text-lg font-medium">
					{artifactType.label}&nbsp;
					<Badge variant="secondary">
						{generating && <Spinner />}
						{available}
					</Badge>
				</p>
				<p className="max-w-80 text-center text-muted-foreground">{artifactType.description}</p>
				<form
					className="w-full max-w-100 flex flex-col items-center gap-3"
					onSubmit={(e) => {
						e.preventDefault();
						e.stopPropagation();
						form.handleSubmit();
					}}
				>
					<Card className="w-full">
						<CardContent>
							<div className="w-full flex flex-col gap-4">
								<form.Field name="count">
									{(field) => (
										<Field data-invalid={field.state.meta.isTouched && !field.state.meta.isValid}>
											<FieldLabel htmlFor={`${quizType}-count`}>Number of questions</FieldLabel>
											<FieldContent>
												<Input
													type="number"
													inputMode="numeric"
													id={`${quizType}-count`}
													min={1}
													step={1}
													placeholder="10"
													value={field.state.value}
													onChange={(e) => field.handleChange(e.target.value)}
													onBlur={field.handleBlur}
													aria-invalid={field.state.meta.isTouched && !field.state.meta.isValid}
												/>
												{field.state.meta.isTouched && field.state.meta.errors.length > 0 && (
													<FieldError>{field.state.meta.errors.map((error) => error?.message).join(", ")}</FieldError>
												)}
											</FieldContent>
										</Field>
									)}
								</form.Field>
								<form.Field name="time">
									{(field) => (
										<Field data-invalid={field.state.meta.isTouched && !field.state.meta.isValid}>
											<FieldLabel htmlFor={`${quizType}-time`}>Time limit (minutes)</FieldLabel>
											<FieldContent>
												<Input
													id={`${quizType}-time`}
													type="number"
													inputMode="numeric"
													min={1}
													step={1}
													placeholder="Untimed"
													value={field.state.value}
													onChange={(e) => field.handleChange(e.target.value)}
													onBlur={field.handleBlur}
													aria-invalid={field.state.meta.isTouched && !field.state.meta.isValid}
												/>
												{field.state.meta.isTouched && field.state.meta.errors.length > 0 && (
													<FieldError>{field.state.meta.errors.map((error) => error?.message).join(", ")}</FieldError>
												)}
											</FieldContent>
										</Field>
									)}
								</form.Field>
							</div>
						</CardContent>
					</Card>
					<form.Subscribe selector={(state) => state.canSubmit}>
						{(canSubmit) => (
							<Button type="submit" disabled={!canSubmit || available === 0}>
								Start quiz
							</Button>
						)}
					</form.Subscribe>
				</form>
			</div>
		</TabsContent>
	);
}
