import {
	IconChevronDown,
	IconLoader,
	IconMinus,
	IconPlus,
	IconSettings,
} from "@tabler/icons-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { useGenerateArtifacts } from "@/data/artifacts";
import {
	formatDownloadProgress,
	modelDownloadLabel,
	useModelDownload,
} from "@/data/model-downloads";
import { usePipelineProgress } from "@/data/progress";
import { ARTIFACT_TYPE_OPTIONS, type ArtifactType } from "@/lib/artifact-types";
import { toErrorMessage } from "@/lib/errors";
import { cn } from "@/lib/utils";

interface GenerateDialogProps {
	worksheetId: string;
	open: boolean;
	onOpenChange: (open: boolean) => void;
}

export function GenerateDialog({
	worksheetId,
	open,
	onOpenChange,
}: GenerateDialogProps) {
	const [artifactType, setArtifactType] =
		useState<ArtifactType>("MultipleChoiceQuiz");
	const [count, setCount] = useState(1);
	const [advanced, setAdvanced] = useState(false);
	const [temperature, setTemperature] = useState("0.7");
	const [topP, setTopP] = useState("0.9");
	const [maxTokens, setMaxTokens] = useState("1024");
	const [seed, setSeed] = useState("1234");

	const generate = useGenerateArtifacts(worksheetId);
	const progress = usePipelineProgress("generation-progress", worksheetId);
	const download = useModelDownload();

	const percent =
		progress && progress.total > 0
			? Math.round((progress.done / progress.total) * 100)
			: 0;

	const handleSubmit = () => {
		generate.mutate(
			{
				artifactType,
				count,
				params: {
					temperature: Number(temperature) || undefined,
					top_p: Number(topP) || undefined,
					max_tokens: Number(maxTokens) || undefined,
					seed: Number(seed) || undefined,
				},
			},
			{
				onSuccess: () => onOpenChange(false),
			},
		);
	};

	const busy = generate.isPending;

	return (
		<Dialog
			open={open}
			onOpenChange={(nextOpen) => {
				if (!nextOpen && !busy) {
					onOpenChange(false);
				}
			}}
		>
			<DialogContent className="sm:max-w-lg">
				<DialogHeader>
					<DialogTitle>Generate artifact</DialogTitle>
				</DialogHeader>

				<div className="space-y-4 py-4">
					<div className="grid grid-cols-1 gap-2">
						{ARTIFACT_TYPE_OPTIONS.map((option) => {
							const active = artifactType === option.value;
							return (
								<button
									key={option.value}
									type="button"
									disabled={busy}
									onClick={() => setArtifactType(option.value)}
									className={cn(
										"flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors",
										active
											? "border-primary/50 bg-primary/5 ring-1 ring-primary/30"
											: "border-border hover:bg-muted/50",
									)}
								>
									<option.icon className="size-5 text-muted-foreground" />
									<span className="flex flex-col">
										<span className="text-sm font-medium">{option.label}</span>
										<span className="text-xs text-muted-foreground">
											{option.description}
										</span>
									</span>
								</button>
							);
						})}
					</div>

					<Field className="flex flex-row items-center justify-between gap-3">
						<Label>Number of artifacts</Label>
						<div className="flex items-center gap-2">
							<Button
								type="button"
								variant="outline"
								size="icon-sm"
								disabled={busy || count <= 1}
								onClick={() => setCount((value) => Math.max(1, value - 1))}
							>
								<IconMinus />
							</Button>
							<span className="w-8 text-center text-sm tabular-nums">
								{count}
							</span>
							<Button
								type="button"
								variant="outline"
								size="icon-sm"
								disabled={busy || count >= 10}
								onClick={() => setCount((value) => Math.min(10, value + 1))}
							>
								<IconPlus />
							</Button>
						</div>
					</Field>

					<Button
						type="button"
						variant="ghost"
						size="sm"
						className="self-start"
						disabled={busy}
						onClick={() => setAdvanced((value) => !value)}
					>
						<IconSettings />
						Advanced
						<IconChevronDown
							className={cn("transition-transform", advanced && "rotate-180")}
						/>
					</Button>

					{advanced && (
						<div className="grid grid-cols-2 gap-3">
							<Field className="flex flex-col gap-1.5">
								<Label htmlFor="temperature">Temperature</Label>
								<Input
									id="temperature"
									type="number"
									step="0.1"
									min="0"
									max="2"
									value={temperature}
									onChange={(e) => setTemperature(e.target.value)}
								/>
							</Field>
							<Field className="flex flex-col gap-1.5">
								<Label htmlFor="topP">Top P</Label>
								<Input
									id="topP"
									type="number"
									step="0.05"
									min="0"
									max="1"
									value={topP}
									onChange={(e) => setTopP(e.target.value)}
								/>
							</Field>
							<Field className="flex flex-col gap-1.5">
								<Label htmlFor="maxTokens">Max tokens</Label>
								<Input
									id="maxTokens"
									type="number"
									step="16"
									min="16"
									value={maxTokens}
									onChange={(e) => setMaxTokens(e.target.value)}
								/>
							</Field>
							<Field className="flex flex-col gap-1.5">
								<Label htmlFor="seed">Seed</Label>
								<Input
									id="seed"
									type="number"
									step="1"
									value={seed}
									onChange={(e) => setSeed(e.target.value)}
								/>
							</Field>
						</div>
					)}

					{busy && download?.active && (
						<div className="flex flex-col gap-1">
							<div className="flex items-center justify-between text-xs text-muted-foreground">
								<span className="flex items-center gap-1.5">
									<IconLoader className="size-3.5 animate-spin" />
									Downloading {modelDownloadLabel(download.kind)}
								</span>
								<span className="tabular-nums">
									{formatDownloadProgress(download.done, download.total)}
								</span>
							</div>
							<Progress
								value={
									download.total > 0
										? Math.round((download.done / download.total) * 100)
										: 0
								}
							/>
						</div>
					)}
					{busy && !download?.active && (
						<div className="flex flex-col gap-1">
							<div className="flex items-center justify-between text-xs text-muted-foreground">
								<span className="flex items-center gap-1.5">
									<IconLoader className="size-3.5 animate-spin" />
									Generating
								</span>
								<span className="tabular-nums">
									{progress?.done ?? 0}/{progress?.total ?? count}
								</span>
							</div>
							<Progress value={percent} />
						</div>
					)}

					{generate.error && (
						<p className="text-sm text-destructive">
							{toErrorMessage(generate.error)}
						</p>
					)}
				</div>

				<DialogFooter>
					<Button
						type="button"
						variant="outline"
						disabled={busy}
						onClick={() => onOpenChange(false)}
					>
						Cancel
					</Button>
					<Button type="button" disabled={busy} onClick={handleSubmit}>
						{busy ? <IconLoader className="animate-spin" /> : <IconPlus />}
						Generate
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}
