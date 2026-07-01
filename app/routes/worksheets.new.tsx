import { FileCard } from "@/components/file-card";
import { FilesUploader } from "@/components/files-uploader";
import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogClose,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
	DialogTrigger,
} from "@/components/ui/dialog";
import { Field, FieldDescription } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import type { Worksheet } from "@/data/worksheets";
import { useForm } from "@tanstack/react-form";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { z } from "zod";

export const Route = createFileRoute("/worksheets/new")({
	component: NewWorksheet,
});

const worksheetSchema = z.object({
	name: z.string().min(1, "Worksheet name is required"),
});

function NewWorksheet() {
	const navigate = useNavigate();
	const [files, setFiles] = useState<string[]>([]);

	const form = useForm({
		defaultValues: { name: "" },
		validators: { onChange: worksheetSchema },
		onSubmit: async ({ value }) => {
			const name = value.name.trim() || "Untitled Worksheet";
			const worksheet = await invoke<Worksheet>("create_worksheet", {
				name,
				files,
			});
			navigate({ to: "/worksheets/$id", params: { id: worksheet.id } });
		},
	});

	const onFiles = (newFiles: string[]) => {
		if (!newFiles.length) return;
		setFiles((previous) => [
			...previous,
			...newFiles.filter((file) => !previous.includes(file)),
		]);
	};

	const removeFile = (path: string) => {
		setFiles((previous) => previous.filter((file) => file !== path));
	};

	return (
		<Dialog
			defaultOpen
			onOpenChange={(open) => {
				if (!open) {
					form.reset();
					navigate({ to: "/worksheets" });
				}
			}}
		>
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
								<DialogTrigger render={<Button>Continue</Button>} />
							</div>
						</footer>
					)}
				</aside>

				<DialogContent showCloseButton={false}>
					<form
						onSubmit={(e) => {
							e.preventDefault();
							e.stopPropagation();
							form.handleSubmit();
						}}
					>
						<DialogHeader>
							<DialogTitle>Name your worksheet</DialogTitle>
						</DialogHeader>
						<div className="py-4">
							<form.Field name="name">
								{(field) => (
									<Field className="flex flex-col gap-1.5">
										<Input
											placeholder="e.g. Biology 101"
											value={field.state.value}
											onChange={(e) => field.handleChange(e.target.value)}
											onBlur={field.handleBlur}
											aria-invalid={
												field.state.meta.isTouched &&
												!field.state.meta.isValid
											}
										/>
										{field.state.meta.isTouched &&
											field.state.meta.errors.length > 0 && (
												<FieldDescription>
													{field.state.meta.errors
														.map((error) => error?.message)
														.join(", ")}
												</FieldDescription>
											)}
									</Field>
								)}
							</form.Field>
						</div>
						<DialogFooter>
							<DialogClose render={<Button variant="outline">Cancel</Button>} />
							<form.Subscribe selector={(state) => state.canSubmit}>
								{(canSubmit) => (
									<Button type="submit" disabled={!canSubmit}>
										Create worksheet
									</Button>
								)}
							</form.Subscribe>
						</DialogFooter>
					</form>
				</DialogContent>
			</main>
		</Dialog>
	);
}
