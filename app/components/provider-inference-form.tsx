import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { useProviderModels, useProviderStatus, useSetProviderConfig, type ProviderPreset } from "@/data/provider";
import { cn } from "@/lib/utils";
import { IconCheck, IconEye, IconEyeOff, IconX } from "@tabler/icons-react";
import { useCallback, useEffect, useRef, useState } from "react";

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

const PERSIST_DELAY_MS = 400;

interface ProviderInferenceFormProps {
	onTestResult?: (ok: boolean) => void;
}

export function ProviderInferenceForm({ onTestResult }: ProviderInferenceFormProps) {
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

	const loadedRef = useRef(false);
	useEffect(() => {
		if (loadedRef.current || !status) return;
		loadedRef.current = true;
		setPreset(status.config.preset as ProviderPreset);
		setBaseUrl(status.config.base_url);
		setModel(status.config.model);
		setConcurrency(status.config.concurrency);
		setApiKey("");
		setConnection(null);
	}, [status]);

	const draftRef = useRef({ preset, baseUrl, model, concurrency, apiKey });
	draftRef.current = { preset, baseUrl, model, concurrency, apiKey };

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

	const persistTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

	// biome-ignore lint/correctness/useExhaustiveDependencies: FIXME
	useEffect(() => {
		if (!loadedRef.current) return;
		if (persistTimerRef.current) clearTimeout(persistTimerRef.current);
		persistTimerRef.current = setTimeout(() => {
			void persist();
		}, PERSIST_DELAY_MS);

		return () => {
			if (persistTimerRef.current) clearTimeout(persistTimerRef.current);
		};
	}, [preset, baseUrl, model, concurrency, apiKey, persist]);

	const modelsQuery = useProviderModels(false);

	const testConnection = async () => {
		setConnection(null);
		setTesting(true);
		try {
			await persist();
			const result = await modelsQuery.refetch();
			if (result.isError) {
				setConnection({ ok: false, error: String(result.error ?? "Connection failed.") });
				onTestResult?.(false);
				return;
			}
			const models = result.data ?? [];
			setConnection({ ok: true, count: models.length });
			if (models.length > 0 && !model.trim()) {
				setModel(models[0]);
			}
			onTestResult?.(true);
		} finally {
			setTesting(false);
		}
	};

	return (
		<div className="flex flex-col gap-5">
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
				<Button type="button" variant="outline" onClick={testConnection} disabled={!baseUrl.trim() || testing}>
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
	);
}
