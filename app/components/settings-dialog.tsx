import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { useTheme } from "@/components/theme-provider";
import {
	useProviderModels,
	useProviderStatus,
	useSetProviderConfig,
	useValidateProvider,
	type ProviderPreset,
} from "@/data/provider";
import { onOpenSettings, type SettingsTab } from "@/lib/settings-bus";
import { cn } from "@/lib/utils";
import { IconBotId, IconCheck, IconFileAi, IconPalette, IconX } from "@tabler/icons-react";
import { useEffect, useRef, useState } from "react";
import { Sidebar, SidebarContent, SidebarGroup, SidebarMenuButton, SidebarProvider } from "./ui/sidebar";

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

const NAV: { tab: SettingsTab; label: string; icon: typeof IconPalette }[] = [
	{ tab: "appearance", label: "Appearance", icon: IconPalette },
	{ tab: "inference", label: "Inference", icon: IconBotId },
	{ tab: "artifacts", label: "Artifacts", icon: IconFileAi },
];

const THEME_OPTIONS: { value: "dark" | "light" | "system"; label: string; hint: string }[] = [
	{ value: "light", label: "Light", hint: "Bright, high-contrast interface." },
	{ value: "dark", label: "Dark", hint: "Easy on the eyes in low light." },
	{ value: "system", label: "System", hint: "Follows your OS appearance setting." },
];

