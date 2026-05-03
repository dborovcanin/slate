# AUR Build Scripts

This directory contains Arch Linux AUR packaging scripts for Slate.

## Packages

- `slate-git/`: tracks `main` from GitHub and builds from source.

## Test Locally

```sh
cd packaging/aur/slate-git
makepkg -si
```

## Refresh `.SRCINFO`

```sh
./packaging/aur/update-srcinfo.sh
```
