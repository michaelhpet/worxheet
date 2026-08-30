import { ProviderInferenceForm } from "@/components/provider-inference-form";
import { useTheme } from "@/components/theme-provider";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Field, FieldDescription, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import { onOpenSettings, type SettingsTab } from "@/lib/settings-bus";
import { IconBotId, IconFileAi, IconPalette } from "@tabler/icons-react";
import { useEffect, useState } from "react";
import { Sidebar, SidebarContent, SidebarGroup, SidebarMenuButton, SidebarProvider } from "./ui/sidebar";

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
	const [tab, setTab] = useState<SettingsTab>("appearance");

	const { theme, setTheme } = useTheme();

	// Artifact generation knobs — frontend-only shell for now. Not persisted
	// or wired to the backend yet.
	const [temperature, setTemperature] = useState(0.7);
	const [maxTokens, setMaxTokens] = useState(2048);
	const [seed, setSeed] = useState(1234);

	// Open from anywhere: gear buttons, blocked flows, native menu event.
	useEffect(() => {
		return onOpenSettings((requested) => {
			setTab(requested ?? "appearance");
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
					setTab("appearance");
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

									<ProviderInferenceForm />
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
