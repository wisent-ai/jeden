# Changelog

All user-visible Jeden changes are recorded here, newest first.

Every release published so far is a **prerelease**. `Cargo.toml` holds
`version = "0.1.0"` as a compatibility *floor*, and the pipeline publishes
`floor + 1` patch with a canary suffix
(`<major>.<minor>.<patch+1>-canary.<run>.<attempt>.sha<12>`), so no stable
version has ever been released. There is no `v*` tag in the repository. The
`stable` channel exists in the pipeline (`promote.yml`) and in the binary's
built-in trust roots (key id `jeden-stable-2026-07-13`), but has never carried
an artifact.

None of the published releases shipped release notes: the canary releases carry
the single line `Locally verified and signed canary release.` and the
source-only preview carries a two-sentence disclaimer. Every section below was
therefore reconstructed after the fact from the Git diff between the released
commits and from the published release assets; each section says so. The
`/changelog` slash command in the binary generates its output from Git history
for the same reason, with the in-code note "no bundled CHANGELOG file exists" —
this file replaces that gap for published releases only.

Distribution is signed per-platform binaries attached to GitHub Releases,
deliberately not crates.io (the name `jeden` there belongs to an unrelated
stub). Published platform coordinates are `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`; a canary release is
three separate GitHub releases, one per coordinate, sharing one version.

## Unreleased

Derived from `git diff platform-surface-2026-07-31.1 HEAD` (11 commits, 24
files, +1333/-402 excluding lockfiles).

### Added

- Added product onboarding (`rust/onboarding.rs`,
  `rust/onboarding_first_use.json`).
- Added a Probierz runner (`rust/probierz.rs`) and scoped runtime credentials
  (`rust/cli/token.rs`).
- Added the launcher `scripts/run-with-stado.sh`, which resolves its Skarbiec
  consumer and token file.
