# Worxheet

Worxheet is a desktop application built to transform primary study materials into Higher-Order Thinking Skills (HOTS) artifacts. By utilizing a Retrieval-Augmented Generation (RAG) pipeline, it ensures that all generated quizzes, summaries, and mind-maps are strictly grounded in the user's uploaded curriculum.

## 🚀 Architecture

The project is structured as a high-performance monolith:

- Frontend (`/app`): React 19 + TanStack Router. Handles the UI and artifact visualization.

- Backend (`/core`): Rust + Tauri. Manages SQLite persistence, file system access, and IPC commands.

- SQLite: Local relational storage for worksheets, file metadata, text chunks, vector embeddings, and generated artifacts.

## 🛠 Tech Stack

- Framework: Tauri 2.x (Rust + React)

- Frontend: React 19, TanStack Router, Tailwind CSS 4, shadcn/ui, Biome

- Language: Rust, TypeScript, SQL

- Build: Vite, pnpm

- AI/LLM: `pdf_oxide` + `office_oxide` (document extraction), `bge-small-en-v1.5` (Embeddings, GGUF, ~27MB), `SmolLM2-360M-Instruct` (HOTS Generation, GGUF, ~365MB)

- AI Runtime: `llama-cpp-2` (local inference, grammar-constrained sampling)

- Database: SQLite (Embedded, including vector storage as BLOBs)

- Communication: Asynchronous IPC (Inter-Process Communication)

## ⚙️ How it Works (The RAG Pipeline)

- Ingestion: User creates a "Worksheet" and uploads files.

- Parsing: Rust core uses `pdf_oxide` (PDF) and `office_oxide` (PPTX/DOCX/PPT/DOC) to extract text locally (no cloud API).

- Vectorization: Text chunks are embedded via `bge-small-en-v1.5` (GGUF) running in `llama-cpp-2` and stored as BLOBs in SQLite.

- Clustering: Embedded chunks are grouped into topic clusters (HDBSCAN) so generation exhausts the whole material.

- Generation: Each topic cluster is turned into grammar-constrained JSON artifacts (MCQs, essays, fill-in-the-blank, summaries, mind-maps) by `SmolLM2-360M-Instruct` (GGUF) — all local, no external API calls.

- Consumption: Artifacts are saved to SQLite and rendered in the React workspace.

## ⚙️ Automatic Pipeline

Creating a worksheet with files automatically starts a background job
(`core/src/pipeline/jobs.rs`) that runs ingest → embed → cluster → generate
for all five artifact types. The worksheet detail page shows live progress
(polling `get_pipeline_status`) and then the generated artifacts; there are no
process or generate buttons.

## 📄 License

This project is developed for academic research purposes focusing on the impact of AI in facilitating knowledge assimilation in higher education.
