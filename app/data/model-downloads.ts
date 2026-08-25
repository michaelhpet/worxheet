import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

export type ModelDownloadKind = "embedding";

export interface ModelDownload {
	kind: ModelDownloadKind;
	done: number;
	total: number;
	active: boolean;
}

interface ModelDownloadPayload {
	kind: ModelDownloadKind;
	done: number;
	total: number;
}

const KIND_LABELS: Record<ModelDownloadKind, string> = {
	embedding: "Embedding model",
};

export function modelDownloadLabel(kind: ModelDownloadKind): string {
	return KIND_LABELS[kind];
}

/**
 * Tracks the current model artifact download (if any) reported by the Rust
 * core. `active` is true while `done < total`, so callers can switch from a
 * stale 0/0 pipeline progress to a bytes-based download bar.
 */
export function useModelDownload(): ModelDownload | null {
	const [download, setDownload] = useState<ModelDownload | null>(null);

	useEffect(() => {
		let unlisten: (() => void) | undefined;
		let disposed = false;

		listen<ModelDownloadPayload>("model-download", (e) => {
			const { kind, done, total } = e.payload;
			setDownload({ kind, done, total, active: done < total });
		}).then((unregister) => {
			if (disposed) {
				unregister();
			} else {
				unlisten = unregister;
			}
		});

		return () => {
			disposed = true;
			unlisten?.();
		};
	}, []);

	return download;
}

export function formatDownloadProgress(done: number, total: number): string {
	return `${formatMegabytes(done)} / ${formatMegabytes(total)}`;
}

function formatMegabytes(bytes: number): string {
	return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
