# ─── Config ───────────────────────────────────────────────────────────────────

# Local chain config
LOCAL_CHAIN_ID    := bluechipChain
LOCAL_KEYRING     := test
LOCAL_FROM        := alice
LOCAL_CHAIN_BIN   := bluechipChaind
LOCAL_GAS         := 5000000
LOCAL_GAS_ADJ     := 1.3

# Sei testnet config
SEI_NODE          := https://rpc-testnet.sei-apis.com
SEI_CHAIN_ID      := atlantic-2
SEI_FROM          := taku

# Common
WASM_TARGET       := wasm32-unknown-unknown
ARTIFACTS         := artifacts

# ─── Build (local testing — includes mock oracle with testing feature) ────────
build:
	@mkdir -p $(ARTIFACTS)
	RUSTFLAGS="-C link-arg=-s" cargo build --release --target $(WASM_TARGET)
	RUSTFLAGS="-C link-arg=-s" cargo build --release --target $(WASM_TARGET) -p oracle --features testing
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

# ─── Local Chain Deploy (store wasm on local chain) ──────────────────────────
deploy-pool-local: build
	@echo "Deploying pool contract to local chain..."
	$(LOCAL_CHAIN_BIN) tx wasm store $(ARTIFACTS)/pool.wasm \
		--from $(LOCAL_FROM) \
		--chain-id $(LOCAL_CHAIN_ID) \
		--gas $(LOCAL_GAS) --gas-adjustment $(LOCAL_GAS_ADJ) \
		--keyring-backend $(LOCAL_KEYRING) \
		-y --output json

deploy-factory-local: build
	@echo "Deploying factory contract to local chain..."
	$(LOCAL_CHAIN_BIN) tx wasm store $(ARTIFACTS)/factory.wasm \
		--from $(LOCAL_FROM) \
		--chain-id $(LOCAL_CHAIN_ID) \
		--gas $(LOCAL_GAS) --gas-adjustment $(LOCAL_GAS_ADJ) \
		--keyring-backend $(LOCAL_KEYRING) \
		-y --output json

deploy-expand-economy-local: build
	@echo "Deploying expand-economy contract to local chain..."
	$(LOCAL_CHAIN_BIN) tx wasm store $(ARTIFACTS)/expand_economy.wasm \
		--from $(LOCAL_FROM) \
		--chain-id $(LOCAL_CHAIN_ID) \
		--gas $(LOCAL_GAS) --gas-adjustment $(LOCAL_GAS_ADJ) \
		--keyring-backend $(LOCAL_KEYRING) \
		-y --output json

deploy-mockoracle-local: build
	@echo "Deploying mock oracle contract to local chain..."
	$(LOCAL_CHAIN_BIN) tx wasm store $(ARTIFACTS)/oracle.wasm \
		--from $(LOCAL_FROM) \
		--chain-id $(LOCAL_CHAIN_ID) \
		--gas $(LOCAL_GAS) --gas-adjustment $(LOCAL_GAS_ADJ) \
		--keyring-backend $(LOCAL_KEYRING) \
		-y --output json

# ─── Full Stack Local Deploy ─────────────────────────────────────────────────
deploy-all-local:
	@echo "For full stack local deployment, use: ./deploy_full_stack_mock_oracle.sh"
	@echo "For robust deployment with base contracts: ./deploy_robust.sh"

# ─── Sei Testnet Deploy ─────────────────────────────────────────────────────
deploy-pool: optimize-pool
	@echo "Deploying pool contract to Sei testnet..."
	seid tx wasm store artifacts/pool.wasm \
		--from $(SEI_FROM) \
		--node $(SEI_NODE) \
		--chain-id $(SEI_CHAIN_ID) \
		-b block \
		--gas 5000000 \
		--fees 300000usei \
		-y | tee ./config/pool_deploy_result.txt

deploy-factory: optimize-factory
	@echo "Deploying factory contract to Sei testnet..."
	seid tx wasm store artifacts/factory.wasm \
		--from $(SEI_FROM) \
		--node $(SEI_NODE) \
		--chain-id $(SEI_CHAIN_ID) \
		-b block \
		--gas 5000000 \
		--fees 300000usei \
		-y | tee ./config/factory_deploy_result.txt

init-factory:
	seid tx wasm instantiate 10234 "$$(cat ./config/factory_init.json)" \
		--from $(SEI_FROM) \
		--label "bluechip_factory" \
		--admin $$(seid keys show $(SEI_FROM) -a) \
		--node $(SEI_NODE) \
		--chain-id $(SEI_CHAIN_ID) \
		--gas 5000000 \
		--fees 300000usei \
		-b block \
		-y | tee ./config/factory_init_result.txt

init-pool:
	seid tx wasm instantiate 10236 "$$(cat ./config/pool_init.json)" \
		--from $(SEI_FROM) \
		--label "bluechip_pool" \
		--admin $$(seid keys show $(SEI_FROM) -a) \
		--node $(SEI_NODE) \
		--chain-id $(SEI_CHAIN_ID) \
		--gas 5000000 \
		--fees 300000usei \
		-b block \
		-y | tee ./config/pool_init_result.txt

.PHONY: build test optimize optimize-all optimize-pool optimize-factory optimize-expand-economy optimize-mockoracle \
	check check-pool check-factory check-expand-economy check-mockoracle \
	deploy-pool-local deploy-factory-local deploy-expand-economy-local deploy-mockoracle-local deploy-all-local \
