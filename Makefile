.PHONY: optimize-pool optimize-factory optimize-airdrops optimize-all check-pool check-factory check-airdrops deploy-pool deploy-factory deploy-airdrops

# Test upgrade function
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


optimize-airdrops:
	docker run --rm -v ${CURDIR}:/code \
  --mount type=volume,source=airdrops_cache,target=/target \
  --mount type=volume,source=registry_cache,target=/usr/local/cargo/registry \
  cosmwasm/optimizer:0.16.0 ./airdrops

optimize-all:
	make optimize-pool optimize-factory optimize-airdrops

check-pool:
	cosmwasm-check artifacts/pool.wasm 

check-factory:
	  cosmwasm-check artifacts/factory.wasm 

check-airdrops:
	cosmwasm-check artifacts/airdrops.wasm

# Deployment targets
deploy-pool: optimize-pool
	@echo "Deploying pool contract..."
	seid tx wasm store artifacts/pool.wasm \
		--from taku \
		--node https://rpc-testnet.sei-apis.com \
		--chain-id atlantic-2 \
		-b block \
		--gas 5000000 \
		--fees 300000usei \
		-y


deploy-factory: optimize-factory
	@echo "Deploying factory contract..."
	seid tx wasm store artifacts/factory.wasm \
		--from taku \
		--node https://rpc-testnet.sei-apis.com \
		--chain-id atlantic-2 \
		-b block \
		--gas 5000000 \
		--fees 300000usei \
		-y

deploy-airdrops: optimize-airdrops
	@echo "Deploying airdrops contract..."
	seid tx wasm store artifacts/airdrops.wasm \
		--from taku \
		--node https://rpc-testnet.sei-apis.com \
		--chain-id atlantic-2 \
		-b block \
		--gas 5000000 \
		--fees 300000usei \
		-y


