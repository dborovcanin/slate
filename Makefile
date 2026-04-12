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
