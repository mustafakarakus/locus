# Releasing Locus

Only a human owner may approve a use case or mark it `Done`. Release commands
must be run without `sudo` so files under `~/.locus` remain user-owned.

## 1. Prepare and validate

1. Confirm the release version is consistent in the workspace manifest,
   changelog, and Homebrew formula.
2. Confirm the working tree contains no generated project configuration,
   database files, or `*.locus-backup*` files intended only for local use.
3. Run the required quality gates:

   ```sh
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test --all
   cargo build --release --all-features
   cargo package -p locus-memory-core --allow-dirty
   cargo package --workspace --allow-dirty --no-verify
   sh -n scripts/install.sh scripts/uninstall.sh
   git diff --check
   ```

4. Test installation and removal from a temporary directory. Run
   `locus --version` and `locus doctor` from the installed binaries.
5. Set U-014 to `Ready for Review`, then stop for human review.

## 2. Tag and create the release

After approval, merge the release-preparation branch and create an annotated
tag from a clean `main` branch:

```sh
git tag -a v0.1.0 -m "Locus v0.1.0"
git push origin v0.1.0
```

Create the GitHub release from that tag and use the corresponding changelog
section as its release notes. GitHub tag archives are the source artifacts used
by the Homebrew formula.

## 3. Finalize Homebrew

Download the exact archive referenced by `Formula/locus.rb`, calculate its
checksum, and replace the formula's all-zero placeholder:

```sh
curl -L -o locus-v0.1.0.tar.gz \
  https://github.com/mustafakarakus/locus/archive/refs/tags/v0.1.0.tar.gz
shasum -a 256 locus-v0.1.0.tar.gz
```

Commit the checksum update to the `mustafakarakus/homebrew-tap` repository,
whose local tap name is `mustafakarakus/tap`. Current Homebrew versions require
formulae to be audited by tap-qualified name rather than file path:

```sh
brew tap mustafakarakus/tap
brew audit --strict mustafakarakus/tap/locus
brew install --build-from-source mustafakarakus/tap/locus
brew test mustafakarakus/tap/locus
```

For initial local validation before the GitHub tap exists, create it with
`brew tap-new mustafakarakus/tap`, copy the formula into the resulting
`Formula/` directory, and run the same tap-qualified commands. Never invent the
checksum before the immutable tag archive exists.

## 4. Publish Cargo crates

Authenticate locally without committing credentials. Publish in dependency
order, allowing crates.io to index each dependency before publishing its
dependants:

1. `locus-memory-core`
2. `locus-mcp` and `locus-testkit`
3. `locusd`, `locus-viz`, and `locus-memory-cli`

Use `cargo publish --dry-run -p <package>` before each real publish. Publishing
is irreversible; verify package contents and versions before continuing.

The package names `locus-core` and `locus-cli` belong to unrelated projects on
crates.io. Locus therefore publishes those two packages as `locus-memory-core`
and `locus-memory-cli`; their Rust crate and executable names remain
`locus_core` and `locus`.

## 5. Verify

Install from each published channel in a clean temporary environment. Verify
all four binaries (`locus`, `locusd`, `locus-mcp`, and `locus-viz`), run
`locus doctor`, and confirm the release page links to the changelog.