# AUR Build Scripts

This directory contains Arch Linux AUR packaging scripts for Slate.

## Packages

- `slate-git/`: tracks `main` from GitHub and builds from source.
- `slate-bin/`: installs the prebuilt Linux binary from a tagged GitHub release.

## Test Locally

```sh
cd packaging/aur/slate-git
makepkg -si
```

## Refresh `.SRCINFO`

```sh
./packaging/aur/update-srcinfo.sh
```

## Releasing

Bump `version` in `crates/tui/Cargo.toml`, merge to `main`, then run:

```sh
scripts/release.sh
```

It tags `v<version>` and pushes the tag. The `Release` workflow then:

- builds `slate-linux-x86_64` and `slate-macos-aarch64` with `.sha256` files,
- creates the GitHub release for the tag,
- runs `scripts/aur-publish.sh <version>`, which rewrites `slate-bin/PKGBUILD`
  and `.SRCINFO` with real checksums and pushes them to the AUR.

The AUR push needs the `AUR_SSH_PRIVATE_KEY` secret: an SSH private key
registered in the AUR account. Without it the AUR step is skipped. AUR host
keys are pinned in `known_hosts`.

Commit the rewritten `slate-bin/PKGBUILD` and `.SRCINFO` after a release so
the repository matches what the AUR has.
