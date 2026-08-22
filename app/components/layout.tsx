import type { PropsWithChildren } from "react";
import { Button } from "./ui/button";
import { IconArrowLeft } from "@tabler/icons-react";

interface Props extends PropsWithChildren {
	header?: React.ReactNode;
	onBack?: () => void;
}

export function Layout({ header, children, onBack }: Props) {
	return (
		<main className="w-screen flex flex-col">
			<header className="sticky top-0 w-full h-14 flex items-center justify-center gap-2 px-4 bg-background">
				{!!onBack && (
					<Button size="icon" variant="secondary" onClick={onBack} className="absolute left-3">
						<IconArrowLeft />
					</Button>
				)}
				{header}
			</header>
			{children}
		</main>
	);
}
