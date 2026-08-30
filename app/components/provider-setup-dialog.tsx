import { ProviderInferenceForm } from "@/components/provider-inference-form";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { useProviderStatus, useValidateProvider, isProviderReady } from "@/data/provider";
import { useEffect, useRef, useState } from "react";

/**
 * Provider/model setup wizard shown at launch whenever the LLM provider is
 * missing or unhealthy:
 *  - nothing configured → open immediately
 *  - configured → ping the provider once; open if the connection fails
 *
 * Dismissible per session (the close button hides it for this launch; it
 * re-evaluates on the next launch). A successful "Test connection" inside the
 * wizard re-pings and dismisses it.
 */
export function ProviderSetupDialog() {
	const { data: status, isLoading } = useProviderStatus();
	const validate = useValidateProvider();
	const [open, setOpen] = useState(false);

	// Decide once per mount (i.e. once per app launch).
	const checkedRef = useRef(false);
	useEffect(() => {
		if (checkedRef.current) return;
		if (isLoading || !status) return;

		checkedRef.current = true;
		if (!isProviderReady(status)) {
			// No provider/model selected yet — guide setup.
			setOpen(true);
			return;
		}

		// Configured: ping the connection and open only if it's unhealthy.
		validate.mutate(undefined, {
			onSuccess: (result) => {
				if (!result.ok) setOpen(true);
			},
			onError: () => setOpen(true),
		});
	}, [isLoading, status, validate]);

	return (
		<Dialog open={open} onOpenChange={setOpen}>
			<DialogContent className="sm:max-w-lg">
				<DialogHeader>
					<DialogTitle>Connect your LLM provider</DialogTitle>
					<DialogDescription>
						Worxheet needs a working provider to generate artifacts. Set one up below, or close to skip for now.
					</DialogDescription>
				</DialogHeader>
				<ProviderInferenceForm
					onTestResult={(ok) => {
						if (ok) setOpen(false);
					}}
				/>
			</DialogContent>
		</Dialog>
	);
}
