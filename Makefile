# Rusty Beaver — run `make help` for targets.
# GNU make on macOS, Linux and Windows (Git Bash / MSYS2).

CARGO  ?= cargo
PREFIX ?= $(HOME)/.local
PORT   ?= 7979
IMAGE  ?= rust:1-bookworm
PG_IMAGE    ?= postgres:17-alpine
MYSQL_IMAGE ?= mysql:8
UNAME  := $(shell uname -s 2>/dev/null || echo Windows)
EXE    := $(if $(filter Windows MINGW% MSYS% CYGWIN%,$(UNAME)),.exe,)
BIN    := target/release/rusty-beaver$(EXE)
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
PID    := target/rusty-beaver.pid
LOG    := target/rusty-beaver.log
MCP    := python3 scripts/mcp.py

.PHONY: help build build-release release run dev start stop restart status logs test smoke lint fmt mcp-tools screenshots screenshot site \
        icons bundle install uninstall db-up db-down docker-ca linux-docker windows-docker clean distclean

help: ## list targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | sed 's/:.*## /\t/' | expand -t 16

build: ## debug build
	$(CARGO) build

build-release: ## optimized build (icon embedded on Windows)
	$(CARGO) build --release

release: test ## tag the Cargo.toml version (vX.Y.Z) and push it: CI builds the installers
	@git diff --quiet && git diff --cached --quiet || { echo "commit your changes first"; exit 1; }
	@! git rev-parse -q --verify refs/tags/v$(VERSION) >/dev/null || { echo "v$(VERSION) is already tagged: bump version in Cargo.toml"; exit 1; }
	git tag v$(VERSION)
	git push origin v$(VERSION)

run: build-release ## build and run in the foreground (MCP on http://127.0.0.1:$(PORT)/mcp)
	RUSTY_BEAVER_MCP_PORT=$(PORT) $(BIN)

dev: ## debug build + run
	RUSTY_BEAVER_MCP_PORT=$(PORT) $(CARGO) run

start: build-release ## run in the background (pid in target/, log: make logs)
	@if [ -f $(PID) ] && kill -0 $$(cat $(PID)) 2>/dev/null; then echo "already running (pid $$(cat $(PID)))"; \
	else RUSTY_BEAVER_MCP_PORT=$(PORT) $(BIN) > $(LOG) 2>&1 & echo $$! > $(PID); echo "started (pid $$(cat $(PID))), MCP: http://127.0.0.1:$(PORT)/mcp"; fi

stop: ## stop the background app
	@if [ -f $(PID) ] && kill $$(cat $(PID)) 2>/dev/null; then echo "stopped"; else echo "not running"; fi; rm -f $(PID)

restart: stop start ## restart the background app

status: ## is the background app running?
	@if [ -f $(PID) ] && kill -0 $$(cat $(PID)) 2>/dev/null; then echo "running (pid $$(cat $(PID)))"; else echo "not running"; fi

logs: ## follow the background app's log
	@tail -f $(LOG)

test: ## unit tests (and the web page's game rules, when node is installed)
	$(CARGO) test
	@if command -v node >/dev/null; then node scripts/game_test.js; else echo "node not found: skipped scripts/game_test.js"; fi

smoke: build ## end-to-end check: drive every feature over MCP (background app, throwaway files)
	python3 scripts/smoke.py target/debug/rusty-beaver$(EXE)

lint: ## clippy, warnings are errors
	$(CARGO) clippy -- -D warnings

fmt: ## format
	$(CARGO) fmt

mcp-tools: ## list the MCP tools of the running app
	RUSTY_BEAVER_MCP=http://127.0.0.1:$(PORT)/mcp $(MCP) tools

screenshots: build-release ## feature screenshots + animated tour -> docs/ (run `make db-up` first for PG/MySQL)
	python3 scripts/screenshots.py $(BIN)

screenshot: ## screenshot of the running app via MCP -> target/screenshot.png
	RUSTY_BEAVER_MCP=http://127.0.0.1:$(PORT)/mcp $(MCP) screenshot --png target/screenshot.png

