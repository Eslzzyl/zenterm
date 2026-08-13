# zenterm

A modern terminal emulator in pure Rust. WIP

## Version management

Use the version bump script to update all workspace crates, regenerate
`Cargo.lock`, create a commit, and create the matching `v<version>` tag:

```bash
./scripts/bump-version.sh 0.2.0
git push origin master --follow-tags
```

On Windows, use `scripts/bump-version.ps1` instead. Pushing a `v*` tag starts
the multi-platform release workflow, which publishes Linux, macOS, Windows
x86_64, and Windows ARM64 installers to GitHub Releases.

To package locally for a specific target:

```bash
./scripts/package.sh --target aarch64-apple-darwin --format app
```

# LICENSE

Apache 2.0
