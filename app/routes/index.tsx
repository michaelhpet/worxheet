import { CreateWorksheetDialog } from "@/components/create-worksheet-dialog";
import { Layout } from "@/components/layout";
import { PipelineStatusBadge } from "@/components/pipeline-status-badge";
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
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Field } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
	Pagination,
	PaginationContent,
	PaginationEllipsis,
	PaginationItem,
	PaginationLink,
	PaginationNext,
	PaginationPrevious,
} from "@/components/ui/pagination";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useRetryPipeline, usePipelineProgressListener } from "@/data/pipeline";
import { useDeleteWorksheet, useWorksheets, type Worksheet } from "@/data/worksheets";
import {
	IconDotsVertical,
	IconExternalLink,
	IconFile,
	IconLoader,
	IconPlus,
	IconPlayerStop,
	IconRotateClockwise,
	IconSelector,
	IconTrash,
} from "@tabler/icons-react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useState } from "react";

export const Route = createFileRoute("/")({
	component: Home,
});

const PER_PAGE = 20;

type PageItem = number | "ellipsis";

function paginationItems(current: number, total: number): PageItem[] {
	if (total <= 7) {
		return Array.from({ length: total }, (_, i) => i + 1);
	}
	const items: PageItem[] = [1];
	if (current > 3) items.push("ellipsis");
	for (let p = Math.max(2, current - 1); p <= Math.min(total - 1, current + 1); p++) {
		items.push(p);
	}
	if (current < total - 2) items.push("ellipsis");
	items.push(total);
	return items;
}

