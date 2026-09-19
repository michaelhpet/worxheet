import {
	IconAlignLeft,
	IconBook2,
	IconCsv,
	IconEdit,
	IconFileText,
	IconFileWord,
	IconListCheck,
	IconMarkdown,
	IconNetwork,
	IconPdf,
	IconPhoto,
	IconPresentation,
} from "@tabler/icons-react";
import type { ArtifactType, ArtifactTypeOption } from "@/lib/types";

// Must stay in sync with `SUPPORTED_EXTENSIONS` in core/src/worksheet.rs and
// `parse_blocks` in core/src/pipeline/ingest.rs.
export const SUPPORTED_EXTENSIONS = [
	"pdf",
	"png",
	"jpg",
	"jpeg",
	"gif",
	"bmp",
	"tif",
	"tiff",
	"webp",
	"svg",
	"ppt",
	"pptx",
	"doc",
	"docx",
	"txt",
	"md",
	"csv",
];

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
	png: { icon: IconPhoto, class: "text-yellow-200" },
	jpg: { icon: IconPhoto, class: "text-yellow-200" },
	jpeg: { icon: IconPhoto, class: "text-yellow-200" },
	gif: { icon: IconPhoto, class: "text-yellow-200" },
	bmp: { icon: IconPhoto, class: "text-yellow-200" },
	tif: { icon: IconPhoto, class: "text-yellow-200" },
	tiff: { icon: IconPhoto, class: "text-yellow-200" },
	webp: { icon: IconPhoto, class: "text-yellow-200" },
	svg: { icon: IconPhoto, class: "text-yellow-200" },
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
