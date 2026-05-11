# AUR Build Scripts

This directory contains Arch Linux AUR packaging scripts for Slate.

## Packages

- `slate-git/`: tracks `main` from GitHub and builds from source.
- `slate-bin/`: installs the prebuilt Linux binary from GitHub releases.

## Test Locally

```sh
cd packaging/aur/slate-git
makepkg -si
```

## Refresh `.SRCINFO`

```sh
./packaging/aur/update-srcinfo.sh
```

## CI Publishing

- GitHub Actions publishes the Linux binary to the rolling `aur-bin` release tag.
- CI updates `packaging/aur/slate-bin/PKGBUILD` + `.SRCINFO` and pushes to AUR.
- Configure the `GH_AUR_KEY` secret with an SSH private key registered in your AUR account.
