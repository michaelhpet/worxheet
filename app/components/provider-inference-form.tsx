import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { Spinner } from "@/components/ui/spinner";
import { useProviderModels, useProviderStatus, useSetProviderConfig, type ProviderPreset } from "@/data/provider";
import { cn } from "@/lib/utils";
import { IconCheck, IconEye, IconEyeOff, IconX } from "@tabler/icons-react";
import { useForm } from "@tanstack/react-form";
import { useCallback, useEffect, useState } from "react";

const PRESETS = [
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

export function ProviderInferenceForm() {
	const [connection, setConnection] = useState<{ ok: boolean; error?: string; count?: number } | null>(null);
	const [testing, setTesting] = useState(false);
	const [showKey, setShowKey] = useState(false);
	const modelsQuery = useProviderModels(false);
	const setConfig = useSetProviderConfig();
	const { data: status, isLoading } = useProviderStatus();
	const form = useForm({
		defaultValues: {
			preset: status?.preset ?? "",
			baseUrl: status?.config.base_url ?? "",
			model: status?.config.model ?? "",
			apiKey: "",
			concurrency: status?.config.concurrency ?? 8,
		},
	});

	const persistSettings = useCallback(async () => {
		const { preset, baseUrl, model, apiKey, concurrency } = form.state.values;
		await setConfig.mutateAsync({
			preset,
			baseUrl: baseUrl.trim(),
			model: model.trim(),
			concurrency,
			apiKey: apiKey.trim() === "" ? undefined : apiKey.trim(),
		});
	}, [form.state.values, setConfig]);

	const testConnection = async () => {
		setConnection(null);
		setTesting(true);
		try {
			await persistSettings();
			const result = await modelsQuery.refetch();
			if (result.isError) return setConnection({ ok: false, error: String(result.error ?? "Connection failed.") });
			const models = result.data ?? [];
			setConnection({ ok: true, count: models.length });
			if (models.length > 0 && !form.state.values.model.trim()) {
				form.setFieldValue("model", models[0]);
				// Persist the discovered model so the status badge flips to
				// Ready without another manual step.
				await persistSettings();
			}
		} finally {
			setTesting(false);
		}
	};

	useEffect(() => {
		if (!form.state.values.preset) return;
		const timeoutId = setTimeout(persistSettings, PERSIST_DELAY_MS);
		return () => clearTimeout(timeoutId);
	}, [persistSettings, form.state.values.preset]);

	if (isLoading) {
		return <Spinner />;
	}

	return (
		<form onSubmit={(e) => e.preventDefault()} className="flex flex-col gap-5">
			<Field>
				<FieldLabel>Provider</FieldLabel>
				<form.Field name="preset">
					{(field) => (
						<Select
							value={field.state.value}
							onValueChange={(value) => {
								const next = value as ProviderPreset;
								const target = PRESETS.find((entry) => entry.value === next);
								field.handleChange(next);
								if (target) {
									form.setFieldValue("baseUrl", target.baseUrl);
									setConnection(null);
								}
								form.setFieldValue("model", next !== "custom" ? (target?.model ?? "") : "");
								// A preset with a stored key is ready to verify: ping
								// immediately so the status badge flips without a
								// manual Test connection.
								if (status?.keys_set?.[next]) {
									void testConnection();
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
					)}
				</form.Field>
				<form.Field name="preset">
					{(field) => (
						<FieldDescription>{PRESETS.find((entry) => entry.value === field.state.value)?.hint}</FieldDescription>
					)}
				</form.Field>
			</Field>

			<form.Field name="baseUrl">
				{(field) =>
					form.state.values.preset === "custom" ? (
						<Field>
							<FieldLabel>Base URL</FieldLabel>
							<Input
								placeholder="https://your-server.example.com/v1"
								value={field.state.value}
								onChange={(event) => field.handleChange(event.target.value)}
							/>
						</Field>
					) : null
				}
			</form.Field>

			<Field>
				<FieldLabel>API key</FieldLabel>
				<div className="relative">
					<form.Field name="apiKey">
						{(field) => (
							<Input
								type={showKey ? "text" : "password"}
								placeholder={status?.api_key_set ? "*****************" : "Paste your API key"}
								value={field.state.value}
								onChange={(event) => field.handleChange(event.target.value)}
								className="pr-10"
							/>
						)}
					</form.Field>
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
					disabled={!form.state.values.baseUrl.trim() || testing}
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
				<form.Field name="model">
					{(field) => (
						<Select value={field.state.value} onValueChange={(value) => field.handleChange(value ?? "")}>
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
					)}
				</form.Field>
			</Field>

			<form.Field name="concurrency">
				{(field) => (
					<Field>
						<FieldLabel>Parallel requests — {field.state.value}</FieldLabel>
						<Slider
							min={1}
							max={32}
							step={1}
							value={field.state.value}
							onValueChange={(value) => field.handleChange(Number(value))}
						/>
						<FieldDescription>
							Higher is faster; lower it if you hit provider rate limits. Free tiers often need 2–4.
						</FieldDescription>
					</Field>
				)}
			</form.Field>
		</form>
	);
}
