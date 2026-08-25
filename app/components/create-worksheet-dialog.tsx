import { FileCard } from "@/components/file-card";
import { FilesUploader } from "@/components/files-uploader";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { isProviderReady, useProviderStatus } from "@/data/provider";
import type { Worksheet } from "@/data/worksheets";
import { openSettings } from "@/lib/settings-bus";
import { useForm } from "@tanstack/react-form";
import { useNavigate } from "@tanstack/react-router";
import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { z } from "zod";

interface CreateWorksheetDialogProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
}

const worksheetSchema = z.object({
	name: z.string().trim(),
});

export function CreateWorksheetDialog({ open, onOpenChange }: CreateWorksheetDialogProps) {
	const navigate = useNavigate();
	const [files, setFiles] = useState<string[]>([]);
	const [providerBlocked, setProviderBlocked] = useState(false);
	const providerStatus = useProviderStatus();

	const form = useForm({
		defaultValues: { name: "" },
		validators: { onChange: worksheetSchema },
		onSubmit: async ({ value }) => {
			if (files.length > 0 && !isProviderReady(providerStatus.data)) {
				setProviderBlocked(true);
				return;
			}
			const name = value.name.trim() || "Untitled Worksheet";
			const worksheet = await invoke<Worksheet>("create_worksheet", {
				name,
				files,
			});
			onOpenChange(false);
			navigate({ to: "/worksheets/$id", params: { id: worksheet.id } });
		},
	});

	const onFiles = (newFiles: string[]) => {
		if (!newFiles.length) return;
		setFiles((previous) => [...previous, ...newFiles.filter((file) => !previous.includes(file))]);
	};

	const removeFile = (path: string) => {
		setFiles((previous) => previous.filter((file) => file !== path));
	};

	return (
		<Dialog
			open={open}
			onOpenChange={(nextOpen) => {
				if (!nextOpen) {
					form.reset();
					setFiles([]);
				}
				onOpenChange(nextOpen);
			}}
		>
			<DialogContent className="sm:max-w-lg">
				<form
					onSubmit={(e) => {
						e.preventDefault();
						e.stopPropagation();
						form.handleSubmit();
					}}
				>
					<DialogHeader>
						<DialogTitle>Create worksheet</DialogTitle>
					</DialogHeader>

					<div className="space-y-4 py-4">
						<form.Field name="name">
							{(field) => (
								<Field className="flex flex-col gap-1.5">
									<Input
										placeholder="e.g. Biology 101"
										value={field.state.value}
										onChange={(e) => field.handleChange(e.target.value)}
										onBlur={field.handleBlur}
										aria-invalid={field.state.meta.isTouched && !field.state.meta.isValid}
									/>
									{field.state.meta.isTouched && field.state.meta.errors.length > 0 && (
										<FieldDescription>
											{field.state.meta.errors.map((error) => error?.message).join(", ")}
										</FieldDescription>
									)}
								</Field>
							)}
						</form.Field>

						<FilesUploader onFiles={onFiles} />
					</div>

					{files.length > 0 && (
						<ul className="flex flex-col border rounded-lg mb-4 max-h-40 overflow-auto">
							{files.map((file) => (
								<FileCard key={file} path={file} onRemove={removeFile} className="border-0 border-b last:border-b-0" />
							))}
						</ul>
					)}

					{providerBlocked && files.length > 0 && (
						<div className="mx-6 mb-4 rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
							Generating artifacts needs an LLM provider.{" "}
							<button
								type="button"
								className="underline font-medium"
								onClick={() => {
									onOpenChange(false);
									openSettings("provider");
								}}
							>
								Set one up in Settings
							</button>{" "}
							to continue.
						</div>
					)}

					<DialogFooter>
						<Button
							type="button"
							variant="outline"
							onClick={() => {
								form.reset();
								setFiles([]);
								setProviderBlocked(false);
								onOpenChange(false);
							}}
						>
							Cancel
						</Button>
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
		</Dialog>
	);
}
