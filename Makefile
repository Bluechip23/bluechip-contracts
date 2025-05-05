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
		-y | tee ./config/pool_deploy_result.txt


deploy-factory: optimize-factory
	@echo "Deploying factory contract..."
	seid tx wasm store artifacts/factory.wasm \
		--from taku \
		--node https://rpc-testnet.sei-apis.com \
		--chain-id atlantic-2 \
		-b block \
		--gas 5000000 \
		--fees 300000usei \
		-y | tee ./config/factory_deploy_result.txt

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

init-factory:
	seid tx wasm instantiate 10234 "$$(cat ./config/factory_init.json)" \
		--from taku \
		--label "bluechip_factory" \
		--admin $$(seid keys show taku -a) \
		--node https://rpc-testnet.sei-apis.com \
		--chain-id atlantic-2 \
		--gas 5000000 \
		--fees 300000usei \
		-b block \
		-y | tee ./config/factory_init_result.txt

init-pool:
	seid tx wasm instantiate 10236 "$$(cat ./config/pool_init.json)" \
		--from taku \
		--label "bluechip_pool" \
		--admin $$(seid keys show taku -a) \
		--node https://rpc-testnet.sei-apis.com \
		--chain-id atlantic-2 \
		--gas 5000000 \
		--fees 300000usei \
		-b block \
		-y | tee ./config/pool_init_result.txt