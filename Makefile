IMAGE_NAME := contextforge-data-plane:latest
SERVICES ?= nginx gateway redis postgres pgbouncer migration fast_time_server register_fast_time
ARGS     ?=
CF_INTEGRATION ?= cf-integration
CF_INTEGRATION_DIR ?= $(CURDIR)/.integration
CF_DATAPLANE_REPO ?= $(CURDIR)
CF_DATAPLANE_REF ?= $(shell git -C "$(CF_DATAPLANE_REPO)" rev-parse HEAD)
CONFORMANCE_BASELINE_DIR := $(CURDIR)/tests/conformance/baselines

# Virtual-environment variables
VENV_DIR ?= $(CURDIR)/.venv

.PHONY: venv
venv:                                      ## Create the benchmark Python virtual environment
	@command -v uv >/dev/null 2>&1 || { echo "uv is required to create $(VENV_DIR)"; exit 1; }
	@test -d "$(VENV_DIR)" || uv venv "$(VENV_DIR)"

# MCP benchmark configuration
MCP_PROTOCOL_LOCUSTFILE            ?= tests/loadtest/locustfile_mcp_protocol.py
MCP_BENCHMARK_HOST                 ?= http://localhost:8080
MCP_BENCHMARK_SERVER_ID            ?= b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8
MCP_BENCHMARK_CONTROL_HOST         ?= http://localhost:4444
MCP_BENCHMARK_TOKEN_URL            ?= $(MCP_BENCHMARK_HOST)/contextforge-rs/admin/tokens/default/admin@example.com
MCP_BENCHMARK_USERS                ?= 125
MCP_BENCHMARK_SPAWN_RATE           ?= 30
MCP_BENCHMARK_RUN_TIME             ?= 60s
MCP_BENCHMARK_READY_TIMEOUT     ?= 90
MCP_BENCHMARK_READINESS_TOOL    ?= fast-time-verify-protocol
MCP_BENCHMARK_TOOL_DENYLIST        ?= schema_error,flaky
MCP_BENCHMARK_TOOLS_HTML_REPORT    ?= reports/benchmark_mcp_tools.html
MCP_BENCHMARK_TOOLS_CSV_PREFIX     ?= reports/benchmark_mcp_tools

# IBM detect-secrets hardened fork — pinned to the same commit used in mcp-context-forge.
DETECT_SECRETS_SPEC ?= git+https://github.com/ibm/detect-secrets.git@076672a9a01abdfc7ecee2e7d14f08cdccb73976
DETECT_SECRETS_EXCLUDE := '(?x)(Cargo\.lock$$|\.lock$$)|^\.secrets\.baseline$$'

.PHONY: help docker-prod compose-up compose-down conformance conformance-bless docs-serve pre-commit secrets-scan-all configure-git benchmark-mcp-tools

