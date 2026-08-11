/**
 * Normalize a rejected IPC value into a readable message. Tauri rejects command
 * errors with the raw value (a plain string for `Result<_, String>` commands),
 * which has no `.message` property.
 */
export function toErrorMessage(error: unknown): string {
	if (error instanceof Error) {
		return error.message;
	}
	if (typeof error === "string") {
		return error;
	}
	return String(error);
}
