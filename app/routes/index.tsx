import { createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import { FileCard } from "@/components/file-card";
import { FilesUploader } from "@/components/files-uploader";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import { invoke } from "@tauri-apps/api/core";

export const Route = createFileRoute("/")({
	component: Home,
});

function Home() {
	const [files, setFiles] = useState<string[]>([]);

	const onFiles = (newFiles: string[]) => {
		if (!newFiles.length) return;
		setFiles((prev) => [...prev, ...newFiles.filter((f) => !prev.includes(f))]);
	};

	const removeFile = (path: string) => {
		setFiles((prev) => prev.filter((f) => f !== path));
	};

	const createWorksheet = async () => {
		const worksheet = await invoke("create_worksheet", {
			name: "New Worksheet",
		});
		console.log("Created worksheet:", worksheet);
	};

	return (
		<main className="w-screen h-screen flex items-stretch">
			<section className="w-2/3 h-full border-r">
				<FilesUploader onFiles={onFiles} />
			</section>
			<aside className="relative w-1/3 h-full flex flex-col">
				<ul className="flex flex-col p-3 overflow-auto">
					{files.map((file) => (
						<FileCard key={file} path={file} onRemove={removeFile} />
					))}
				</ul>
				{!!files.length && (
					<footer className="sticky bottom-0 w-full mt-auto bg-background">
						<Separator />
						<div className="flex items-center justify-end gap-2 p-3">
							<Button onClick={createWorksheet}>Continue</Button>
						</div>
					</footer>
				)}
			</aside>
		</main>
	);
}
