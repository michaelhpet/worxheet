import { useEffect, useRef, useState } from "react";

/**
 * Ticks down from `totalSeconds` to zero, updating ~4x per second without
 * drifting (the deadline is fixed at start). Returns the remaining seconds,
 * or `null` while no countdown is active. `onExpire` fires exactly once when
 * zero is reached; changing its identity never restarts the clock.
 */
export function useCountdown(totalSeconds: number | undefined, onExpire?: () => void): number | null {
	const [remaining, setRemaining] = useState<number | null>(null);
	const firedRef = useRef(false);
	const onExpireRef = useRef(onExpire);
	onExpireRef.current = onExpire;

	useEffect(() => {
		if (!totalSeconds || totalSeconds <= 0) return;
		const deadline = Date.now() + totalSeconds * 1000;
		firedRef.current = false;
		setRemaining(totalSeconds);

		let interval: ReturnType<typeof setInterval> | undefined;
		const stop = () => {
			if (interval !== undefined) {
				clearInterval(interval);
				interval = undefined;
			}
		};

		const tick = () => {
			const left = Math.max(0, Math.ceil((deadline - Date.now()) / 1000));
			setRemaining(left);
			if (left > 0) return;
			stop();
			if (firedRef.current) return;
			firedRef.current = true;
			onExpireRef.current?.();
		};

		tick();
		interval = setInterval(tick, 250);
		return stop;
	}, [totalSeconds]);

	return remaining;
}