function Home() {
	const [page, setPage] = useState(1);
	const { data, isLoading, error } = useWorksheets(page, PER_PAGE);
	const navigate = useNavigate();
	const [createDialogOpen, setCreateDialogOpen] = useState(false);
	const [search, setSearch] = useState("");
	usePipelineProgressListener();

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
			<>
				<main className="w-screen h-screen flex items-center justify-center">
					<Empty>
						<EmptyHeader>
							<EmptyMedia variant="icon" className="size-16 bg-card">
								<IconFile className="size-12" />
							</EmptyMedia>
							<EmptyTitle>No worksheets yet</EmptyTitle>
							<EmptyDescription>Create your first worksheet to get started.</EmptyDescription>
						</EmptyHeader>
						<EmptyContent>
							<Button variant="secondary" onClick={() => setCreateDialogOpen(true)}>
								Create worksheet
							</Button>
						</EmptyContent>
					</Empty>
				</main>
				<CreateWorksheetDialog open={createDialogOpen} onOpenChange={setCreateDialogOpen} />
			</>
		);
	}

	const searchTerm = search.trim().toLowerCase();
	const visible = searchTerm
		? data.items.filter((worksheet) => worksheet.name.toLowerCase().includes(searchTerm))
		: data.items;

	return (
		<>
			<Layout
				header={
					<Field orientation="horizontal" className="w-max mx-auto">
						<Input
							type="search"
							placeholder="Search worksheets..."
							className="w-64"
							value={search}
							onChange={(e) => setSearch(e.target.value)}
						/>
						<Button type="button" onClick={() => setCreateDialogOpen(true)}>
							<IconPlus />
							New worksheet
						</Button>
					</Field>
				}
			>
				<div className="w-full max-w-200 mx-auto flex flex-col gap-3 flex-1 min-h-0 pt-3">
					<div className="border rounded-lg overflow-hidden">
						<Table>
							<TableHeader className="bg-muted">
								<TableRow>
									<TableHead className="flex items-center justify-center">
										<IconSelector className="size-4" />
									</TableHead>
									<TableHead>Name</TableHead>
									<TableHead>Materials</TableHead>
									<TableHead>Artifacts</TableHead>
									<TableHead>Created</TableHead>
									<TableHead>Status</TableHead>
									<TableHead className="w-12" />
								</TableRow>
							</TableHeader>
							<TableBody>
								{visible.length === 0 && (
									<TableRow>
										<TableCell colSpan={7} className="py-8 text-center text-muted-foreground">
											No worksheets match &ldquo;{search}&rdquo;
										</TableCell>
									</TableRow>
								)}
								{visible.map((worksheet, index) => (
									<TableRow
										key={worksheet.id}
										className="cursor-pointer"
										onClick={() => navigate({ to: "/worksheets/$id", params: { id: worksheet.id } })}
									>
										<TableCell className="text-center text-muted-foreground">
											{(page - 1) * PER_PAGE + index + 1}
										</TableCell>
										<TableCell className="font-medium">{worksheet.name}</TableCell>
										<TableCell className="whitespace-nowrap">
											<FilesCell worksheet={worksheet} />
										</TableCell>
										<TableCell className="whitespace-nowrap text-muted-foreground">
											<QuizCounts worksheet={worksheet} />
										</TableCell>
										<TableCell>{formatDate(worksheet.created_at)}</TableCell>
										<TableCell>
											<PipelineStatusBadge worksheet={worksheet} />
										</TableCell>
										<TableCell onPointerDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
											<WorksheetActions worksheet={worksheet} />
										</TableCell>
									</TableRow>
								))}
							</TableBody>
						</Table>
					</div>
					<footer className="flex items-center justify-between">
						<p className="text-sm text-muted-foreground">
							Showing {Math.min((page - 1) * PER_PAGE + 1, data.total)}
							&ndash;{Math.min(page * PER_PAGE, data.total)} of {data.total}
						</p>
						<Pagination className="mx-0 w-auto">
							<PaginationContent>
								<PaginationItem>
									<PaginationPrevious
										aria-disabled={page <= 1}
										onClick={(e) => {
											e.preventDefault();
											if (page > 1) setPage(page - 1);
										}}
									/>
								</PaginationItem>
								{paginationItems(page, data.total_pages).map((item, index) =>
									item === "ellipsis" ? (
										<PaginationItem key={`ellipsis-${index}`}>
											<PaginationEllipsis />
										</PaginationItem>
									) : (
										<PaginationItem key={item}>
											<PaginationLink
												isActive={item === page}
												onClick={(e) => {
													e.preventDefault();
													setPage(item);
												}}
											>
												{item}
											</PaginationLink>
										</PaginationItem>
									),
								)}
								<PaginationItem>
									<PaginationNext
										aria-disabled={page >= data.total_pages}
										onClick={(e) => {
											e.preventDefault();
											if (page < data.total_pages) setPage(page + 1);
										}}
									/>
								</PaginationItem>
							</PaginationContent>
						</Pagination>
					</footer>
				</div>
			</Layout>
			<CreateWorksheetDialog open={createDialogOpen} onOpenChange={setCreateDialogOpen} />
		</>
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
	const retryPipeline = useRetryPipeline(worksheet.id);
	const navigate = useNavigate();
	const [alertOpen, setAlertOpen] = useState(false);

	return (
		<>
			<DropdownMenu>
				<DropdownMenuTrigger
					render={<Button variant="ghost" size="icon-sm" onClick={(e: React.MouseEvent) => e.stopPropagation()} />}
				>
					<IconDotsVertical />
					<span className="sr-only">Actions</span>
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
					<DropdownMenuItem
						onClick={(e) => {
							e.stopPropagation();
							navigate({ to: "/worksheets/$id", params: { id: worksheet.id } });
						}}
					>
						<IconExternalLink />
						Open
					</DropdownMenuItem>
					{worksheet.pipeline_status === "failed" && (
						<DropdownMenuItem
							disabled={retryPipeline.isPending}
							onClick={(e) => {
								e.stopPropagation();
								retryPipeline.mutate();
							}}
						>
							{retryPipeline.isPending ? <IconLoader className="animate-spin" /> : <IconRotateClockwise />}
							Retry
						</DropdownMenuItem>
					)}
					{worksheet.pipeline_status === "running" && (
						<DropdownMenuItem onClick={(e) => e.stopPropagation()}>
							<IconPlayerStop />
							Stop
						</DropdownMenuItem>
					)}
					<DropdownMenuSeparator />
					<DropdownMenuItem
						variant="destructive"
						onClick={(e) => {
							e.stopPropagation();
							setAlertOpen(true);
						}}
					>
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
						<AlertDialogAction variant="destructive" onClick={() => deleteWorksheet.mutate(worksheet.id)}>
							Delete
						</AlertDialogAction>
					</AlertDialogFooter>
				</AlertDialogContent>
			</AlertDialog>
		</>
	);
}

function FilesCell({ worksheet }: { worksheet: Worksheet }) {
	return (
		<>
			{worksheet.file_count} {worksheet.file_count === 1 ? "file" : "files"}
			{worksheet.file_extensions.length > 0 && (
				<>
					{" · "}
					<span className="text-muted-foreground text-xs">{worksheet.file_extensions.join(", ")}</span>
				</>
			)}
		</>
	);
}

function QuizCounts({ worksheet }: { worksheet: Worksheet }) {
	const counts = worksheet.quiz_counts ?? worksheet.artifact_counts;
	if (!counts) return null;

	const parts = [
		counts.MultipleChoiceQuiz ? `${counts.MultipleChoiceQuiz} MCQ` : null,
		counts.EssayQuiz ? `${counts.EssayQuiz} essay` : null,
		counts.CompletionQuiz ? `${counts.CompletionQuiz} fill` : null,
	].filter(Boolean);

	if (parts.length === 0) return <span className="text-muted-foreground">—</span>;

	return <>{parts.join(" · ")}</>;
}
