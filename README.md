# Worxheet

Worxheet is a desktop application built to transform primary study materials into Higher-Order Thinking Skills (HOTS) artifacts. By utilizing a Retrieval-Augmented Generation (RAG) pipeline, it ensures that all generated quizzes, summaries, and mind-maps are strictly grounded in the user's uploaded curriculum.

## 🚀 Architecture

The project is structured as a high-performance monolith:

- Frontend (`/app`): React 19 + TanStack Router. Handles the UI and artifact visualization.

- Backend (`/core`): Rust + Tauri. Manages SQLite persistence, file system access, and IPC commands.

- SQLite: Local relational storage for "Subjects" and metadata.

- Pinecone: Remote vector store for semantic similarity search.

## 🛠 Tech Stack

- Framework: Tauri 2.x (Rust + React)

- Frontend: React 19, TanStack Router, Tailwind CSS 4, shadcn/ui, Biome

- Language: Rust, TypeScript, SQL

- Build: Vite, pnpm

- AI/LLM: `LlamaParse` (Ingestion), `nomic-embed-text` (Embeddings), (TBD) (HOTS Generation)

- Database: SQLite (Embedded), Pinecone (Vector)

- Communication: Asynchronous IPC (Inter-Process Communication)

## ⚙️ How it Works (The RAG Pipeline)

- Ingestion: User creates a "Subject" and uploads files.

- Parsing: Rust core triggers LlamaParse to convert PDFs/Images to structured Markdown.

- Vectorization: Text chunks are embedded and stored in a Pinecone namespace unique to that Subject.

- Generation: The system retrieves relevant chunks and applies Few-Shot Prompting to generate MCQ items and summaries.

- Consumption: Artifacts are saved to SQLite and rendered in the React Workspace.

## 📄 License

This project is developed for academic research purposes focusing on the impact of AI in facilitating knowledge assimilation in higher education.
