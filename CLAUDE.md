# Talk

- Run every cargo command in the dev shell, with the `fast` profile:
  `nix develop path:./nix --command cargo build --profile fast`.
- While working, build and test only what changed, such as
  `cargo test --profile fast -p talk-core history`; incremental builds take
  seconds. Run `scripts/check --fast` once, before the last commit of a piece
  of work, not before each commit. It enters the dev shell itself and covers
  Discord-only, Matrix-only and headless builds; it takes under a minute
  after an edit.
- In a new worktree, run `scripts/warm-worktree` before building. It copies
  the main checkout's build, so the first check compiles only the crates in
  the repository, about 3.5 minutes.
- An edit under `vendor/i-slint-*` recompiles Slint and the UI crate, 2–3
  minutes each time; settle such a patch in as few builds as possible.
- See the app: `scripts/ui-session` (private display, screenshots, input; run
  it with `--help`). Discord runs only against `testing/fake_discord.ts`.
- Discord service tests: `scripts/test-fake-discord`. Matrix and LiveKit:
  `scripts/test-stack up`.
- How a UI change flows from `ui/*.slint` to `application/`:
  `docs/architecture.md`. Shells, logging (`TALK_LOG`, not `RUST_LOG`) and
  dependency sources: `docs/building.md`. Memory measurements and how to
  reproduce them: `docs/measurements/README.md`.
- Start new branches from `origin/main` after `git fetch`.
