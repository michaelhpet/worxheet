import { FilesUploader } from "@/components/files-uploader";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import type { Worksheet } from "@/data/worksheets";
import { useForm } from "@tanstack/react-form";
import { useNavigate } from "@tanstack/react-router";
import { invoke } from "@tauri-apps/api/core";
import { z } from "zod";

interface CreateWorksheetDialogProps {
	open: boolean;
	onOpenChange: (open: boolean) => void;
}

const worksheetSchema = z.object({
	name: z.string().trim(),
	files: z.array(z.string()).min(1, "Add at least one file"),
});

export function CreateWorksheetDialog({ open, onOpenChange }: CreateWorksheetDialogProps) {
	const navigate = useNavigate();

	const form = useForm({
		defaultValues: { name: "", files: [] as string[] },
		validators: { onChange: worksheetSchema },
		onSubmit: async ({ value }) => {
			const name = value.name.trim() || "Untitled Worksheet";
			const worksheet = await invoke<Worksheet>("create_worksheet", {
				name,
				files: value.files,
			});
			onOpenChange(false);
			navigate({ to: "/worksheets/$id", params: { id: worksheet.id } });
		},
	});

	return (
		<Dialog
			open={open}
			onOpenChange={(nextOpen) => {
				if (!nextOpen) {
					form.reset();
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
						<form.Field name="files">
							{(field) => {
								const currentFiles = field.state.value;
								const invalid = field.state.meta.isTouched && field.state.meta.errors.length > 0;
								return (
									<Field className="flex flex-col gap-1.5">
										<FilesUploader
											files={currentFiles}
											onFiles={(newFiles) => {
												if (!newFiles.length) return;
												field.handleChange([
													...currentFiles,
													...newFiles.filter((file) => !currentFiles.includes(file)),
												]);
											}}
											onRemoveFile={(path) => field.handleChange(currentFiles.filter((file) => file !== path))}
											invalid={invalid}
										/>
										{invalid && <FieldError errors={field.state.meta.errors} />}
									</Field>
								);
							}}
						</form.Field>
					</div>

					<DialogFooter>
						<Button
							type="button"
							variant="outline"
							onClick={() => {
								form.reset();
								onOpenChange(false);
							}}
						>
							Cancel
						</Button>
						<Button type="submit">Create worksheet</Button>
					</DialogFooter>
				</form>
			</DialogContent>
		</Dialog>
	);
}
