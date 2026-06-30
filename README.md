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

- AI/LLM: `pdf_oxide` (PDF extraction), `bge-small-en-v1.5` (Embeddings, GGUF), `SmolLM2-360M-Instruct` (HOTS Generation, GGUF, ~271MB)

- AI Runtime: `mistralrs` + `candle` (pure Rust, local inference)

- Database: SQLite (Embedded, including vector storage as BLOBs)

- Communication: Asynchronous IPC (Inter-Process Communication)

## ⚙️ How it Works (The RAG Pipeline)

- Ingestion: User creates a "Worksheet" and uploads files.

- Parsing: Rust core uses `pdf_oxide` to extract structured Markdown from PDFs locally (no cloud API).

- Vectorization: Text chunks are embedded via `bge-small-en-v1.5` (GGUF) running in `mistralrs` and stored as BLOBs in SQLite.

- Generation: Cosine similarity over SQLite retrieves relevant chunks; `SmolLM2-360M-Instruct` (GGUF) generates MCQ items and summaries via few-shot prompting — all local, no external API calls.

- Consumption: Artifacts are saved to SQLite and rendered in the React Workspace.

## 📄 License

This project is developed for academic research purposes focusing on the impact of AI in facilitating knowledge assimilation in higher education.