export function SettingsDialog() {
	const [open, setOpen] = useState(false);
	const [tab, setTab] = useState<SettingsTab>("inference");

	const { theme, setTheme } = useTheme();

	const { data: status, refetch } = useProviderStatus();
	const setConfig = useSetProviderConfig();
	const validate = useValidateProvider();

	const [preset, setPreset] = useState<ProviderPreset>("openai");
	const [baseUrl, setBaseUrl] = useState("");
	const [model, setModel] = useState("");
	const [apiKey, setApiKey] = useState("");
	const [concurrency, setConcurrency] = useState(8);
	const [validation, setValidation] = useState<{ ok: boolean; error?: string } | null>(null);

	// Artifact generation knobs — frontend-only shell for now. Not persisted
	// or wired to the backend yet.
	const [temperature, setTemperature] = useState(0.7);
	const [maxTokens, setMaxTokens] = useState(2048);
	const [seed, setSeed] = useState(1234);

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
			setTab(requested ?? "inference");
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
					setTab("inference");
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

	// First-run: nothing configured yet → open straight to the inference tab.
	const autoOpened = useRef(false);
	useEffect(() => {
		if (autoOpened.current || !status) return;
		autoOpened.current = true;
		const needsKey = status.config.preset !== "ollama" && status.config.preset !== "lmstudio";
		const ready = Boolean(status.config.base_url && status.config.model && (!needsKey || status.api_key_set));
		if (!ready) {
			setTab("inference");
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
		<Dialog
			open={open}
			onOpenChange={(nextOpen, details) => {
				// Only the close button may dismiss the dialog; ignore outside
				// clicks and the Escape key.
				if (!nextOpen && details?.reason !== "close-press") return;
				setOpen(nextOpen);
			}}
		>
			<DialogContent className="sm:max-w-3xl h-[70dvh] p-0 overflow-hidden">
				<SidebarProvider className="h-full min-h-0" style={{ "--sidebar-width": "180px" } as React.CSSProperties}>
					<Sidebar collapsible="none" className="border-r w-45 shrink-0">
						<SidebarContent>
							<SidebarGroup>
								{NAV.map(({ tab: key, label, icon: Icon }) => (
									<SidebarMenuButton key={key} isActive={tab === key} onClick={() => setTab(key)}>
										<Icon />
										{label}
									</SidebarMenuButton>
								))}
							</SidebarGroup>
						</SidebarContent>
					</Sidebar>

					<div className="grow flex flex-col min-w-0">
						<div className="grow overflow-y-auto px-6 py-6">
							{tab === "appearance" && (
								<div className="flex flex-col gap-4">
									<div>
										<h2 className="text-base font-medium">Appearance</h2>
										<p className="mt-1 text-sm text-muted-foreground">Choose how Worxheet looks.</p>
									</div>
									<Field>
										<FieldLabel>Theme</FieldLabel>
										<Select value={theme} onValueChange={(value) => setTheme(value as typeof theme)}>
											<SelectTrigger>
												<SelectValue />
											</SelectTrigger>
											<SelectContent>
												{THEME_OPTIONS.map((option) => (
													<SelectItem key={option.value} value={option.value}>
														{option.label}
													</SelectItem>
												))}
											</SelectContent>
										</Select>
										<FieldDescription>{THEME_OPTIONS.find((option) => option.value === theme)?.hint}</FieldDescription>
									</Field>
								</div>
							)}

							{tab === "inference" && (
								<div className="flex flex-col gap-5">
									<div>
										<h2 className="text-base font-medium">Inference</h2>
										<p className="mt-1 text-sm text-muted-foreground">
											Connect an LLM provider used to generate artifacts.
										</p>
									</div>

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

									<Field>
										<FieldLabel>Parallel requests — {concurrency}</FieldLabel>
										<Slider
											min={1}
											max={32}
											step={1}
											value={concurrency}
											onValueChange={(value) => setConcurrency(Number(value))}
										/>
										<FieldDescription>
											Higher is faster; lower it if you hit provider rate limits. Free tiers often need 2–4.
										</FieldDescription>
									</Field>

									{validation && (
										<div
											className={cn(
												"flex items-center gap-2 rounded-md border px-3 py-2 text-sm",
												validation.ok
													? "border-green-600/30 bg-green-500/10 text-green-700 dark:text-green-400"
													: "border-destructive/30 bg-destructive/10 text-destructive",
											)}
										>
											{validation.ok ? <IconCheck className="size-4" /> : <IconX className="size-4" />}
											{validation.ok ? "Connection verified." : (validation.error ?? "Validation failed.")}
										</div>
									)}
								</div>
							)}

							{tab === "artifacts" && (
								<div className="flex flex-col gap-5">
									<div>
										<h2 className="text-base font-medium">Artifacts</h2>
										<p className="mt-1 text-sm text-muted-foreground">
											Tune how generated study artifacts are produced.
										</p>
									</div>

									<Field>
										<FieldLabel>Creativity (temperature) — {temperature.toFixed(1)}</FieldLabel>
										<Slider
											min={0}
											max={2}
											step={0.1}
											value={temperature}
											onValueChange={(value) => setTemperature(Number(value))}
										/>
										<FieldDescription>
											Lower is more factual and predictable; higher is more varied. Quiz questions stay grounded so
											facts are never invented.
										</FieldDescription>
									</Field>

									<Field>
										<FieldLabel>Max output tokens — {maxTokens}</FieldLabel>
										<Slider
											min={512}
											max={4096}
											step={256}
											value={maxTokens}
											onValueChange={(value) => setMaxTokens(Number(value))}
										/>
										<FieldDescription>
											Ceiling on how much a single generation request may write. Summaries and mind maps are capped
											lower automatically.
										</FieldDescription>
									</Field>

									<Field>
										<FieldLabel>Random seed</FieldLabel>
										<Input type="number" value={seed} onChange={(event) => setSeed(Number(event.target.value))} />
										<FieldDescription>
											Fixing the seed makes regeneration reproducible. Unset to vary results each run.
										</FieldDescription>
									</Field>

									<div className="rounded-md border border-dashed px-3 py-2 text-sm text-muted-foreground">
										These generation settings are not wired up yet — they're a preview of what's coming.
									</div>
								</div>
							)}
						</div>

						{tab === "inference" && (
							<DialogFooter className="shrink-0 gap-3 border-t px-6 py-4">
								<Button variant="ghost" onClick={runValidation} disabled={!baseUrl || !model || validate.isPending}>
									{validate.isPending ? <Spinner /> : null}
									Test connection
								</Button>
								<Button onClick={save} disabled={!baseUrl || !model || !dirty || setConfig.isPending}>
									{setConfig.isPending ? <Spinner /> : null}
									Save
								</Button>
							</DialogFooter>
						)}
					</div>
				</SidebarProvider>
			</DialogContent>
		</Dialog>
	);
}
