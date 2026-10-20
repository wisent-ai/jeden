# Release automation

The exact release version is the SemVer in `Cargo.toml`. Stado reads it through `.wisent-release.json` and supplies `WISENT_OUTPUT_DIR` to `stado product cargo stage`; no run number or provider identity participates in the release version.

The recipe stages `bin/jeden` and the Darwin sandbox helper where applicable. Stado signs the declared native stage before producing the archive and its signed source/build/publication receipts. Darwin release jobs obtain the certificate and private key through the manifest's exact Skarbiec field references and use the signer's temporary keychain; they do not request a system consent dialog.

Release builders receive locked private Git crates as the immutable
`private-cargo-sources` input, without GitHub credentials or sibling checkouts.
After changing those package names, versions or revisions, run
`stado release catalog pin-input . --name private-cargo-sources --source . --revision HEAD --cargo --json`
from the committed checkout, then commit the updated `.wisent-release.json`.
Stado vendors, publishes and pins the input through one shared operation;
changing only Jeden's version does not invalidate it. See
[private Cargo build inputs](https://stado.wisent.com/docs/builds#private-cargo-build-inputs)
for commands, desktop controls, refusals and the real publication journey.

Release quality uses `stado product cargo build`, and the build step is
`stado product cargo stage`, the same executor: it builds each `--bin` with the
locked sources and places it at `WISENT_OUTPUT_DIR/<name>`.
Missing output or private input, stale private packages and inconsistent source
configuration are refusals, not reasons to fetch private repositories on the
worker. Cargo verifies the vendored checksums and preserves the source lockfile.
Jeden has no separate private-source exporter or source-replacement wrapper.

The recipe runs `JEDEN_TEST_SUCCESS_EXIT=0 node tests/release/stage.mjs` with
the worker's `WISENT_SOURCE_DIR`, `WISENT_SOURCE_COMMIT`, `WISENT_OUTPUT_DIR`
and `WISENT_INPUT_PRIVATE_CARGO_SOURCES_DIR`. This builds and executes the
staged CLI, checks the helper's staged bytes, and verifies that missing output
or input does not replace successful artifacts or change the lockfile.
Reports and command logs are archived under `evidence/release-tests`.
This check does not qualify signing, sandbox execution or a model-backed task.

The version gate (`.github/workflows/version-check.yml`) compares the public command vocabulary with `released-surface.json` using the same tool: `jeden-tools surface` reads the dispatcher and the compiled slash catalogue, `jeden-tools versioning decide` is the Rust port of the fleet's AutoVersion rule, and `jeden-tools versioning baseline` regenerates the baseline from the newest published release.

Jeden is a command-line package, not a fleet service. Publication therefore does not start a daemon or claim a host has installed the package. A consumer must install `bin/jeden` and `bin/jeden-sandbox-helper` from the same Darwin archive and keep them together; installing only the primary executable leaves sandboxed runs unavailable.

`darwin-arm64` and `linux-amd64` are the declared promoted outputs. A compilation gate is not evidence that the model-backed completion journey passed; the real contract run and its result remain separate recorded evidence.

The documentation site at [jeden.wisent.com/docs](https://jeden.wisent.com/docs) is published from source, and no rendered page is committed. Vercel deploys `web/` on every push to `main`: its build installs the repository's locked Node packages (`npm ci --prefix ..`) and runs `node docs/pages.mjs`, which renders `web/docs/**/*.html`, `web/sitemap.xml` and `web/wisent-components.css` from the page sources in `web/docs/` (`pages.mjs`, `cli-pages.mjs`, `content/`, `commands/`). A documentation change is therefore published by pushing its source. `npm run build:docs` renders the same files for a local preview; git ignores them. `node tests/docs/published-pages.probierz.spec.mjs` fetches every production page, the sitemap and the stylesheet, and fails with the page and the first differing text unless production serves exactly what the sources of the checked-out revision render. Run it once that revision is deployed: a failed Vercel build leaves the previous deployment in production, and the check reports that as a difference.