help: ## Show available commands
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-22s\033[0m %s\n", $$1, $$2}'

docker-prod: ## Build production Docker image with plugins and without testing-only with_tools
	docker build -t $(IMAGE_NAME) -f docker/Dockerfile .

compose-up: ## Launch stack: nginx, gateway, redis, postgres, pgbouncer, migration, fast_time_server, register_fast_time
	@docker image inspect $(IMAGE_NAME) >/dev/null 2>&1 || { \
		echo "Image $(IMAGE_NAME) not found. Run 'make docker-prod' first."; \
		exit 1; \
	}
	docker compose -f docker/docker-compose.yml up -d $(SERVICES) $(ARGS)

compose-down: ## Tear down the stack
	docker compose -f docker/docker-compose.yml stop $(SERVICES) $(ARGS)

conformance: ## Run strict modern MCP conformance against the committed data-plane HEAD
	@if ! command -v "$(CF_INTEGRATION)" >/dev/null 2>&1; then \
		echo "cf-integration not found: install its published binary with cargo binstall or set CF_INTEGRATION to its path."; \
		exit 1; \
	fi
	@if [ -n "$$(git -C "$(CF_DATAPLANE_REPO)" status --porcelain --untracked-files=no)" ]; then \
		echo "Tracked data-plane changes are not committed; commit or stash them before conformance."; \
		exit 1; \
	fi
	@CF_INTEGRATION_DIR="$(CF_INTEGRATION_DIR)" \
	CF_DATAPLANE_REPO="$(CF_DATAPLANE_REPO)" \
	CF_DATAPLANE_REF="$(CF_DATAPLANE_REF)" \
	"$(CF_INTEGRATION)" conformance run \
		--client-era modern \
		--server-era modern \
		--lane external \
		--standalone \
		--baseline-dir "$(CONFORMANCE_BASELINE_DIR)" \
		--output-dir "$(CF_INTEGRATION_DIR)/reports"

conformance-bless: ## Run strict modern conformance and atomically update its baselines
	@if ! command -v "$(CF_INTEGRATION)" >/dev/null 2>&1; then \
		echo "cf-integration not found: install its published binary with cargo binstall or set CF_INTEGRATION to its path."; \
		exit 1; \
	fi
	@if [ -n "$$(git -C "$(CF_DATAPLANE_REPO)" status --porcelain --untracked-files=no)" ]; then \
		echo "Tracked data-plane changes are not committed; commit or stash them before conformance."; \
		exit 1; \
	fi
	@CF_INTEGRATION_DIR="$(CF_INTEGRATION_DIR)" \
	CF_DATAPLANE_REPO="$(CF_DATAPLANE_REPO)" \
	CF_DATAPLANE_REF="$(CF_DATAPLANE_REF)" \
	"$(CF_INTEGRATION)" conformance run \
		--client-era modern \
		--server-era modern \
		--lane external \
		--standalone \
		--baseline-dir "$(CONFORMANCE_BASELINE_DIR)" \
		--output-dir "$(CF_INTEGRATION_DIR)/reports" \
		--bless

docs-serve: ## Serve the wiki book locally at http://127.0.0.1:3000
	mdbook serve _context/wiki --hostname 127.0.0.1 --port 3000 --open

pre-commit: ## Run all pre-commit hooks against every file
	@if ! command -v pre-commit >/dev/null 2>&1; then \
		echo "pre-commit not found. Install it with one of:"; \
		echo "  uv tool install pre-commit"; \
		echo "  brew install pre-commit"; \
		exit 1; \
	fi
	@mkdir -p .cache/pre-commit-home .cache/tmp .cache/cargo
	PRE_COMMIT_HOME='$(CURDIR)/.cache/pre-commit-home' \
	TMPDIR='$(CURDIR)/.cache/tmp' \
	CARGO_HOME='$(CURDIR)/.cache/cargo' \
	pre-commit run --config .pre-commit-config.yaml --all-files --show-diff-on-failure

secrets-scan-all: ## Full-tree scan — regenerate .secrets.baseline from scratch
	@if ! command -v detect-secrets >/dev/null 2>&1 && ! command -v uv >/dev/null 2>&1; then \
		echo "detect-secrets not found. Install it with:"; \
		echo "  uv tool install '$(DETECT_SECRETS_SPEC)'"; \
		exit 1; \
	fi
	@if command -v detect-secrets >/dev/null 2>&1; then \
		detect-secrets scan \
			--use-all-plugins \
			--exclude-files $(DETECT_SECRETS_EXCLUDE) \
			> .secrets.baseline; \
	else \
		uv tool run --from '$(DETECT_SECRETS_SPEC)' detect-secrets scan \
			--use-all-plugins \
			--exclude-files $(DETECT_SECRETS_EXCLUDE) \
			> .secrets.baseline; \
	fi
	@echo "✅ .secrets.baseline regenerated — audit new findings before committing"

# Internal target used by .gitattributes; intentionally omitted from `make help`.
configure-git:
	@common_dir=$$(git rev-parse --git-common-dir); \
	mkdir -p "$$common_dir/git-drivers"; \
	cp scripts/git/resolve-secrets-baseline-conflict.sh "$$common_dir/git-drivers/"; \
	chmod +x "$$common_dir/git-drivers/resolve-secrets-baseline-conflict.sh"; \
	git config merge.secrets-baseline.name "Regenerate .secrets.baseline via detect-secrets-scan"; \
	git config merge.secrets-baseline.driver \
		"$$common_dir/git-drivers/resolve-secrets-baseline-conflict.sh %O %A %B %P"

benchmark-mcp-tools:                        ## Quick tools-only MCP benchmark against the testing stack
	@echo "📊 Running tools-only MCP benchmark..."
	@echo "   Host: $(MCP_BENCHMARK_HOST)"
	@echo "   Server: $(MCP_BENCHMARK_SERVER_ID)"
	@test -d "$(VENV_DIR)" || $(MAKE) venv
	@$(VENV_DIR)/bin/python -c 'import jwt' >/dev/null 2>&1 || \
		uv pip install --python "$(VENV_DIR)/bin/python" 'PyJWT[crypto]'
	@mkdir -p reports
	@token="$${MCPGATEWAY_BEARER_TOKEN:-$$($(VENV_DIR)/bin/python -c 'import jwt; from datetime import datetime, timedelta, timezone; print(jwt.encode({"sub":"admin@example.com","tenantId":"default","exp":datetime.now(timezone.utc)+timedelta(hours=1)}, open("assets/jwt.key").read(), algorithm="RS256", headers={"kid":"test"}))')}"; \
		deadline=$$(( $$(date +%s) + $(MCP_BENCHMARK_READY_TIMEOUT) )); \
		while ! docker compose -f docker/docker-compose.yml exec -T -e TOKEN="$$token" gateway python3 -c 'import json, os, urllib.request; u="http://127.0.0.1:4445/contextforge-rs/servers/$(MCP_BENCHMARK_SERVER_ID)/mcp"; p={"jsonrpc":"2.0","id":"readiness","method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"benchmark-readiness","version":"1"},"io.modelcontextprotocol/clientCapabilities":{"tools":{},"resources":{},"prompts":{}}}}}; r=urllib.request.Request(u, data=json.dumps(p).encode(), method="POST", headers={"Authorization":"Bearer "+os.environ["TOKEN"],"Content-Type":"application/json","Accept":"application/json, text/event-stream","MCP-Protocol-Version":"2026-07-28","Mcp-Method":"server/discover"}); urllib.request.urlopen(r, timeout=5)' >/dev/null 2>&1; do \
			if [ "$$(date +%s)" -ge "$$deadline" ]; then \
				echo "Dataplane did not publish server $(MCP_BENCHMARK_SERVER_ID) on port 4445 within $(MCP_BENCHMARK_READY_TIMEOUT)s"; \
				exit 1; \
			fi; \
			sleep 1; \
		done
	@token="$${MCPGATEWAY_BEARER_TOKEN:-$$($(VENV_DIR)/bin/python -c 'import jwt; from datetime import datetime, timedelta, timezone; print(jwt.encode({"sub":"admin@example.com","tenantId":"default","exp":datetime.now(timezone.utc)+timedelta(hours=1)}, open("assets/jwt.key").read(), algorithm="RS256", headers={"kid":"test"}))')}"; \
		docker compose -f docker/docker-compose.yml exec -T -e TOKEN="$$token" gateway python3 -c 'import json, os, urllib.error, urllib.request; u="http://127.0.0.1:4445/contextforge-rs/servers/$(MCP_BENCHMARK_SERVER_ID)/mcp"; p={"jsonrpc":"2.0","id":"readiness-call","method":"tools/call","params":{"name":"$(MCP_BENCHMARK_READINESS_TOOL)","arguments":{},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"benchmark-readiness","version":"1"},"io.modelcontextprotocol/clientCapabilities":{"tools":{},"resources":{},"prompts":{}}}}}; r=urllib.request.Request(u, data=json.dumps(p).encode(), method="POST", headers={"Authorization":"Bearer "+os.environ["TOKEN"],"Content-Type":"application/json","Accept":"application/json, text/event-stream","MCP-Protocol-Version":"2026-07-28","Mcp-Method":"tools/call","Mcp-Name":"$(MCP_BENCHMARK_READINESS_TOOL)"}); x=urllib.request.urlopen(r, timeout=5); d=json.loads(x.read()); raise SystemExit(0 if "error" not in d else 1)' >/dev/null 2>&1 || { \
		echo "Dataplane has no callable route for $(MCP_BENCHMARK_READINESS_TOOL) on port 4445."; \
		echo "The control-plane publisher must emit explicit virtual_hosts.<server>.tools routes before benchmarking."; \
		exit 1; \
	}
	@/bin/bash -eu -o pipefail -c 'source $(VENV_DIR)/bin/activate && \
		LOCUST_LOG_LEVEL=$(MCP_BENCHMARK_LOCUST_LOG_LEVEL) MCP_SERVER_ID=$(MCP_BENCHMARK_SERVER_ID) \
		MCP_CONTROL_PLANE_HOST=$(MCP_BENCHMARK_CONTROL_HOST) MCP_TOKEN_URL=$(MCP_BENCHMARK_TOKEN_URL) \
		MCP_BENCHMARK_TOOL_POOL_SIZE=$(MCP_BENCHMARK_TOOL_POOL_SIZE) \
		MCP_BENCHMARK_TOOL_DENYLIST=$(MCP_BENCHMARK_TOOL_DENYLIST) \
		locust -f $(MCP_PROTOCOL_LOCUSTFILE) \
			--host=$(MCP_BENCHMARK_HOST) \
			--users=$(MCP_BENCHMARK_USERS) \
			--spawn-rate=$(MCP_BENCHMARK_SPAWN_RATE) \
			--run-time=$(MCP_BENCHMARK_RUN_TIME) \
			--tags call \
			--html=$(MCP_BENCHMARK_TOOLS_HTML_REPORT) \
			--csv=$(MCP_BENCHMARK_TOOLS_CSV_PREFIX) \
			--only-summary \
			MCPToolCallerUser'
	@echo ""
	@echo "📄 HTML Report: $(MCP_BENCHMARK_TOOLS_HTML_REPORT)"
	@echo "📊 CSV Reports: $(MCP_BENCHMARK_TOOLS_CSV_PREFIX)_stats.csv"
