# Repository Guidelines

## Project Structure & Module Organization

This workspace is an early project shell for a simpler Creality Print style application. `craft.json` records the request and intended stack (Rust + egui/eframe); `investigation.cany/` contains project metadata and an activity journal; `reference/` is reserved for user-provided screenshots and other references; `capture/README.md` defines the clean-room boundary. The Rust source, assets, and tests have not been added yet. When scaffolding begins, keep application code under `src/`, static assets under `assets/`, and automated tests beside modules or under `tests/`.

## Clean-Room and Reference Handling

Do not inspect, mount, extract, decompile, or otherwise read `/home/thawkins/bin/CrealityPrint-V7.3.0.6149-x86_64-Release.AppImage`, its containing folder, or its contents. Build from the user's written request and materials explicitly placed in `reference/`. Keep implementation independent of the closed application. If a requirement is unclear, ask the user before inferring behavior from the protected app.

## Build, Test, and Development Commands

No build manifest or runnable source exists yet, so there are currently no project build, run, or test commands. Once a Cargo project is added, document and use its actual commands; conventional starting points are `cargo run`, `cargo test`, and `cargo fmt --check`. Do not report these as passing until they have been run against the project.

## Coding Style & Naming

Use Rust formatting from `rustfmt` with standard four-space indentation. Follow Rust naming conventions: `snake_case` for modules, functions, and variables; `UpperCamelCase` for types and traits; and `SCREAMING_SNAKE_CASE` for constants. Prefer small modules organized around application features and keep UI state and rendering responsibilities clear.

## Testing Guidelines

Add focused unit tests for behavior and integration tests for user-visible workflows as features are implemented. Use descriptive Rust test names, such as `loads_saved_profile` or `rejects_invalid_settings`, and run them with `cargo test`. No coverage threshold or test framework configuration is established yet.

## Commits and Pull Requests

This workspace has no Git history or repository metadata, so no existing commit convention or PR process can be confirmed. Use concise imperative commit subjects (for example, `Add printer profile settings`) when version control is established. PRs should explain the user-facing change, list validation performed, link relevant issues, and include screenshots for visual UI changes.