site: ## GitHub Pages site: copy screenshots, tour GIF and icons into site/assets (open site/index.html)
	mkdir -p site/assets/screenshots
	cp docs/screenshots/*.png site/assets/screenshots/
	cp docs/tour.gif assets/icon.png assets/splash.png site/assets/
	cp assets/hicolor/64x64/apps/rusty-beaver.png site/assets/favicon.png

icons: ## regenerate the pixel-art icons + splash (python3; iconutil on macOS for .icns)
	python3 scripts/pixel_icon.py

bundle: ## macOS: target/release/RustyBeaver.app with icon
	./scripts/bundle-macos.sh

ifeq ($(UNAME),Darwin)
install: bundle ## install (macOS: ~/Applications; Linux: $(PREFIX) + .desktop + icons)
	mkdir -p $(HOME)/Applications
	rm -rf $(HOME)/Applications/RustyBeaver.app
	cp -R target/release/RustyBeaver.app $(HOME)/Applications/
	@echo "installed ~/Applications/RustyBeaver.app"
uninstall: ## remove an installed copy
	rm -rf $(HOME)/Applications/RustyBeaver.app
else
install: build-release
	install -Dm755 $(BIN) $(PREFIX)/bin/rusty-beaver$(EXE)
	install -Dm644 assets/rusty-beaver.desktop $(PREFIX)/share/applications/rusty-beaver.desktop
	cp -R assets/hicolor $(PREFIX)/share/icons/
	@echo "installed to $(PREFIX)"
uninstall:
	rm -f $(PREFIX)/bin/rusty-beaver$(EXE) $(PREFIX)/share/applications/rusty-beaver.desktop
	find $(PREFIX)/share/icons/hicolor -name rusty-beaver.png -delete 2>/dev/null || true
endif

db-up: ## start throwaway PostgreSQL (55432) + MySQL (53306) containers, password "beaver"
	-docker run -d --rm --name rb-pg -e POSTGRES_PASSWORD=beaver -p 55432:5432 $(PG_IMAGE)
	-docker run -d --rm --name rb-mysql -e MYSQL_ROOT_PASSWORD=beaver -e MYSQL_DATABASE=beaver -p 53306:3306 $(MYSQL_IMAGE)

db-down: ## stop the test database containers
	-docker stop rb-pg rb-mysql

docker-ca: ## macOS: let Docker builds trust the CAs this Mac trusts (TLS-inspecting proxies)
	security find-certificate -a -p /Library/Keychains/System.keychain \
	  /System/Library/Keychains/SystemRootCertificates.keychain > .docker-ca.pem
	@echo "wrote .docker-ca.pem; remove it to build without extra CAs"

linux-docker: ## build + test on Linux in Docker -> target/linux/rusty-beaver
	docker build -f scripts/Dockerfile --target linux --build-arg IMAGE=$(IMAGE) -t rusty-beaver-linux .
	mkdir -p target/linux
	id=$$(docker create rusty-beaver-linux) && docker cp $$id:/rusty-beaver target/linux/rusty-beaver; docker rm $$id >/dev/null

windows-docker: ## cross-build the Windows .exe (mingw) in Docker -> target/windows/rusty-beaver.exe
	docker build -f scripts/Dockerfile --target windows --build-arg IMAGE=$(IMAGE) -t rusty-beaver-windows .
	mkdir -p target/windows
	id=$$(docker create rusty-beaver-windows) && docker cp $$id:/rusty-beaver.exe target/windows/rusty-beaver.exe; docker rm $$id >/dev/null

clean: stop ## stop the app, remove build output and logs
	$(CARGO) clean
	rm -f screenshot.png

distclean: clean db-down ## clean + remove this project's docker images and build caches (saved connections are kept)
	-docker rmi rusty-beaver-linux rusty-beaver-windows 2>/dev/null
	-docker builder prune -f --filter 'description~=rusty-beaver' 2>/dev/null
	rm -f .docker-ca.pem
