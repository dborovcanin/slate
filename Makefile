APP_NAME := slate
BIN_DIR := $(HOME)/.local/bin
DESKTOP_DIR := $(HOME)/.local/share/applications
PIXMAP_DIR := $(HOME)/.local/share/pixmaps
BUILD_DIR := build

LINUX_TARGET := x86_64-unknown-linux-gnu
MAC_INTEL_TARGET := x86_64-apple-darwin
MAC_ARM_TARGET := aarch64-apple-darwin

HOST_OS := $(shell uname -s)
HAS_OSXCROSS := $(shell command -v o64-clang >/dev/null 2>&1 && command -v oa64-clang >/dev/null 2>&1 && echo 1 || echo 0)

RELEASE_BIN := target/release/$(APP_NAME)

.PHONY: all build test perf install release release-linux release-macos

all: install

build:
	cargo build --release -p slate --bin $(APP_NAME)

test:
	cargo test --workspace

perf:
	node scripts/perf-check.mjs

install: build
	mkdir -p "$(BIN_DIR)"
	install -m 0755 "$(RELEASE_BIN)" "$(BIN_DIR)/$(APP_NAME)"
	mkdir -p "$(DESKTOP_DIR)" "$(PIXMAP_DIR)"
	install -m 0644 "$(APP_NAME).desktop" "$(DESKTOP_DIR)/$(APP_NAME).desktop"
	install -m 0644 "$(APP_NAME).png" "$(PIXMAP_DIR)/$(APP_NAME).png"
	@echo "Installed $(APP_NAME) to $(BIN_DIR)"
	@echo "Installed desktop launcher to $(DESKTOP_DIR)/$(APP_NAME).desktop"

release: release-linux release-macos
	@echo "Release artifacts are in $(BUILD_DIR)/"

release-linux:
	mkdir -p "$(BUILD_DIR)"
	rustup target add "$(LINUX_TARGET)"
	cargo build --release -p slate --bin $(APP_NAME) --target "$(LINUX_TARGET)"
	cp "target/$(LINUX_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-linux"

release-macos:
	mkdir -p "$(BUILD_DIR)"
	@if [ "$(HOST_OS)" = "Darwin" ]; then \
		rustup target add "$(MAC_INTEL_TARGET)" "$(MAC_ARM_TARGET)"; \
		cargo build --release -p slate --bin $(APP_NAME) --target "$(MAC_INTEL_TARGET)"; \
		cp "target/$(MAC_INTEL_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-intel"; \
		cargo build --release -p slate --bin $(APP_NAME) --target "$(MAC_ARM_TARGET)"; \
		cp "target/$(MAC_ARM_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-arm"; \
	elif [ "$(HAS_OSXCROSS)" = "1" ]; then \
		echo "Using osxcross toolchain for macOS cross-build"; \
		rustup target add "$(MAC_INTEL_TARGET)" "$(MAC_ARM_TARGET)"; \
		CC_x86_64_apple_darwin=o64-clang CXX_x86_64_apple_darwin=o64-clang++ cargo build --release -p slate --bin $(APP_NAME) --target "$(MAC_INTEL_TARGET)"; \
		cp "target/$(MAC_INTEL_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-intel"; \
		CC_aarch64_apple_darwin=oa64-clang CXX_aarch64_apple_darwin=oa64-clang++ cargo build --release -p slate --bin $(APP_NAME) --target "$(MAC_ARM_TARGET)"; \
		cp "target/$(MAC_ARM_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-arm"; \
	else \
		echo "Skipping macOS build: requires macOS host or osxcross (o64-clang/oa64-clang)."; \
	fi
