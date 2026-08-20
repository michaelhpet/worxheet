import { IconAlignLeft, IconBook2, IconEdit, IconListCheck, IconNetwork } from "@tabler/icons-react";

export const ARTIFACT_TYPES = {
	MultipleChoiceQuiz: "MultipleChoiceQuiz",
	EssayQuiz: "EssayQuiz",
	CompletionQuiz: "CompletionQuiz",
	Summary: "Summary",
	MindMap: "MindMap",
} as const;

export type ArtifactType = keyof typeof ARTIFACT_TYPES;
export type QuizArtifactType = keyof Pick<typeof ARTIFACT_TYPES, "MultipleChoiceQuiz" | "EssayQuiz" | "CompletionQuiz">;

export interface ArtifactTypeOption {
	value: ArtifactType;
	label: string;
	description: string;
	icon: React.ElementType;
}

export const ARTIFACT_TYPE_OPTIONS: ArtifactTypeOption[] = [
	{
		value: "MultipleChoiceQuiz",
		label: "Multiple choice",
		description:
			"A question with four answer options, one correct, plus a detailed explanation of the right answer and why the others don't apply.",
		icon: IconListCheck,
	},
	{
		value: "EssayQuiz",
		label: "Essay",
		description:
			"An open-ended writing prompt that encourages critical thinking and deeper understanding, with a suggested answer you can compare against your own.",
		icon: IconBook2,
	},
	{
		value: "CompletionQuiz",
		label: "Fill in the blank",
		description:
			"A sentence with blanks to fill in, helping you reinforce key terms and concepts through context with a helpful hint along the way.",
		icon: IconEdit,
	},
	{
		value: "Summary",
		label: "Summary",
		description:
			"A clear title, a concise paragraph distilling the main ideas, and a bullet-point list of key takeaways you can skim at a glance.",
		icon: IconAlignLeft,
	},
	{
		value: "MindMap",
		label: "Mind map",
		description:
			"A visual overview of a central topic with branching subtopics and concepts, showing how different ideas relate to each other.",
		icon: IconNetwork,
	},
];

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

export interface MindMapBranch {
	label: string;
	children: string[];
}

export interface MindMapContent {
	topic: string;
	branches: MindMapBranch[];
}

export type ArtifactContent = McqContent | EssayContent | CompletionContent | SummaryContent | MindMapContent;

export function parseArtifactContent(_artifactType: ArtifactType, content: string): ArtifactContent {
	return JSON.parse(content) as ArtifactContent;
}

export function isArtifactType(value: string): value is ArtifactType {
	return value in ARTIFACT_TYPES;
}
