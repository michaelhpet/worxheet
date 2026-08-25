import { Button } from "@/components/ui/button";
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import {
	useProviderModels,
	useProviderStatus,
	useSetProviderConfig,
	useValidateProvider,
	type ProviderPreset,
} from "@/data/provider";
import { onOpenSettings, type SettingsTab } from "@/lib/settings-bus";
import { IconCheck, IconSettings, IconX } from "@tabler/icons-react";
import { useEffect, useRef, useState } from "react";

const PRESETS: { value: ProviderPreset; label: string; hint: string; baseUrl: string; model: string }[] = [
	{
		value: "openai",
		label: "OpenAI",
		hint: "Get a key at platform.openai.com/api-keys",
		baseUrl: "https://api.openai.com/v1",
		model: "gpt-4o-mini",
	},
	{
		value: "gemini",
		label: "Google Gemini",
		hint: "Get a key at aistudio.google.com/apikey",
		baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
		model: "gemini-2.0-flash",
	},
	{
		value: "ollama",
		label: "Ollama (local)",
		hint: "Runs on this machine — no key needed",
		baseUrl: "http://localhost:11434/v1",
		model: "",
	},
	{
		value: "lmstudio",
		label: "LM Studio (local)",
		hint: "Runs on this machine — no key needed",
		baseUrl: "http://localhost:1234/v1",
		model: "",
	},
	{ value: "custom", label: "Custom endpoint", hint: "Any OpenAI-compatible server", baseUrl: "", model: "" },
];

