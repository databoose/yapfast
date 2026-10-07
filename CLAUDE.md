# Talk

- Run every cargo command in the dev shell, with the `fast` profile:
  `nix develop path:./nix --command cargo build --profile fast`.
- Check before committing: `scripts/check --fast`. It enters the dev shell
  itself and covers Discord-only, Matrix-only and headless builds.
- See the app: `scripts/ui-session` (private display, screenshots, input; run
  it with `--help`). Discord runs only against `testing/fake_discord.ts`.
- Discord service tests: `scripts/test-fake-discord`. Matrix and LiveKit:
  `scripts/test-stack up`.
- How a UI change flows from `ui/*.slint` to `application/`:
  `docs/architecture.md`. Shells, logging (`TALK_LOG`, not `RUST_LOG`) and
  dependency sources: `docs/building.md`. Memory measurements and how to
  reproduce them: `docs/measurements/README.md`.
- Start new branches from `origin/main` after `git fetch`.
