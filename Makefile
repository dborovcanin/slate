APP_NAME := note
MSG_NAME := note-msg
BIN_DIR := $(HOME)/.local/bin
TAURI_DIR := src-tauri
ICON_DIR := $(TAURI_DIR)/icons
BUILD_DIR := build

LINUX_TARGET := x86_64-unknown-linux-gnu
WINDOWS_TARGET := x86_64-pc-windows-gnu
MAC_INTEL_TARGET := x86_64-apple-darwin
MAC_ARM_TARGET := aarch64-apple-darwin

HOST_OS := $(shell uname -s)
HAS_MINGW := $(shell command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1 && echo 1 || echo 0)
HAS_OSXCROSS := $(shell command -v o64-clang >/dev/null 2>&1 && command -v oa64-clang >/dev/null 2>&1 && echo 1 || echo 0)
HAS_MAGICK := $(shell command -v magick >/dev/null 2>&1 && echo 1 || echo 0)

RELEASE_NOTE_BIN := $(TAURI_DIR)/target/release/$(APP_NAME)
RELEASE_MSG_BIN := $(TAURI_DIR)/target/release/$(MSG_NAME)

.PHONY: all npm-install build install ensure-icons release release-linux release-windows release-macos

all: install

npm-install:
	npm install

build: npm-install
	cargo tauri build

install: build
	mkdir -p "$(BIN_DIR)"
	install -m 0755 "$(RELEASE_NOTE_BIN)" "$(BIN_DIR)/$(APP_NAME)"
	install -m 0755 "$(RELEASE_MSG_BIN)" "$(BIN_DIR)/$(MSG_NAME)"
	@echo "Installed $(APP_NAME) and $(MSG_NAME) to $(BIN_DIR)"

ensure-icons:
	@mkdir -p "$(ICON_DIR)"
	@if [ ! -f "$(ICON_DIR)/icon.ico" ]; then \
		if [ "$(HAS_MAGICK)" = "1" ] && [ -f "$(ICON_DIR)/256x256.png" ]; then \
			echo "Generating $(ICON_DIR)/icon.ico from 256x256.png"; \
			magick "$(ICON_DIR)/256x256.png" -define icon:auto-resize=16,24,32,48,64,128,256 "$(ICON_DIR)/icon.ico"; \
		else \
			echo "error: missing $(ICON_DIR)/icon.ico and ImageMagick (magick) is unavailable."; \
			echo "       add an .ico icon file or install ImageMagick."; \
			exit 1; \
		fi; \
	fi

release: npm-install ensure-icons release-linux release-windows release-macos
	@echo "Release artifacts are in $(BUILD_DIR)/"

release-linux:
	mkdir -p "$(BUILD_DIR)"
	rustup target add "$(LINUX_TARGET)"
	cargo tauri build --target "$(LINUX_TARGET)"
	cp "$(TAURI_DIR)/target/$(LINUX_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-linux"
	@if [ -f "$(TAURI_DIR)/target/$(LINUX_TARGET)/release/$(MSG_NAME)" ]; then \
		cp "$(TAURI_DIR)/target/$(LINUX_TARGET)/release/$(MSG_NAME)" "$(BUILD_DIR)/$(MSG_NAME)-linux"; \
	fi

release-windows:
	mkdir -p "$(BUILD_DIR)"
	@if [ "$(HAS_MINGW)" != "1" ]; then \
		echo "Skipping Windows build: x86_64-w64-mingw32-gcc not found."; \
		exit 0; \
	fi
	rustup target add "$(WINDOWS_TARGET)"
	cargo tauri build --target "$(WINDOWS_TARGET)"
	cp "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/$(APP_NAME).exe" "$(BUILD_DIR)/$(APP_NAME)-windows.exe"
	@if [ -f "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/$(MSG_NAME).exe" ]; then \
		cp "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/$(MSG_NAME).exe" "$(BUILD_DIR)/$(MSG_NAME)-windows.exe"; \
	fi

release-macos:
	mkdir -p "$(BUILD_DIR)"
	@if [ "$(HOST_OS)" = "Darwin" ]; then \
		rustup target add "$(MAC_INTEL_TARGET)" "$(MAC_ARM_TARGET)"; \
		cargo tauri build --target "$(MAC_INTEL_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-intel"; \
		cargo tauri build --target "$(MAC_ARM_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-arm"; \
		if [ -f "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(MSG_NAME)" ]; then \
			cp "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(MSG_NAME)" "$(BUILD_DIR)/$(MSG_NAME)-mac-intel"; \
		fi; \
		if [ -f "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(MSG_NAME)" ]; then \
			cp "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(MSG_NAME)" "$(BUILD_DIR)/$(MSG_NAME)-mac-arm"; \
		fi; \
	elif [ "$(HAS_OSXCROSS)" = "1" ]; then \
		echo "Using osxcross toolchain for macOS cross-build"; \
		rustup target add "$(MAC_INTEL_TARGET)" "$(MAC_ARM_TARGET)"; \
		CC_x86_64_apple_darwin=o64-clang CXX_x86_64_apple_darwin=o64-clang++ cargo tauri build --target "$(MAC_INTEL_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-intel"; \
		CC_aarch64_apple_darwin=oa64-clang CXX_aarch64_apple_darwin=oa64-clang++ cargo tauri build --target "$(MAC_ARM_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-arm"; \
		if [ -f "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(MSG_NAME)" ]; then \
			cp "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(MSG_NAME)" "$(BUILD_DIR)/$(MSG_NAME)-mac-intel"; \
		fi; \
		if [ -f "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(MSG_NAME)" ]; then \
			cp "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(MSG_NAME)" "$(BUILD_DIR)/$(MSG_NAME)-mac-arm"; \
		fi; \
	else \
		echo "Skipping macOS build: requires macOS host or osxcross (o64-clang/oa64-clang)."; \
	fi
