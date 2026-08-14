# Releasing Quilon

How a Quilon compiler release is cut. The release is automated: pushing a
version **tag** triggers the `Release` workflow, which builds the binary and
publishes a GitHub Release. Releasing is therefore "land the version bump, then
push the tag."

The VS Code extension is released separately (its own `vscode-extension.yml`
workflow and its own version); nothing here touches it.

## What the release workflow does

`.github/workflows/release.yml`:

- **Trigger:** a pushed tag matching `v*` (e.g. `v0.9.1`). Tags are the only
  trigger — pushing to a branch does not release.
- **Runner / toolchain:** `ubuntu-latest`; installs Rust stable, LLVM 22
  (`llvm-22-dev`, `libpolly-22-dev`), `libgc-dev`, `gcc`, and `clang` — the same
  toolchain as CI.
- **Build:** `cargo build --release --bin quilon`.
- **Publishes:** a **GitHub Release** for the tag (via
  `softprops/action-gh-release@v3`) with:
  - the compiled `target/release/quilon` binary attached as a release asset — a
    **dynamically-linked Linux x86-64** build (requires `libgc` at runtime; a
    single target, no cross-compilation / macOS / Windows / static build yet);
  - release notes = an auto-generated PR/commit list (`generate_release_notes:
    true`) appended to the hand-written highlights/limitations body in the
    workflow.
- **Secrets / prerequisites:** none beyond the default `GITHUB_TOKEN` (the job
  declares `permissions: contents: write`). It does **not** publish to crates.io,
  npm, Homebrew, or anywhere else — GitHub Releases only.

## Cutting a release (e.g. 0.9.1)

1. **Land the release-prep PR.** Ensure the "Prepare 0.9.1 release" PR is
   reviewed and merged into `main`. It bumps `Cargo.toml` and
   `quilon-rt/Cargo.toml` to the new version (with `Cargo.lock` updated) and
   finalizes the dated `CHANGELOG.md` section.

2. **Sync `main` locally.**
   ```bash
   git checkout main
   git pull origin main
   ```

3. **Sanity-check the version.** Confirm the crate version and changelog match
   the release you are about to tag:
   ```bash
   grep '^version' Cargo.toml quilon-rt/Cargo.toml   # both -> 0.9.1
   head -n 20 CHANGELOG.md                            # top section is the dated 0.9.1
   ```

4. **Tag and push.** The tag name drives the release; use an annotated tag on the
   merge commit:
   ```bash
   git tag -a v0.9.1 -m "Quilon 0.9.1"
   git push origin v0.9.1
   ```

5. **CI runs.** The `Release` workflow builds the release binary and creates the
   GitHub Release with the `quilon` asset and generated notes. Watch the run:
   ```bash
   gh run watch
   ```

6. **Verify.** Confirm the Actions run is green, then check the published
   release:
   ```bash
   gh release view v0.9.1
   ```
   The release page should show tag `v0.9.1`, the attached `quilon` binary, and
   the notes. Optionally download the asset and smoke-test it on a machine with
   `libgc` installed:
   ```bash
   gh release download v0.9.1 -p quilon
   chmod +x quilon
   ./quilon --version    # or: ./quilon run examples/hello_world.ql
   ```

## If something goes wrong

- **Wrong tag / bad build:** delete the tag locally and remotely, delete the
  draft/published release, fix the problem on `main`, and re-tag.
  ```bash
  git push origin :refs/tags/v0.9.1   # delete remote tag
  git tag -d v0.9.1                   # delete local tag
  gh release delete v0.9.1            # delete the release if it was created
  ```
- **Re-tagging the same version** is discouraged once a release is public; prefer
  a patch bump (`v0.9.2`) if the artifact was already downloaded.
