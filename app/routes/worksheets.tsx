import {
	AlertDialog,
	AlertDialogAction,
	AlertDialogCancel,
	AlertDialogContent,
	AlertDialogDescription,
	AlertDialogFooter,
	AlertDialogHeader,
	AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Item, ItemActions, ItemContent, ItemDescription, ItemTitle } from "@/components/ui/item";
import {
	Table,
	TableBody,
	TableCell,
	TableHead,
	TableHeader,
	TableRow,
} from "@/components/ui/table";
import { useDeleteWorksheet, useWorksheets, type Worksheet } from "@/data/worksheets";
import { IconChevronLeft, IconChevronRight, IconDotsVertical, IconFile, IconLayoutGrid, IconList, IconPlus, IconSelector, IconTable, IconTrash } from "@tabler/icons-react";
import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useState } from "react";

export const Route = createFileRoute("/worksheets")({
	component: WorksheetsPage,
});

const PER_PAGE = 20;

type ViewMode = "table" | "grid" | "list";

const VIEW_MODE_KEY = "worksheet-view-mode";

function getStoredViewMode(): ViewMode {
	const stored = localStorage.getItem(VIEW_MODE_KEY);
	if (stored === "table" || stored === "grid" || stored === "list") return stored;
	return "table";
}

const viewModes: { mode: ViewMode; icon: typeof IconTable; label: string }[] = [
	{ mode: "table", icon: IconTable, label: "Table" },
	{ mode: "grid", icon: IconLayoutGrid, label: "Grid" },
	{ mode: "list", icon: IconList, label: "List" },
];

function WorksheetsPage() {
	const [page, setPage] = useState(1);
	const { data, isLoading, error } = useWorksheets(page, PER_PAGE);
	const [viewMode, setViewMode] = useState<ViewMode>(getStoredViewMode);

	const changeView = (mode: ViewMode) => {
		localStorage.setItem(VIEW_MODE_KEY, mode);
		setViewMode(mode);
	};

	if (isLoading) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-muted-foreground">Loading worksheets...</p>
			</main>
		);
	}

	if (error) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<p className="text-destructive">Failed to load worksheets</p>
			</main>
		);
	}

	if (!data || data.total === 0) {
		return (
			<main className="w-screen h-screen flex items-center justify-center">
				<Empty>
					<EmptyHeader>
						<EmptyMedia variant="icon" className="size-16 bg-card">
							<IconFile className="size-12" />
						</EmptyMedia>
						<EmptyTitle>No worksheets yet</EmptyTitle>
						<EmptyDescription>
							Create your first worksheet to get started.
						</EmptyDescription>
					</EmptyHeader>
					<EmptyContent>
						<Link to="/worksheets/new">
							<Button variant="secondary">Create worksheet</Button>
						</Link>
					</EmptyContent>
				</Empty>
			</main>
		);
	}

	return (
		<main className="w-screen h-screen flex flex-col">
			<header className="flex items-center justify-between px-6 py-4 border-b">
				<h1 className="text-xl font-semibold">Worksheets</h1>
				<div className="flex items-center gap-3">
					<div className="flex items-center gap-1 rounded-lg border p-0.5">
						{viewModes.map(({ mode, icon: Icon, label }) => (
							<Button
								key={mode}
								variant={viewMode === mode ? "default" : "ghost"}
								size="icon-sm"
								onClick={() => changeView(mode)}
								aria-label={label}
							>
								<Icon />
							</Button>
						))}
					</div>

					<Link to="/worksheets/new">
						<Button variant="default">
							<IconPlus />
							New worksheet
						</Button>
					</Link>
				</div>
			</header>

			<div className="flex-1 overflow-auto p-6">
				{viewMode === "table" && (
					<TableView worksheets={data.items} page={page} />
				)}
				{viewMode === "grid" && <GridView worksheets={data.items} />}
				{viewMode === "list" && <ListView worksheets={data.items} />}
			</div>

			<footer className="flex items-center justify-between px-6 py-4 border-t">
				<p className="text-sm text-muted-foreground">
					Showing {Math.min((page - 1) * PER_PAGE + 1, data.total)}
					&ndash;{Math.min(page * PER_PAGE, data.total)} of {data.total}
				</p>
				<div className="flex items-center gap-2">
					<Button
						variant="outline"
						size="sm"
						disabled={page <= 1}
						onClick={() => setPage(page - 1)}
					>
						<IconChevronLeft />
						Previous
					</Button>
					<Button
						variant="outline"
						size="sm"
						disabled={page >= data.total_pages}
						onClick={() => setPage(page + 1)}
					>
						Next
						<IconChevronRight />
					</Button>
				</div>
			</footer>
		</main>
	);
}

