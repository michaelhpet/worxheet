import type { ElementType } from "react";

export interface Paginated<T> {
	items: T[];
	total: number;
	page: number;
	per_page: number;
	total_pages: number;
}

export type ArtifactType = "MultipleChoiceQuiz" | "EssayQuiz" | "CompletionQuiz" | "Summary" | "MindMap";

export type QuizArtifactType = Extract<ArtifactType, "MultipleChoiceQuiz" | "EssayQuiz" | "CompletionQuiz">;

export interface ArtifactTypeOption {
	value: ArtifactType;
	label: string;
	description: string;
	icon: ElementType;
}

export interface McqContent {
	question: string;
	options: string[];
	answer: number;
	explanation: string;
}

export interface EssayContent {
	question: string;
	instructions: string;
	model_answer: string;
}

export interface CompletionContent {
	sentence: string;
	answer: string;
	hint: string;
}

export interface SummaryContent {
	title: string;
	summary: string;
	key_points: string[];
}

export interface MindMapContent {
	topic: string;
	branches: { label: string; children: string[] }[];
}
