# ─── Config ───────────────────────────────────────────────────────────────────
CHAIN_ID    := bluechipChain
KEYRING     := test
FROM        := alice
CHAIN_BIN   := bluechipChaind
GAS         := 5000000
GAS_ADJ     := 1.3
WASM_TARGET := wasm32-unknown-unknown
ARTIFACTS   := artifacts

# ─── Build ────────────────────────────────────────────────────────────────────
build:
	@mkdir -p $(ARTIFACTS)
	RUSTFLAGS="-C link-arg=-s" cargo build --release --target $(WASM_TARGET)
	cp target/$(WASM_TARGET)/release/pool.wasm $(ARTIFACTS)/pool.wasm
	cp target/$(WASM_TARGET)/release/factory.wasm $(ARTIFACTS)/factory.wasm
	cp target/$(WASM_TARGET)/release/expand_economy.wasm $(ARTIFACTS)/expand_economy.wasm
	cp target/$(WASM_TARGET)/release/oracle.wasm $(ARTIFACTS)/oracle.wasm

test:
	cargo test

# ─── Docker Optimizer (workspace) ────────────────────────────────────────────
optimize:
	docker run --rm -v "$$(pwd)":/code \
	  --mount type=volume,source="$$(basename "$$(pwd)")_cache",target=/code/target \
	  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
	  cosmwasm/workspace-optimizer:0.15.0

# ─── Docker Optimizer (per-contract) ─────────────────────────────────────────
optimize-pool:
	docker run --rm -v ${CURDIR}:/code \
	  --mount type=volume,source=pool_cache,target=/target \
	  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
	  cosmwasm/optimizer:0.16.0 ./pool

optimize-factory:
	docker run --rm -v ${CURDIR}:/code \
	  --mount type=volume,source=factory_cache,target=/target \
	  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
	  cosmwasm/optimizer:0.16.0 ./factory

optimize-expand-economy:
	docker run --rm -v ${CURDIR}:/code \
	  --mount type=volume,source=expand_economy_cache,target=/target \
	  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
	  cosmwasm/optimizer:0.16.0 ./expand-economy

optimize-mockoracle:
	docker run --rm -v ${CURDIR}:/code \
	  --mount type=volume,source=mockoracle_cache,target=/target \
	  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
	  cosmwasm/optimizer:0.16.0 ./mockoracle

optimize-all: optimize-pool optimize-factory optimize-expand-economy optimize-mockoracle

# ─── Cosmwasm Check ──────────────────────────────────────────────────────────
check:
	cosmwasm-check $(ARTIFACTS)/pool.wasm
	cosmwasm-check $(ARTIFACTS)/factory.wasm
	cosmwasm-check $(ARTIFACTS)/expand_economy.wasm
	cosmwasm-check $(ARTIFACTS)/oracle.wasm

check-pool:
	cosmwasm-check $(ARTIFACTS)/pool.wasm

check-factory:
	cosmwasm-check $(ARTIFACTS)/factory.wasm

check-expand-economy:
	cosmwasm-check $(ARTIFACTS)/expand_economy.wasm

check-mockoracle:
	cosmwasm-check $(ARTIFACTS)/oracle.wasm

# ─── Deploy (store wasm on local chain) ──────────────────────────────────────
deploy-pool: build
	@echo "Deploying pool contract..."
	$(CHAIN_BIN) tx wasm store $(ARTIFACTS)/pool.wasm \
		--from $(FROM) \
		--chain-id $(CHAIN_ID) \
		--gas $(GAS) --gas-adjustment $(GAS_ADJ) \
		--keyring-backend $(KEYRING) \
		-y --output json

deploy-factory: build
	@echo "Deploying factory contract..."
	$(CHAIN_BIN) tx wasm store $(ARTIFACTS)/factory.wasm \
		--from $(FROM) \
		--chain-id $(CHAIN_ID) \
		--gas $(GAS) --gas-adjustment $(GAS_ADJ) \
		--keyring-backend $(KEYRING) \
		-y --output json

deploy-expand-economy: build
	@echo "Deploying expand-economy contract..."
	$(CHAIN_BIN) tx wasm store $(ARTIFACTS)/expand_economy.wasm \
		--from $(FROM) \
		--chain-id $(CHAIN_ID) \
		--gas $(GAS) --gas-adjustment $(GAS_ADJ) \
		--keyring-backend $(KEYRING) \
		-y --output json

deploy-mockoracle: build
	@echo "Deploying mock oracle contract..."
	$(CHAIN_BIN) tx wasm store $(ARTIFACTS)/oracle.wasm \
		--from $(FROM) \
		--chain-id $(CHAIN_ID) \
		--gas $(GAS) --gas-adjustment $(GAS_ADJ) \
		--keyring-backend $(KEYRING) \
		-y --output json

# ─── Full Stack Deploy ───────────────────────────────────────────────────────
deploy-all:
	@echo "For full stack deployment, use: ./deploy_robust.sh"
	@echo "It handles code ID extraction, instantiation, and linking."

.PHONY: build test optimize optimize-all optimize-pool optimize-factory optimize-expand-economy optimize-mockoracle \
	check check-pool check-factory check-expand-economy check-mockoracle \
	deploy-pool deploy-factory deploy-expand-economy deploy-mockoracle deploy-all