function formatDate(iso: string): string {
	const date = new Date(iso);
	return date.toLocaleDateString(undefined, {
		year: "numeric",
		month: "short",
		day: "numeric",
	});
}

function WorksheetActions({ worksheet }: { worksheet: Worksheet }) {
	const deleteWorksheet = useDeleteWorksheet();
	const [alertOpen, setAlertOpen] = useState(false);

	return (
		<>
			<DropdownMenu>
				<DropdownMenuTrigger
					render={
						<Button
							variant="ghost"
							size="icon-xs"
							onClick={(e: React.MouseEvent) => e.stopPropagation()}
						/>
					}
				>
					<IconDotsVertical />
					<span className="sr-only">Actions</span>
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end">
					<DropdownMenuItem onClick={() => setAlertOpen(true)}>
						<IconTrash />
						Delete
					</DropdownMenuItem>
				</DropdownMenuContent>
			</DropdownMenu>
			<AlertDialog open={alertOpen} onOpenChange={setAlertOpen}>
				<AlertDialogContent>
					<AlertDialogHeader>
						<AlertDialogTitle>Delete worksheet?</AlertDialogTitle>
						<AlertDialogDescription>
							Are you sure you want to delete &ldquo;{worksheet.name}&rdquo;?
						</AlertDialogDescription>
					</AlertDialogHeader>
					<AlertDialogFooter>
						<AlertDialogCancel>Cancel</AlertDialogCancel>
						<AlertDialogAction
							variant="destructive"
							onClick={() => deleteWorksheet.mutate(worksheet.id)}
						>
							Delete
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>
		</>
	);
}

function TableView({
	worksheets,
	page,
}: {
	worksheets: Worksheet[];
	page: number;
}) {
	const navigate = useNavigate();

	return (
		<Table>
			<TableHeader>
				<TableRow>
					<TableHead className="w-12">
						<IconSelector className="size-4" />
					</TableHead>
					<TableHead>Name</TableHead>
					<TableHead>Created</TableHead>
					<TableHead>Updated</TableHead>
					<TableHead className="w-12" />
				</TableRow>
			</TableHeader>
			<TableBody>
				{worksheets.map((worksheet, index) => (
					<TableRow
						key={worksheet.id}
						className="cursor-pointer"
						onClick={() =>
							navigate({
								to: "/worksheets/$id",
								params: { id: worksheet.id },
							})
						}
					>
						<TableCell className="text-muted-foreground">
							{(page - 1) * PER_PAGE + index + 1}
						</TableCell>
						<TableCell className="font-medium">{worksheet.name}</TableCell>
						<TableCell>{formatDate(worksheet.created_at)}</TableCell>
						<TableCell>{formatDate(worksheet.updated_at)}</TableCell>
						<TableCell>
							<WorksheetActions worksheet={worksheet} />
						</TableCell>
					</TableRow>
				))}
			</TableBody>
		</Table>
	);
}

function GridView({ worksheets }: { worksheets: Worksheet[] }) {
	const navigate = useNavigate();

	return (
		<div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4">
			{worksheets.map((worksheet) => (
				<div
					key={worksheet.id}
					className="relative flex flex-col gap-2 rounded-xl border p-4 hover:bg-muted/50 transition-colors cursor-pointer"
					onClick={() =>
						navigate({
							to: "/worksheets/$id",
							params: { id: worksheet.id },
						})
					}
				>
					<div
						className="absolute top-2 right-2"
						onClick={(e) => e.stopPropagation()}
					>
						<WorksheetActions worksheet={worksheet} />
					</div>
					<IconFile className="size-8 text-muted-foreground" />
					<span className="font-medium truncate">{worksheet.name}</span>
					<span className="text-sm text-muted-foreground">
						Created {formatDate(worksheet.created_at)}
					</span>
				</div>
			))}
		</div>
	);
}

function ListView({ worksheets }: { worksheets: Worksheet[] }) {
	const navigate = useNavigate();

	return (
		<ul className="flex flex-col">
			{worksheets.map((worksheet) => (
				<Item
					key={worksheet.id}
					className="cursor-pointer border-b border-border last:border-b-0"
					onClick={() =>
						navigate({
							to: "/worksheets/$id",
							params: { id: worksheet.id },
						})
					}
				>
					<ItemContent>
						<ItemTitle>{worksheet.name}</ItemTitle>
						<ItemDescription>
							Created {formatDate(worksheet.created_at)}
						</ItemDescription>
					</ItemContent>
					<ItemActions
						className="self-center"
						onClick={(e: React.MouseEvent) => e.stopPropagation()}
					>
						<WorksheetActions worksheet={worksheet} />
					</ItemActions>
				</Item>
			))}
		</ul>
	);
}
