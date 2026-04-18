import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";
import { FILE_SIZES } from "./constants";

export function cn(...inputs: ClassValue[]) {
	return twMerge(clsx(inputs));
}

export function formatBytes(bytes: number, decimals = 2) {
	if (bytes === 0) return "0B";
	const byteUnit = 1024;
	const fractionDigits = decimals < 0 ? 0 : decimals;
	const sizeUnitIndex = Math.floor(Math.log(bytes) / Math.log(byteUnit));
	return `${parseFloat((bytes / byteUnit ** sizeUnitIndex).toFixed(fractionDigits))}${FILE_SIZES[sizeUnitIndex]}`;
}
