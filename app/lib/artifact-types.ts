import {
	IconAlignLeft,
	IconBook2,
	IconEdit,
	IconListCheck,
	IconNetwork,
} from "@tabler/icons-react";

export const ARTIFACT_TYPES = [
	"MultipleChoiceQuiz",
	"EssayQuiz",
	"CompletionQuiz",
	"Summary",
	"MindMap",
] as const;

export type ArtifactType = (typeof ARTIFACT_TYPES)[number];

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
		description: "4-option question with answer",
		icon: IconListCheck,
	},
	{
		value: "EssayQuiz",
		label: "Essay",
		description: "Open-ended prompt with model answer",
		icon: IconBook2,
	},
	{
		value: "CompletionQuiz",
		label: "Fill in the blank",
		description: "Cloze sentence with hint",
		icon: IconEdit,
	},
	{
		value: "Summary",
		label: "Summary",
		description: "Condensed key ideas",
		icon: IconAlignLeft,
	},
	{
		value: "MindMap",
		label: "Mind map",
		description: "Topic, branches and concepts",
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

export type ArtifactContent =
	| McqContent
	| EssayContent
	| CompletionContent
	| SummaryContent
	| MindMapContent;

export function parseArtifactContent(
	_artifactType: ArtifactType,
	content: string,
): ArtifactContent {
	return JSON.parse(content) as ArtifactContent;
}

export function isArtifactType(value: string): value is ArtifactType {
	return (ARTIFACT_TYPES as readonly string[]).includes(value);
}
