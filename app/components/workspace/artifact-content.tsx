import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
	type ArtifactContent,
	type ArtifactType,
	type CompletionContent,
	type EssayContent,
	type McqContent,
	type MindMapContent,
	type SummaryContent,
	parseArtifactContent,
} from "@/lib/artifact-types";
import { cn } from "@/lib/utils";

const OPTION_LETTERS = ["A", "B", "C", "D", "E", "F", "G", "H"];

function McqView({ content }: { content: McqContent }) {
	const [revealed, setRevealed] = useState(false);
	const answerIndex = content.answer;

	return (
		<div className="flex flex-col gap-3">
			<p className="font-medium">{content.question}</p>
			<ul className="flex flex-col gap-1.5">
				{content.options.map((option, index) => {
					const correct = revealed && index === answerIndex;
					return (
						<li
							key={option}
							className={cn(
								"flex items-center gap-2 rounded-md border px-3 py-1.5",
								correct && "border-primary/50 bg-primary/10 text-primary",
							)}
						>
							<span className="text-xs font-medium text-muted-foreground">{OPTION_LETTERS[index]}</span>
							<span className="min-w-0 wrap-break-word">{option}</span>
							{correct && <span className="ml-auto shrink-0 text-xs font-medium">Correct</span>}
						</li>
					);
				})}
			</ul>
			{revealed ? (
				<p className="text-sm text-muted-foreground">
					<span className="font-medium text-foreground">
						{OPTION_LETTERS[answerIndex]}: {content.options[answerIndex]}
					</span>
					{content.explanation ? ` — ${content.explanation}` : ""}
				</p>
			) : (
				<Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setRevealed(true)}>
					Show answer
				</Button>
			)}
		</div>
	);
}

function EssayView({ content }: { content: EssayContent }) {
	const [revealed, setRevealed] = useState(false);

	return (
		<div className="flex flex-col gap-3">
			<p className="font-medium">{content.question}</p>
			{content.instructions && <p className="text-sm text-muted-foreground">{content.instructions}</p>}
			{revealed ? (
				<div className="flex flex-col gap-1">
					<p className="text-xs font-medium text-muted-foreground">Model answer</p>
					<p className="whitespace-pre-wrap text-sm">{content.model_answer}</p>
				</div>
			) : (
				<Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setRevealed(true)}>
					Show model answer
				</Button>
			)}
		</div>
	);
}

function CompletionView({ content }: { content: CompletionContent }) {
	const [revealed, setRevealed] = useState(false);

	return (
		<div className="flex flex-col gap-3">
			<p className="font-medium">{content.sentence}</p>
			{revealed ? (
				<div className="flex flex-col gap-1">
					<p className="text-sm">
						<span className="font-medium text-primary">{content.answer}</span>
						{content.hint ? ` — hint: ${content.hint}` : ""}
					</p>
				</div>
			) : (
				<Button type="button" variant="outline" size="sm" className="self-start" onClick={() => setRevealed(true)}>
					Show answer
				</Button>
			)}
		</div>
	);
}

function SummaryView({ content }: { content: SummaryContent }) {
	return (
		<div className="flex flex-col gap-3">
			<p className="font-medium">{content.title}</p>
			<p className="text-sm">{content.summary}</p>
			{content.key_points.length > 0 && (
				<ul className="flex flex-col gap-1 text-sm text-muted-foreground">
					{content.key_points.map((point) => (
						<li key={point} className="flex gap-2">
							<span className="shrink-0 text-foreground/50">•</span>
							<span>{point}</span>
						</li>
					))}
				</ul>
			)}
		</div>
	);
}

function MindMapView({ content }: { content: MindMapContent }) {
	return (
		<div className="flex flex-col gap-2">
			<p className="font-medium">{content.topic}</p>
			<ul className="flex flex-col gap-1.5">
				{content.branches.map((branch) => (
					<li key={branch.label} className="flex flex-col gap-1 border-l pl-3">
						<span className="text-sm font-medium">{branch.label}</span>
						{branch.children.length > 0 && (
							<ul className="flex flex-col gap-1 text-sm text-muted-foreground">
								{branch.children.map((child) => (
									<li key={child}>{child}</li>
								))}
							</ul>
						)}
					</li>
				))}
			</ul>
		</div>
	);
}

export function ArtifactContentView({ artifactType, content }: { artifactType: ArtifactType; content: string }) {
	let parsed: ArtifactContent;
	try {
		parsed = parseArtifactContent(artifactType, content);
	} catch {
		return <pre className="overflow-x-auto whitespace-pre-wrap text-xs text-muted-foreground">{content}</pre>;
	}

	switch (artifactType) {
		case "MultipleChoiceQuiz":
			return <McqView content={parsed as McqContent} />;
		case "EssayQuiz":
			return <EssayView content={parsed as EssayContent} />;
		case "CompletionQuiz":
			return <CompletionView content={parsed as CompletionContent} />;
		case "Summary":
			return <SummaryView content={parsed as SummaryContent} />;
		case "MindMap":
			return <MindMapView content={parsed as MindMapContent} />;
	}
}
