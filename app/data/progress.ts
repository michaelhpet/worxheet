import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

export interface PipelineProgress {
	done: number;
	total: number;
}

interface ProgressPayload {
	worksheet_id: string;
	done: number;
	total: number;
}

export function usePipelineProgress(
	event: string,
	worksheetId: string | undefined,
) {
	const [progress, setProgress] = useState<PipelineProgress | null>(null);

	useEffect(() => {
		if (!worksheetId) return;

		let unlisten: (() => void) | undefined;
		let disposed = false;

		listen<ProgressPayload>(event, (e) => {
			if (e.payload.worksheet_id !== worksheetId) return;
			setProgress({ done: e.payload.done, total: e.payload.total });
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
	}, [event, worksheetId]);

	return progress;
}
