APP_NAME := note
BIN_DIR := $(HOME)/.local/bin
RELEASE_BIN := src-tauri/target/release/$(APP_NAME)

.PHONY: all build install

all: install

build:
	npm install
	cargo tauri build

install: build
	mkdir -p "$(BIN_DIR)"
	install -m 0755 "$(RELEASE_BIN)" "$(BIN_DIR)/$(APP_NAME)"
	@echo "Installed $(APP_NAME) to $(BIN_DIR)"

release:
	npm install
	mkdir -p build
	-rustup target add x86_64-unknown-linux-gnu x86_64-pc-windows-gnu x86_64-apple-darwin aarch64-apple-darwin
	
	# Linux
	-cargo tauri build --target x86_64-unknown-linux-gnu
	-cp src-tauri/target/x86_64-unknown-linux-gnu/release/note build/note-linux
	
	# Windows
	-cargo tauri build --target x86_64-pc-windows-gnu
	-cp src-tauri/target/x86_64-pc-windows-gnu/release/note.exe build/note-windows.exe
	
	# macOS Intel
	-cargo tauri build --target x86_64-apple-darwin
	-cp src-tauri/target/x86_64-apple-darwin/release/note build/note-mac-intel

	# macOS Silicon
	-cargo tauri build --target aarch64-apple-darwin
	-cp src-tauri/target/aarch64-apple-darwin/release/note build/note-mac-arm
