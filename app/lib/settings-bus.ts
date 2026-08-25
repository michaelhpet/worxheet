/**
 * Tiny app-wide bus for opening the settings dialog from anywhere:
 * the home-page gear button, blocked worksheet creation flows, and the
 * native macOS "Preferences…" menu item (relayed by the Rust core).
 */

export type SettingsTab = "provider" | "generation";

type OpenHandler = (tab?: SettingsTab) => void;

const TARGET = new EventTarget();
const OPEN_EVENT = "open";

export function openSettings(tab?: SettingsTab): void {
	TARGET.dispatchEvent(new CustomEvent(OPEN_EVENT, { detail: { tab } }));
}

export function onOpenSettings(handler: OpenHandler): () => void {
	const listener = (event: Event) => {
		const detail = (event as CustomEvent<{ tab?: SettingsTab }>).detail;
		handler(detail?.tab);
	};
	TARGET.addEventListener(OPEN_EVENT, listener);
	return () => TARGET.removeEventListener(OPEN_EVENT, listener);
}