- Added `jeden pursue` as an integration with the separately owned, revision-pinned [Agent Contract](https://github.com/wisent-ai/agent-contract) protocol. Jeden supplies Brama-backed conversations and approval-gated tools; Agent Contract owns preference evidence, contract and verdict schemas, independent review and repair loops, validators, and receipts under `.agent-contract/runs/`.

### Changed

- Model and media access now route through Echo
  (`rust/tool_services/media.rs`, `rust/control_plane/brama.rs`).

### Fixed

- Fixed signing of empty Brama requests.
- Fixed an unauthorized Brama secret lookup.

### Configuration changes

- `BRAMA_URL` is no longer prefilled and is now marked required.
- New: `BRAMA_TOKEN` (documented as never to be reused from
  `STADO_API_TOKEN`), `STADO_MEDIA_ROUTER_URL`, `JEDEN_MEDIA_ROUTER_TOKEN`,
  `STADO_MEDIA_ROUTER_ALLOW_INSECURE_LOOPBACK`, `JEDEN_IMAGE_MODEL`,
  `JEDEN_TTS_MODEL`.

### Known limitations

- This revision cannot be built from a checkout of this repository alone:
  `Cargo.toml` declares the path dependency
  `wisent-onboarding-client = { path = "../echo-web/crates/onboarding-client" }`,
  which lives outside the repository.
- No release has been published from this revision.

## platform-surface-2026-07-31.1 - 2026-07-31

Reconstructed from `git show 21eed358` and from the published release, which
carries **no assets** and this body: "Immutable source-only preview aligned
with Monetizer catalogue 2026-07-31.1. This is not a supported Jeden release and
contains no qualified binary, hosted entitlement, approved price, checkout, or
SLA."

This is a source-only preview, not a build channel. The same tag identifier
appears in `brama` and `stado`, so it marks a cross-repository source surface
rather than a Jeden product release. It ships no binary; nothing can be
installed or upgraded from it.

### Added

- Added `platform-entitlements.json` (schema
  `wisent.product-entitlements.current`): product `jeden`, community
  entitlement `jeden.local`, managed entitlements `jeden.managed` and
  `jeden.team`, failure policy `managed: fail-closed` and
  `community: local harness remains available`, plus the legacy environment map
  `WELES_URL` → `WISENT_PLATFORM_BILLING_URL`, `WELES_TOKEN` →
  `WISENT_PLATFORM_BILLING_TOKEN`.
- Jeden was relicensed under Apache 2.0 (`LICENSE`) within the range this tag
  covers.
- Added the versioning gate `.github/workflows/version-check.yml` with
  `scripts/surface.py` and `scripts/baseline.py`, and `released-surface.json`
  as the recorded published command surface.
- Added interface localization across 65 languages (`rust/cli/i18n.rs`,
  `rust/cli/i18n_translations.rs`), the `/setup` wizard
  (`rust/slash/setup.rs`), the commands `completions`, `gallery`, `stats`,
  `token` and `worktree`, self-rebuild (`rust/cli/run/self_rebuild.rs`), the
  sandbox helper binary `jeden-sandbox-helper`, and the roadmap surface
  (`roadmap/roadmap.yaml` with its schema and plan view).

### Changed

- Platform billing configuration was made vendor-neutral.
  `WelesClient::from_env()` now reads `WISENT_PLATFORM_BILLING_URL` and
  `WISENT_PLATFORM_BILLING_TOKEN` first and falls back to `WELES_URL` and
  `WELES_TOKEN`, so existing configurations keep working.
- Two observable strings changed: the health service name is now
  `platform-billing` instead of `weles`, and its unconfigured message is
  "WISENT_PLATFORM_BILLING_URL is not configured" instead of the `WELES_URL`
  wording. Subscription routing reports "cannot list platform billing accounts
  for subscription routing" instead of naming Weles
  (`rust/agent/runtime/routing.rs`).

### Removed

- Removed the migration-compatibility test infrastructure:
  `tests/fixtures/migrations/{config,ledger,collab,memory,plugin-lock,quality,task-store}/{n-2,n-1,n,future}`,
  `tests/migrations/run_migration_matrix.py`,
  `tests/migrations/test_migration_matrix.py` and
  `tests/migrations/fixtures/matrix-v1.json`. The `rust/migrations` module
  remains in the binary, but the N-2 / N-1 / N / future compatibility evidence
  no longer exists in the tree.
- Removed the workflows `nightly-e2e.yml`, `soak.yml` and `staging-e2e.yml`,
  and much of the `tests/` directory including `tests/headless.rs` and
  `tests/test_release_workflow_contract.py`.

### Configuration changes

- `.env.example` gains `WISENT_ORGANIZATION_ID`,
  `WISENT_PLATFORM_BILLING_URL` and `WISENT_PLATFORM_BILLING_TOKEN`, all
  documented as optional — local Jeden works without them.

### Data and state migrations

- None.

### Compatibility requirements

- The legacy `WELES_URL` / `WELES_TOKEN` names still resolve, so the
  environment migration is optional, not forced.
- With no billing configuration, the local harness keeps working; managed mode
  is fail-closed.

### Operator actions required

- Optionally rename `WELES_URL` / `WELES_TOKEN` to
  `WISENT_PLATFORM_BILLING_URL` / `WISENT_PLATFORM_BILLING_TOKEN`.
- Do not treat this tag as an installable release; it has no binary.

### Known limitations

- The tag is a source-only preview with no qualified binary, hosted
  entitlement, approved price, checkout or SLA, as its own release body states.

## 0.1.1-canary.8.1.shac034047b9341 - 2026-07-13

Reconstructed from `git diff fb1d2d6ec5e6 c034047b9341`, which touches exactly
two files — `rust/update/mod.rs` and `rust/update/tests.rs` — because the
release was published with only the body "Locally verified and signed canary
release."

Published as three prereleases, one per platform coordinate. Each carries
`jeden-0.1.1-canary.8.1.shac034047b9341-<target>.tar.gz`,
`manifest.dsse.json`, `channel.json`, `provenance.intoto.json`,
`sbom.spdx.json` and `release-gate-digests.json`.

### Added

- The updater can now fetch release assets from a **private** GitHub release.
  It reads a token from `JEDEN_UPDATE_GITHUB_TOKEN`, or failing that `GH_TOKEN`
  (empty or whitespace values are ignored), resolves the asset through
  `GET https://api.github.com/repos/wisent-ai/jeden/releases/tags/{tag}` and
  downloads `assets[].url` with `Accept: application/octet-stream` and
  `User-Agent: jeden-updater`.
- Added release-archive extraction (`extract_release_executable`). The
  `tar.gz` must contain exactly one root entry named `jeden`, or `jeden.exe`
  for a `-windows-msvc` target.

### Changed

- The updater installs the **extracted** executable instead of the raw
  downloaded bytes (`transaction::install(&paths, &executable, …)`).
- The post-swap health probe changed contract: it now runs
  `jeden capabilities --cwd <dir>` instead of `jeden doctor --json --cwd <dir>`,
  so the check no longer requires service configuration to pass.

### Security-relevant changes

- The GitHub token is sent only to `https://github.com` and
  `https://api.github.com`, and only on `/wisent-ai/jeden…` and
  `/repos/wisent-ai/jeden…` paths. Any other host or scheme receives no token.
- Download URLs are shape-validated: exactly six segments
  `wisent-ai/jeden/releases/download/<tag>/<asset>`, each restricted to
  `[A-Za-z0-9._-]`; an asset endpoint must be
  `api.github.com/repos/wisent-ai/jeden/releases/assets/<digits>`.
- Release metadata is capped at 1 MiB, checked both against `content_length`
  and against the bytes actually read, so an oversized declared payload is
  rejected without allocating it.
- Archive extraction hard-refuses a non-file entry, a nested or traversing
  member path, more than one entry, a zero-length or over-256-MiB payload, and
  a size that disagrees with the tar header.
- The artifact digest stays bound to the compressed archive, not to the
  extracted file, so extraction cannot launder a mismatched download.
- Error paths never format the token into a message.

### Configuration changes

- New, optional: `JEDEN_UPDATE_GITHUB_TOKEN`, with `GH_TOKEN` as fallback.
  Required only to update from a private release.

### Data and state migrations

- None.

### Compatibility requirements

- Update artifacts must now be `tar.gz` archives with a single root executable.
  A bare binary published at a release asset URL is no longer accepted.

### Operator actions required

- Set `JEDEN_UPDATE_GITHUB_TOKEN` (or `GH_TOKEN`) alongside the required
  `JEDEN_UPDATE_MANIFEST` before running an update against a private release.
- Set `JEDEN_UPDATE_CHANNEL=canary` explicitly. The default is `stable`, and
  no stable artifact has ever been published, so the default channel resolves
  to nothing.

### Known limitations

- `stable` remains the default update channel while being empty.
- The updater recognizes six target triples
  (`aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu`,
  `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`) but only three are
  built and published, so the other three resolve to no artifact.
- The upgrade and rollback mechanism exists in code — a phase journal, an
  exclusive `UpdateLock`, `read_installed_state`, `recover`,
  `recover_exclusive` and `install` in `rust/update/transaction.rs` — but there
  is no published document describing it. No `RELEASE.md`, `UPGRADE.md` or
  `docs/release.md` exists at this tag.

## 0.1.1-canary.7.1.shafb1d2d6ec5e6 - 2026-07-13

Reconstructed from the repository state at `fb1d2d6ec5e6` and from the
published release assets. This is the earliest published release, so there is
no previous release to diff against; the surface below is described as a whole
and no "Changed" or "Fixed" section can be derived from Git for it. The release
body is only "Locally verified and signed canary release."

Published as three prereleases, one per platform coordinate, each carrying
`jeden-0.1.1-canary.7.1.shafb1d2d6ec5e6-<target>.tar.gz`,
`manifest.dsse.json`, `channel.json`, `provenance.intoto.json`,
`sbom.spdx.json` and `release-gate-digests.json`.

### Added — released product surface

- A coding agent shipping three binaries: `jeden`, `jeden-quality-report` and
  `jeden-reference-benchmark`. Two modes: an interactive terminal and a
  one-shot `jeden run`. Sessions and artifacts live in `~/.jeden/sessions/`.
- CLI surface documented at this commit: `jeden`, `run`, `sessions`, `show`,
  `export`, `artifacts`, `artifact`, `resume`, `search-sessions`,
  `recall_conversation`, `tools`, `config`, `doctor`, `capabilities`.
- Jailed tool set: filesystem, documents, archives, images, SQLite, search,
  Git, processes, evaluation, URL, artifacts, memory, todo, delegation and MCP.
  File mutations are guarded by a digest and snapshot taken by `read_file`, and
  writes and commands are approved interactively.
- User-supplied JavaScript tools from `~/.jeden/tools/` and
  `<cwd>/.jeden/tools/`, plus lifecycle hooks.
- Self-update with Ed25519 trust roots compiled into the binary: channel
  `canary` (key id `jeden-canary-2026-07-13`) and channel `stable` (key id
  `jeden-stable-2026-07-13`). Downloads are capped at 256 MiB and the probe
  timeout is 10 s.

### Changed — release evidence

- Release evidence bytes are canonical. `dist/sbom.spdx.json`,
  `dist/provenance.intoto.json` and `dist/build-handoff.json` are written with
  `write_bytes(json.encode('utf-8') + b'\n')` instead of `write_text`, so their
  bytes — and therefore the SHA-256 recorded in the build handoff — no longer
  depend on the build platform's line endings or default encoding.

### Configuration changes

Required at this release (`.env.example`, `rust/cli/run/slash.rs`):

- Model routing: `BRAMA_URL`, `WISENT_APP_AGENT_ID` (`wisent-app`),
  `WISENT_APP_AGENT_AUTH_SECRET`. Model selection via `--model`, `JEDEN_MODEL`
  (`claude-code-subscription`) or configuration file. `ENTITLEMENTS_ROUTER_BIN`
  is optional.
- Update: `JEDEN_UPDATE_MANIFEST` is **required** (HTTPS or a local DSSE
  manifest). `JEDEN_UPDATE_CHANNEL` defaults to `stable`.
  `JEDEN_UPDATE_TARGET_TRIPLE` defaults to the native triple;
  `JEDEN_UPDATE_TARGET` is the destination path.

### Data and state migrations

- None.

### Compatibility requirements

- `manifest.dsse.json` declares `minimumVersion 0.1.0`.
- Published platform coordinates are `aarch64-apple-darwin`,
  `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc` only.

### Operator actions required

- Set `JEDEN_UPDATE_MANIFEST` before invoking `update`; it has no default.
- Set `JEDEN_UPDATE_CHANNEL=canary`, since the default `stable` channel has no
  published artifact.

### Known limitations

- This is a prerelease of a private milestone; no stable release exists.
- No release notes, changelog, or documented upgrade/rollback contract shipped
  with it.
