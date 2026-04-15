APP_NAME := slate
BIN_DIR := $(HOME)/.local/bin
TAURI_DIR := src-tauri
ICON_DIR := $(TAURI_DIR)/icons
BUILD_DIR := build

LINUX_TARGET := x86_64-unknown-linux-gnu
WINDOWS_TARGET := x86_64-pc-windows-msvc
MAC_INTEL_TARGET := x86_64-apple-darwin
MAC_ARM_TARGET := aarch64-apple-darwin

HOST_OS := $(shell uname -s)
IS_NATIVE_WINDOWS_HOST := $(shell if printf '%s' "$(HOST_OS)" | grep -Eq '^(MINGW|MSYS|CYGWIN)'; then echo 1; else echo 0; fi)
HAS_MSVC_CL := $(shell command -v cl >/dev/null 2>&1 && echo 1 || echo 0)
HAS_OSXCROSS := $(shell command -v o64-clang >/dev/null 2>&1 && command -v oa64-clang >/dev/null 2>&1 && echo 1 || echo 0)
HAS_MAGICK := $(shell command -v magick >/dev/null 2>&1 && echo 1 || echo 0)

RELEASE_NOTE_BIN := $(TAURI_DIR)/target/release/$(APP_NAME)

.PHONY: all npm-install build install ensure-icons release release-linux release-windows release-macos

all: install

npm-install:
	npm install

build: npm-install
	cargo tauri build

install: build
	mkdir -p "$(BIN_DIR)"
	install -m 0755 "$(RELEASE_NOTE_BIN)" "$(BIN_DIR)/$(APP_NAME)"
	@echo "Installed $(APP_NAME) to $(BIN_DIR)"

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

release-windows:
	mkdir -p "$(BUILD_DIR)"
	@if [ "$(IS_NATIVE_WINDOWS_HOST)" != "1" ]; then \
		echo "Skipping Windows installer build: MSVC target requires a native Windows shell (MSYS2/Git-Bash/Cygwin) or CI on windows-latest."; \
		echo "Hint: do not run release-windows from Linux/WSL."; \
	elif [ "$(HAS_MSVC_CL)" != "1" ]; then \
		echo "error: cl.exe not found in PATH."; \
		echo "       open 'x64 Native Tools Command Prompt for VS' (or run VsDevCmd.bat) before running make release-windows."; \
		exit 1; \
	else \
		rustup target add "$(WINDOWS_TARGET)"; \
		env -u CC -u CXX -u AR -u CFLAGS -u CXXFLAGS cargo tauri build --target "$(WINDOWS_TARGET)"; \
		if ls "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/bundle/nsis/"*.exe >/dev/null 2>&1; then \
			cp "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/bundle/nsis/"*.exe "$(BUILD_DIR)/"; \
		else \
			echo "error: no NSIS installer was generated for target $(WINDOWS_TARGET)."; \
			exit 1; \
		fi; \
		if [ -f "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/$(APP_NAME).exe" ]; then \
			cp "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/$(APP_NAME).exe" "$(BUILD_DIR)/$(APP_NAME)-windows-msvc.exe"; \
		fi; \
		if [ -f "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/WebView2Loader.dll" ]; then \
			cp "$(TAURI_DIR)/target/$(WINDOWS_TARGET)/release/WebView2Loader.dll" "$(BUILD_DIR)/WebView2Loader.dll"; \
		fi; \
	fi

release-macos:
	mkdir -p "$(BUILD_DIR)"
	@if [ "$(HOST_OS)" = "Darwin" ]; then \
		rustup target add "$(MAC_INTEL_TARGET)" "$(MAC_ARM_TARGET)"; \
		cargo tauri build --target "$(MAC_INTEL_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-intel"; \
		cargo tauri build --target "$(MAC_ARM_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-arm"; \
	elif [ "$(HAS_OSXCROSS)" = "1" ]; then \
		echo "Using osxcross toolchain for macOS cross-build"; \
		rustup target add "$(MAC_INTEL_TARGET)" "$(MAC_ARM_TARGET)"; \
		CC_x86_64_apple_darwin=o64-clang CXX_x86_64_apple_darwin=o64-clang++ cargo tauri build --target "$(MAC_INTEL_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_INTEL_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-intel"; \
		CC_aarch64_apple_darwin=oa64-clang CXX_aarch64_apple_darwin=oa64-clang++ cargo tauri build --target "$(MAC_ARM_TARGET)"; \
		cp "$(TAURI_DIR)/target/$(MAC_ARM_TARGET)/release/$(APP_NAME)" "$(BUILD_DIR)/$(APP_NAME)-mac-arm"; \
	else \
		echo "Skipping macOS build: requires macOS host or osxcross (o64-clang/oa64-clang)."; \
	fi
