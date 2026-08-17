import { useFileMetadata } from "@/data/file-metadata";
import { FILE_TYPES } from "@/lib/constants";
import { cn, formatBytes } from "@/lib/utils";
import { IconFile, IconTrash } from "@tabler/icons-react";
import { Button } from "./ui/button";
import {
	Item,
	ItemActions,
	ItemContent,
	ItemDescription,
	ItemTitle,
} from "./ui/item";

interface FileCardProps {
	path: string;
	onRemove: (path: string) => void;
	className?: string;
}

export function FileCard({ className, ...props }: FileCardProps) {
	const { data, error, isLoading } = useFileMetadata(props);

	if (isLoading) {
		return null;
	}

	if (!data || error) {
		return null;
	}

	const fileType = FILE_TYPES[data.extension] || {
		icon: IconFile,
		class: "text-gray-300",
	};

	return (
		<Item
			className={cn(
				"group border-b-border rounded-none last:border-b-0",
				className,
			)}
		>
			<ItemContent>
				<ItemTitle className="break-all">{data.name}</ItemTitle>
				<ItemDescription>
					<fileType.icon className={cn("inline size-5", fileType.class)} />
					&nbsp;&bull;&nbsp;{formatBytes(data.size)}
				</ItemDescription>
			</ItemContent>
			<ItemActions className="self-start">
				<Button
					variant="outline"
					size="icon"
					onClick={() => props.onRemove(props.path)}
					className="opacity-0 group-hover:opacity-100"
				>
					<IconTrash />
				</Button>
			</ItemActions>
		</Item>
	);
}
