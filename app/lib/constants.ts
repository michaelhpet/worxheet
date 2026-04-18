import {
	IconCsv,
	IconFileText,
	IconFileWord,
	IconHtml,
	IconMarkdown,
	IconMusic,
	IconPdf,
	IconPhoto,
	IconPresentation,
	IconVideo,
} from "@tabler/icons-react";

export const FILE_SIZES = [
	"Bytes",
	"KB",
	"MB",
	"GB",
	"TB",
	"PB",
	"EB",
	"ZB",
	"YB",
];

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
