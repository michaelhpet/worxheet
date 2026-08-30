import { useTheme } from "@/components/theme-provider";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { useProviderModels, useProviderStatus, useSetProviderConfig, type ProviderPreset } from "@/data/provider";
import { onOpenSettings, type SettingsTab } from "@/lib/settings-bus";
import { cn } from "@/lib/utils";
import { IconBotId, IconCheck, IconEye, IconEyeOff, IconFileAi, IconPalette, IconX } from "@tabler/icons-react";
import { useCallback, useEffect, useRef, useState } from "react";
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
		label: "Ollama Cloud",
		hint: "Cloud models — needs an API key. Use Custom endpoint for local models",
		baseUrl: "https://ollama.com/v1",
		model: "",
	},
	{
		value: "lmstudio",
		label: "LM Studio",
		hint: "Local models on this machine — no key needed",
		baseUrl: "http://localhost:1234/v1",
		model: "",
	},
	{
		value: "custom",
		label: "Custom endpoint",
		hint: "Any OpenAI-compatible server",
		baseUrl: "http://localhost:11434/v1",
		model: "",
	},
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

/** Debounce before persisting live edits to reduce write churn. */
const PERSIST_DELAY_MS = 400;

export function SettingsDialog() {
	const [open, setOpen] = useState(false);
	const [tab, setTab] = useState<SettingsTab>("appearance");

	const { theme, setTheme } = useTheme();

	const { data: status } = useProviderStatus();
	const setConfig = useSetProviderConfig();

	const [preset, setPreset] = useState<ProviderPreset>("openai");
	const [baseUrl, setBaseUrl] = useState("");
	const [model, setModel] = useState("");
	const [apiKey, setApiKey] = useState("");
	const [concurrency, setConcurrency] = useState(8);
	const [connection, setConnection] = useState<{ ok: boolean; error?: string; count?: number } | null>(null);
	const [testing, setTesting] = useState(false);
	const [showKey, setShowKey] = useState(false);

	// Artifact generation knobs — frontend-only shell for now. Not persisted
	// or wired to the backend yet.
	const [temperature, setTemperature] = useState(0.7);
	const [maxTokens, setMaxTokens] = useState(2048);
	const [seed, setSeed] = useState(1234);

	// Load stored values once per dialog open, then never overwrite the
	// user's in-progress edits.
	const loadedRef = useRef(false);
	useEffect(() => {
		if (!open) {
			loadedRef.current = false;
			return;
		}
		if (loadedRef.current || !status) return;
		loadedRef.current = true;
		setPreset(status.config.preset as ProviderPreset);
		setBaseUrl(status.config.base_url);
		setModel(status.config.model);
		setConcurrency(status.config.concurrency);
		setApiKey("");
		setConnection(null);
	}, [open, status]);

	// Keep the latest form values in a ref so `persist` (below) reads the
	// current draft at call time, not a stale closure.
	const draftRef = useRef({ preset, baseUrl, model, concurrency, apiKey });
	draftRef.current = { preset, baseUrl, model, concurrency, apiKey };

	// Commit the current draft to the backend — the single source of truth the
	// test connection and generation both read. Awaitable so "Test connection"
	// can first flush the form, then list against exactly what was just saved.
	const lastSavedRef = useRef("");
	const persist = useCallback(async () => {
		const { preset, baseUrl, model, concurrency, apiKey } = draftRef.current;
		const trimmedBase = baseUrl.trim();
		const trimmedModel = model.trim();
		const trimmedKey = apiKey.trim();
		const payload = `${preset}\u0000${trimmedBase}\u0000${trimmedModel}\u0000${concurrency}\u0000${trimmedKey}`;
		if (payload === lastSavedRef.current) return;
		lastSavedRef.current = payload;
		await setConfig.mutateAsync({
			preset,
			baseUrl: trimmedBase,
			model: trimmedModel,
			concurrency,
			apiKey: trimmedKey === "" ? undefined : trimmedKey,
		});
	}, [setConfig]);

	// Persist live edits, debounced, once the form has been populated. The
	// draft fields intentionally reset the debounce timer on each edit, even
	// though `persist` reads them via a ref.
	const persistTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
	// biome-ignore lint/correctness/useExhaustiveDependencies: debounce timer reset
	useEffect(() => {
		if (!loadedRef.current || !open) return;
		if (persistTimerRef.current) clearTimeout(persistTimerRef.current);
		persistTimerRef.current = setTimeout(() => {
			void persist();
		}, PERSIST_DELAY_MS);

		return () => {
			if (persistTimerRef.current) clearTimeout(persistTimerRef.current);
		};
	}, [open, preset, baseUrl, model, concurrency, apiKey, persist]);

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

	// Models are only fetched after a successful connection (or an explicit
	// refresh); listing them doubles as the connection test.
	const modelsQuery = useProviderModels(false);

	// "Test connection" first flushes the in-progress form edits to the
	// backend, then lists against that freshly-persisted config — the same
	// state generation reads. Keeps a single source of truth (no separate
	// test path).
	const testConnection = async () => {
		setConnection(null);
		setTesting(true);
		try {
			await persist();
			const result = await modelsQuery.refetch();
			if (result.isError) {
				setConnection({ ok: false, error: String(result.error ?? "Connection failed.") });
				return;
			}
			const models = result.data ?? [];
			setConnection({ ok: true, count: models.length });
			if (models.length > 0 && !model.trim()) {
				setModel(models[0]);
			}
		} finally {
			setTesting(false);
		}
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
												const target = PRESETS.find((entry) => entry.value === next);
												setPreset(next);
												if (target) {
													setBaseUrl(target.baseUrl);
													setConnection(null);
												}
												if (next !== "custom") {
													setModel(target?.model ?? "");
												} else {
													setModel("");
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

									<Field>
										<FieldLabel>API key</FieldLabel>
										<div className="relative">
											<Input
												type={showKey ? "text" : "password"}
												placeholder={status?.api_key_set ? "*****************" : "Paste your API key"}
												value={apiKey}
												onChange={(event) => setApiKey(event.target.value)}
												className="pr-10"
											/>
											<button
												type="button"
												tabIndex={-1}
												onClick={() => setShowKey((value) => !value)}
												className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
												aria-label={showKey ? "Hide API key" : "Show API key"}
											>
												{showKey ? <IconEyeOff className="size-4" /> : <IconEye className="size-4" />}
											</button>
										</div>
										<FieldDescription>
											{status?.api_key_set
												? "A key is stored for this provider. Entering a new one replaces it."
												: "No key stored for this provider. Stored only in this device's keychain."}
										</FieldDescription>
									</Field>

									<Field>
										<FieldLabel>Test connection</FieldLabel>
										<Button
											type="button"
											variant="outline"
											onClick={testConnection}
											disabled={!baseUrl.trim() || testing}
										>
											{testing ? <Spinner /> : null}
											{testing ? "Connecting…" : "Test connection"}
										</Button>
										{connection && (
											<div
												className={cn(
													"flex items-center gap-2 rounded-md border px-3 py-2 text-sm",
													connection.ok
														? "border-green-600/30 bg-green-500/10 text-green-700 dark:text-green-400"
														: "border-destructive/30 bg-destructive/10 text-destructive",
												)}
											>
												{connection.ok ? <IconCheck className="size-4" /> : <IconX className="size-4" />}
												{connection.ok
													? connection.count && connection.count > 0
														? `Connection verified — ${connection.count} ${connection.count === 1 ? "model" : "models"} available.`
														: "Connection verified."
													: (connection.error ?? "Connection failed.")}
											</div>
										)}
									</Field>

									<Field>
										<FieldLabel>Model</FieldLabel>
										<Select value={model} onValueChange={(value) => setModel(value ?? "")}>
											<SelectTrigger>
												<SelectValue placeholder="Run Test connection to load models" />
											</SelectTrigger>
											<SelectContent>
												{(modelsQuery.data ?? []).map((id) => (
													<SelectItem key={id} value={id}>
														{id}
													</SelectItem>
												))}
											</SelectContent>
										</Select>
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
					</div>
				</SidebarProvider>
			</DialogContent>
		</Dialog>
	);
}
