## Architecture

- Monolith: `app/` (React 19 frontend) + `core/` (Rust/Tauri backend)
- All AI inference runs locally via `mistralrs` + `candle`; zero cloud dependencies
- SQLite with WAL mode; migrations in `core/migrations/`
- Embeddings stored as 384×f32 BLOBs in SQLite; brute-force cosine similarity (no vector DB)

## Commands

| Action              | Command                        |
| ------------------- | ------------------------------ |
| Dev (frontend only) | `pnpm app:dev` (Vite on :1420) |
| Dev (Tauri)         | `pnpm dev`                     |
| Build               | `pnpm build`                   |
| Format              | `pnpm format` (Biome)          |
| Rust tests          | `cd core && cargo test`        |

## Conventions

- **Naming:** No abbreviations in variable names. For example, use `worksheet_id` not `ws_id`, `extension` not `ext`, `database_path` not `db_path`, `token_count` not `num_tokens`, `token_ids` not `ids`, `previous` not `prev`, `file` not `f`.
- Biome: tabs, double quotes, lints `app/**` (excludes `route-tree.gen.ts`)
- Tailwind v4 via `@tailwindcss/vite`; shadcn/ui v4 `base-vega` style
- Path alias `@/` → `app/`
- TanStack Router v1: file-based routes in `app/routes/`, generated tree at `app/route-tree.gen.ts`
- Zod v4 + TanStack React Form v1.33.0

## Rust specifics

- k-means clustering + position-based even sampling (not centroid picking)

## Testing

- Async tests use `#[tokio::test]`; `tokio` dev dep with `rt` + `macros` features
- DB tests use in-memory SQLite: `SqlitePool::connect("sqlite::memory:")`
- Fixtures in `core/tests/fixtures/` (test.pdf, test.docx)
