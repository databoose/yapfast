# Yapfast

- Run every cargo command in the dev shell, with the `fast` profile:
  `nix develop path:./nix --command cargo build --profile fast`.
- While working, build and test only what changed, such as
  `cargo test --profile fast -p yapfast-core history`; incremental builds take
  seconds. Run `scripts/check --fast` once, before the last commit of a piece
  of work, not before each commit. It enters the dev shell itself and covers
  Discord-only, Matrix-only and headless builds; it takes about 20 s after
  an edit, and prints each step's time.
- In a new worktree, run `scripts/warm-worktree` before building. It copies
  the main checkout's build, so the first check compiles only the crates in
  the repository, about 3.5 minutes.
- An edit under `vendor/i-slint-*` recompiles Slint and the UI crate, 2–3
  minutes each time; settle such a patch in as few builds as possible.
- See the app: `scripts/ui-session` (private display, screenshots, input; run
  it with `--help`). Discord runs only against `testing/fake_discord.ts`.
- Discord's API, as its own apps use it: https://docs.discord.food. Look up
  an endpoint, payload or gateway event there before adding or changing one,
  and give the fake the same shape.
- Discord service tests: `scripts/test-fake-discord`. Matrix and LiveKit:
  `scripts/test-stack up`.
- How a UI change flows from `ui/*.slint` to `application/`:
  `docs/architecture.md`. Shells, logging (`YAPFAST_LOG`, not `RUST_LOG`) and
  dependency sources: `docs/building.md`.
- Memory is a product goal. Keep every change lean without being asked, and
  check the code it runs through as well: share data held in two places
  (`Arc`, a borrow), make work that runs on every event or frame touch only
  what changed, and stream data to a file or socket as it's made. Fix a
  needless copy you find on that path, or report it. Targets, and how to
  measure: `docs/measurements/README.md`. A commit that saves memory gives
  the numbers it measured.
- Start new branches from `origin/main` after `git fetch`.
