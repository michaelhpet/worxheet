import {
	IconAlignLeft,
	IconBook2,
	IconCsv,
	IconEdit,
	IconFileText,
	IconFileWord,
	IconHtml,
	IconListCheck,
	IconMarkdown,
	IconMusic,
	IconNetwork,
	IconPdf,
	IconPhoto,
	IconPresentation,
	IconVideo,
} from "@tabler/icons-react";
import type { ArtifactType, ArtifactTypeOption } from "@/lib/types";

export const SUPPORTED_EXTENSIONS = ["pdf", "ppt", "pptx", "doc", "docx"];

export const FILE_SIZES = ["Bytes", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];

interface FileType {
	icon: React.ElementType;
	class: string;
}

export const FILE_TYPES: Record<string, FileType> = {
	pdf: { icon: IconPdf, class: "text-amber-300" },
	md: { icon: IconMarkdown, class: "text-gray-300" },
	txt: { icon: IconFileText, class: "text-gray-300" },
	csv: { icon: IconCsv, class: "text-green-300" },
	ppt: { icon: IconPresentation, class: "text-red-300" },
	pptx: { icon: IconPresentation, class: "text-red-300" },
	doc: { icon: IconFileWord, class: "text-blue-300" },
	docx: { icon: IconFileWord, class: "text-blue-300" },
	odt: { icon: IconFileWord, class: "text-blue-300" },
	html: { icon: IconHtml, class: "text-purple-300" },
	png: { icon: IconPhoto, class: "text-yellow-200" },
	jpg: { icon: IconPhoto, class: "text-yellow-200" },
	jpeg: { icon: IconPhoto, class: "text-yellow-200" },
	mov: { icon: IconVideo, class: "text-orange-500" },
	mkv: { icon: IconVideo, class: "text-orange-500" },
	avi: { icon: IconVideo, class: "text-orange-500" },
	mp4: { icon: IconVideo, class: "text-orange-500" },
	webm: { icon: IconVideo, class: "text-orange-500" },
	mp3: { icon: IconMusic, class: "text-pink-500" },
	wav: { icon: IconMusic, class: "text-pink-500" },
	aac: { icon: IconMusic, class: "text-pink-500" },
	flac: { icon: IconMusic, class: "text-pink-500" },
	m4a: { icon: IconMusic, class: "text-pink-500" },
};

export const ARTIFACT_TYPES = {
	MultipleChoiceQuiz: "MultipleChoiceQuiz",
	EssayQuiz: "EssayQuiz",
	CompletionQuiz: "CompletionQuiz",
	Summary: "Summary",
	MindMap: "MindMap",
} as const satisfies Record<ArtifactType, ArtifactType>;

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