export function SettingsDialog() {
	const [open, setOpen] = useState(false);
	const [tab, setTab] = useState<SettingsTab>("provider");

	const { data: status, refetch } = useProviderStatus();
	const setConfig = useSetProviderConfig();
	const validate = useValidateProvider();

	const [preset, setPreset] = useState<ProviderPreset>("openai");
	const [baseUrl, setBaseUrl] = useState("");
	const [model, setModel] = useState("");
	const [apiKey, setApiKey] = useState("");
	const [concurrency, setConcurrency] = useState(8);
	const [validation, setValidation] = useState<{ ok: boolean; error?: string } | null>(null);

	// Load current values whenever the dialog opens.
	useEffect(() => {
		if (!open || !status) return;
		setPreset(status.config.preset as ProviderPreset);
		setBaseUrl(status.config.base_url);
		setModel(status.config.model);
		setConcurrency(status.config.concurrency);
		setApiKey("");
		setValidation(null);
	}, [open, status]);

	// Open from anywhere: gear buttons, blocked flows, native menu event.
	useEffect(() => {
		return onOpenSettings((requested) => {
			setTab(requested ?? "provider");
			setOpen(true);
		});
	}, []);

	// Relay the native macOS Preferences… menu item.
	useEffect(() => {
		let unlisten: (() => void) | undefined;
		let disposed = false;

		import("@tauri-apps/api/event")
			.then(({ listen }) =>
				listen("settings:open", () => {
					setTab("provider");
					setOpen(true);
				}),
			)
			.then((unregister) => {
				if (disposed) unregister();
				else unlisten = unregister;
			});

		return () => {
			disposed = true;
			unlisten?.();
		};
	}, []);

	// First-run: nothing configured yet → open straight to the provider tab.
	const autoOpened = useRef(false);
	useEffect(() => {
		if (autoOpened.current || !status) return;
		autoOpened.current = true;
		const needsKey = status.config.preset !== "ollama" && status.config.preset !== "lmstudio";
		const ready = Boolean(status.config.base_url && status.config.model && (!needsKey || status.api_key_set));
		if (!ready) {
			setTab("provider");
			setOpen(true);
		}
	}, [status]);

	const modelsQuery = useProviderModels(open);

	const needsKey = preset !== "ollama" && preset !== "lmstudio";
	const dirty =
		status != null &&
		(status.config.preset !== preset ||
			status.config.base_url !== baseUrl ||
			status.config.model !== model ||
			status.config.concurrency !== concurrency ||
			apiKey.trim() !== "");

	const save = async () => {
		await setConfig.mutateAsync({
			preset,
			base_url: baseUrl.trim(),
			model: model.trim(),
			concurrency,
			api_key: apiKey.trim() === "" ? undefined : apiKey.trim(),
		});
		refetch();
	};

	const runValidation = async () => {
		// Validation runs against the *stored* config, so persist unsaved edits first.
		if (dirty) await save();
		const result = await validate.mutateAsync();
		setValidation({ ok: result.ok, error: result.error ?? undefined });
	};

	return (
		<Dialog open={open} onOpenChange={setOpen}>
			<DialogContent className="sm:max-w-2xl">
				<DialogHeader>
					<DialogTitle className="flex items-center gap-2">
						<IconSettings className="size-4" />
						Preferences
					</DialogTitle>
					<DialogDescription>
						Material stays on this device until a worksheet is generated. Generation runs through your configured LLM
						provider.
					</DialogDescription>
				</DialogHeader>

				<Tabs value={tab} onValueChange={(value) => setTab(value as SettingsTab)}>
					<TabsList>
						<TabsTrigger value="provider">Provider</TabsTrigger>
						<TabsTrigger value="generation">Generation</TabsTrigger>
					</TabsList>

					<TabsContent value="provider" className="flex flex-col gap-5 pt-4">
						<Field>
							<FieldLabel>Provider</FieldLabel>
							<Select
								value={preset}
								onValueChange={(value) => {
									const next = value as ProviderPreset;
									setPreset(next);
									if (next !== "custom") {
										const target = PRESETS.find((entry) => entry.value === next);
										if (target) {
											setBaseUrl(target.baseUrl);
											setModel(target.model);
										}
									}
								}}
							>
								<SelectTrigger>
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									{PRESETS.map((entry) => (
										<SelectItem key={entry.value} value={entry.value}>
											{entry.label}
										</SelectItem>
									))}
								</SelectContent>
							</Select>
							<FieldDescription>{PRESETS.find((entry) => entry.value === preset)?.hint}</FieldDescription>
						</Field>

						{preset === "custom" && (
							<Field>
								<FieldLabel>Base URL</FieldLabel>
								<Input
									placeholder="https://your-server.example.com/v1"
									value={baseUrl}
									onChange={(event) => setBaseUrl(event.target.value)}
								/>
							</Field>
						)}

						{needsKey && (
							<Field>
								<FieldLabel>API key</FieldLabel>
								<Input
									type="password"
									placeholder={status?.api_key_set ? "Stored in your OS keychain" : "Paste your API key"}
									value={apiKey}
									onChange={(event) => setApiKey(event.target.value)}
								/>
								<FieldDescription>
									{status?.api_key_set
										? "Leave blank to keep the stored key."
										: "Stored only in this device's keychain."}
								</FieldDescription>
							</Field>
						)}

						<Field>
							<FieldLabel>Model</FieldLabel>
							<div className="flex gap-2">
								<Input
									placeholder="model id, e.g. gpt-4o-mini"
									value={model}
									onChange={(event) => setModel(event.target.value)}
									list="provider-models"
								/>
								<datalist id="provider-models">
									{(modelsQuery.data ?? []).map((id) => (
										<option key={id} value={id} />
									))}
								</datalist>
								<Button type="button" variant="outline" onClick={() => modelsQuery.refetch()}>
									{modelsQuery.isFetching ? <Spinner /> : "Fetch"}
								</Button>
							</div>
						</Field>

						{validation && (
							<div
								className={`flex items-center gap-2 rounded-md border px-3 py-2 text-sm ${
									validation.ok
										? "border-green-600/30 bg-green-500/10 text-green-700 dark:text-green-400"
										: "border-destructive/30 bg-destructive/10 text-destructive"
								}`}
							>
								{validation.ok ? <IconCheck className="size-4" /> : <IconX className="size-4" />}
								{validation.ok ? "Connection verified." : (validation.error ?? "Validation failed.")}
							</div>
						)}
					</TabsContent>

					<TabsContent value="generation" className="flex flex-col gap-5 pt-4">
						<Field>
							<FieldLabel>Parallel requests — {concurrency}</FieldLabel>
							<input
								type="range"
								min={1}
								max={32}
								value={concurrency}
								onChange={(event) => setConcurrency(Number(event.target.value))}
								className="accent-primary w-full"
							/>
							<FieldDescription>
								Higher is faster; lower it if you hit provider rate limits. Free tiers often need 2–4.
							</FieldDescription>
						</Field>
					</TabsContent>
				</Tabs>

				<DialogFooter className="gap-3">
					<Button variant="ghost" onClick={runValidation} disabled={!baseUrl || !model || validate.isPending}>
						{validate.isPending ? <Spinner /> : null}
						Test connection
					</Button>
					<Button onClick={save} disabled={!baseUrl || !model || !dirty || setConfig.isPending}>
						{setConfig.isPending ? <Spinner /> : null}
						Save
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	);
}
