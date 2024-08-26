#!/bin/bash

#set -euo pipefail

DASHES='------'

# TODO: this script is going to be based on which root is selected, e.g. local, calibration, or other
if (($# != 1)); then
  echo "Arguments: <Specify github remote branch name to use to deploy. Or use 'local' (without quote) to indicate using local repo instead. If not provided, will default to main branch"
  head_ref=main
  local_deploy=false
else
  if [ "$1" = "local" ]; then
    echo "$DASHES deploying to local net $DASHES"
    local_deploy=true
  else
    local_deploy=false
    head_ref=$1
  fi
fi

# don't need ssh-add etc for local
if ! $local_deploy ; then
  eval "$(ssh-agent -s)"
  ssh-add
  if [ -e "${HOME}"/.ssh/id_rsa.ipc ]; then
    ssh-add "${HOME}"/.ssh/id_rsa.ipc
  fi

  # TODO: we won't have the supply source for local until after we deploy it
  if [[ ! -v SUPPLY_SOURCE_ADDRESS ]]; then
    echo "SUPPLY_SOURCE_ADDRESS is not set"
    exit 1
  fi

  # TODO: we won't need a parent auth token for local
  if [[ ! -v PARENT_HTTP_AUTH_TOKEN ]]; then
    echo "PARENT_HTTP_AUTH_TOKEN is not set"
    exit 1
  fi
else
  # we set SUPPLY_SOURCE_ADDRESS below for the local net
  # TODO: should this just be an empty string for local? maybe hust leave it undefined?
  PARENT_HTTP_AUTH_TOKEN=""
fi

if [[ $local_deploy = true ]]; then
  dir=$(dirname -- "$(readlink -f -- "${BASH_SOURCE[0]}")")
  IPC_FOLDER=$(readlink -f -- "$dir"/../..)
fi
if [[ ! -v IPC_FOLDER ]]; then
    IPC_FOLDER=${HOME}/ipc
fi
IPC_CONFIG_FOLDER=${HOME}/.ipc

echo "$DASHES starting with env $DASHES"
echo IPC_FOLDER $IPC_FOLDER
echo IPC_CONFIG_FOLDER $IPC_CONFIG_FOLDER

wallet_addresses=()
public_keys=()
CMT_P2P_HOST_PORTS=(26656 26756 26856)
CMT_RPC_HOST_PORTS=(26657 26757 26857)
ETHAPI_HOST_PORTS=(8645 8745 8845)
RESOLVER_HOST_PORTS=(26655 26755 26855)
OBJECTS_HOST_PORTS=(8001 8002 8003)
IROH_RPC_HOST_PORTS=(4921 4922 4923)

FENDERMINT_METRICS_HOST_PORTS=(9184 9185 9186)
IROH_METRICS_HOST_PORTS=(9091 9092 9093)
PROMTAIL_AGENT_HOST_PORTS=(9080 9081 9082)

PROMETHEUS_HOST_PORT=9090
LOKI_HOST_PORT=3100
GRAFANA_HOST_PORT=3000
ANVIL_HOST_PORT=8545

if [[ -z ${PARENT_ENDPOINT+x} ]]; then
  if [[ $local_deploy == true ]]; then
    echo "$DASHES using local PARENT_ENDPOINT"
    PARENT_ENDPOINT="http://anvil:${ANVIL_HOST_PORT}"
  else
    echo "$DASHES using calibration PARENT_ENDPOINT"
    PARENT_ENDPOINT="https://calibration.node.glif.io/archive/lotus/rpc/v1"
  fi
fi

# Install build dependencies
if [[ -z ${SKIP_BUILD+x} || "$SKIP_BUILD" == "" || "$SKIP_BUILD" == "false" ]]; then
  echo "${DASHES} Installing build dependencies..."
  sudo apt update && sudo apt install build-essential libssl-dev mesa-opencl-icd ocl-icd-opencl-dev gcc git bzr jq pkg-config curl clang hwloc libhwloc-dev wget ca-certificates gnupg -y

  # Install rust + cargo
  echo "$DASHES Check rustc & cargo..."
  if which cargo ; then
    echo "$DASHES rustc & cargo already installed."
  else
    echo "$DASHES Need to install rustc & cargo"
    curl https://sh.rustup.rs -sSf | sh -s -- -y
    # Refresh env
    source "${HOME}"/.bashrc
  fi

  # Install cargo make
  echo "$DASHES Installing cargo-make"
  cargo install cargo-make
  # Install toml-cli
  echo "$DASHES Installing toml-cli"
  cargo install toml-cli

  # Install Foundry
  echo "$DASHES Check foundry..."
  if which foundryup ; then
    echo "$DASHES foundry is already installed."
  else
    echo "$DASHES Need to install foundry"
    curl -L https://foundry.paradigm.xyz | bash
    foundryup
  fi

  # Install node
  echo "$DASHES Check node..."
  if which node ; then
    echo "$DASHES node is already installed."
  else
    echo "$DASHES Need to install node"
    curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.3/install.sh | bash
    source "$HOME/.bashrc"
    nvm install --default lts/*
  fi

  # Install docker
  echo "$DASHES check docker"
  if which docker ; then
    echo "$DASHES docker is already installed."
  else
    echo "$DASHES Need to install docker"
    # Add Docker's official GPG key:
    sudo apt-get update
    sudo apt-get install ca-certificates curl
    sudo install -m 0755 -d /etc/apt/keyrings
    sudo curl -fsSL https://download.docker.com/linux/ubuntu/gpg -o /etc/apt/keyrings/docker.asc
    sudo chmod a+r /etc/apt/keyrings/docker.asc

    # Add the repository to Apt sources:
    echo \
      "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/ubuntu \
      $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
      sudo tee /etc/apt/sources.list.d/docker.list > /dev/null
    sudo apt-get update
    sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin

    # Remove the need to use sudo
    getent group docker || sudo groupadd docker
    sudo usermod -aG docker "$USER"
    newgrp docker

    # Test running docker without sudo
    docker ps
  fi

  # Make sure we re-read the latest env before finishing dependency installation.
  set +u
  source "${HOME}"/.bashrc
  set -u
else
  echo "$DASHES skpping dependencies install and build $DASHES"
fi

# Prepare code repo
if ! $local_deploy ; then
  # TODO: this is only really needed when the machine is "brand new" e.g. gcp
  #       we should probably add some other env or flag to check for this.
  echo "$DASHES Preparing ipc repo..."
  if ! ls "$IPC_FOLDER" ; then
    git clone --recurse-submodules -j8 git@github.com-ipc:amazingdatamachine/ipc.git "${IPC_FOLDER}"
  fi
  cd "${IPC_FOLDER}"
  git fetch
  git stash
  git checkout "$head_ref"
  git pull --rebase origin "$head_ref"
  git submodule sync
  git submodule update --init --recursive
fi

# Stop prometheus
echo "IPC_FOLDER" "$IPC_FOLDER"
cd "$IPC_FOLDER"
cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=prometheus \
    prometheus-destroy

# Stop grafana
cd "$IPC_FOLDER"
cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=grafana \
    grafana-destroy

# Stop loki
cd "$IPC_FOLDER"
cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=loki \
    loki-destroy

# TODO: does this work for local and calibration?
if [ -e "${IPC_CONFIG_FOLDER}/config.toml" ]; then
    subnet_id=$(toml get -r "${IPC_CONFIG_FOLDER}"/config.toml subnets[1].id)
    echo "Existing subnet id: $subnet_id"
    # Stop validators
    cd "$IPC_FOLDER"
    for i in {0..2}
    do
      cargo make --makefile infra/fendermint/Makefile.toml \
          -e NODE_NAME=validator-"$i" \
          -e SUBNET_ID="$subnet_id" \
          child-validator-down
    done
fi

# Remove existing deployment
rm -rf "$IPC_CONFIG_FOLDER"
mkdir -p "$IPC_CONFIG_FOLDER"

# Copy configs
# TODO: how do we know the evm_keystore.json exists?
if ! $local_deploy ; then
  echo "$DASHES using calibration net config $DASHES"
  cp "$HOME"/evm_keystore.json "$IPC_CONFIG_FOLDER"
  cp "$IPC_FOLDER"/scripts/deploy_subnet/.ipc-cal/config.toml "$IPC_CONFIG_FOLDER"
else
  echo "$DASHES using local net config $DASHES"
  cp "$IPC_FOLDER"/scripts/deploy_subnet/.ipc-local/config.toml "$IPC_CONFIG_FOLDER"
fi
cp "$IPC_FOLDER"/infra/prometheus/prometheus.yaml "$IPC_CONFIG_FOLDER"
cp "$IPC_FOLDER"/infra/loki/loki-config.yaml "$IPC_CONFIG_FOLDER"
cp "$IPC_FOLDER"/infra/promtail/promtail-config.yaml "$IPC_CONFIG_FOLDER"
cp "$IPC_FOLDER"/infra/iroh/iroh-config.yaml "$IPC_CONFIG_FOLDER"

if [[ -z ${SKIP_BUILD+x} || "$SKIP_BUILD" == "" || "$SKIP_BUILD" == "false" ]]; then
  # Build contracts
  echo "$DASHES Building ipc contracts..."
  cd "${IPC_FOLDER}"/contracts
  make build

  # Build ipc-cli
  echo "$DASHES Building ipc-cli..."
  cd "${IPC_FOLDER}"/ipc
  make install
fi


if [[ $local_deploy = true ]]; then
  # TODO: should we remove and recreate the docker network somewhere here?
  subnet_id="/r31337/t410f6dl55afbyjbpupdtrmedyqrnmxdmpk7rxuduafq"
  cd "$IPC_FOLDER"
  cargo make --makefile infra/fendermint/Makefile.toml \
      -e NODE_NAME=anvil \
      anvil-destroy

  # note: the subnet_id is used here because it's how we define the name of the docker network
  cd "$IPC_FOLDER"
  cargo make --makefile infra/fendermint/Makefile.toml \
      -e NODE_NAME=anvil \
      -e SUBNET_ID="$subnet_id" \
      -e ANVIL_HOST_PORT="${ANVIL_HOST_PORT}" \
      anvil-start

  ipc-cli wallet import --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 --wallet-type evm
  ipc-cli wallet import --private-key 0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d --wallet-type evm
  ipc-cli wallet import --private-key 0x5de4111afa1a4b94908f83103eb1f1706367c2e68ca870fc3fb9a804cdab365a --wallet-type evm
fi

# Prepare wallet by using existing wallet json file
echo "$DASHES Using 3 addresses in wallet..."
for i in {0..2}
do
  addr=$(jq .["$i"].address < "${IPC_CONFIG_FOLDER}"/evm_keystore.json | tr -d '"')
  wallet_addresses+=("$addr")
  echo "Wallet $i address: $addr"
  pk=$(ipc-cli wallet pub-key --wallet-type evm --address "$addr" | tr -d '"')
  public_keys+=("$pk")
done

default_wallet_address=${wallet_addresses[0]}
echo "Default wallet address: $default_wallet_address"

# Export validator private keys into files
for i in {0..2}
do
  ipc-cli wallet export --wallet-type evm --address "${wallet_addresses[i]}" --hex > "${IPC_CONFIG_FOLDER}"/validator_"${i}".sk
  echo "Export private key for ${wallet_addresses[i]} to ${IPC_CONFIG_FOLDER}/validator_${i}.sk"
done

# Update IPC config file with parent auth token
toml set "${IPC_CONFIG_FOLDER}"/config.toml subnets[0].config.auth_token "$PARENT_HTTP_AUTH_TOKEN" > /tmp/config.toml.0
cp /tmp/config.toml.0 "${IPC_CONFIG_FOLDER}"/config.toml

# Deploy IPC contracts
if [[ ! -v PARENT_GATEWAY_ADDRESS || ! -v PARENT_REGISTRY_ADDRESS ]]; then
  echo "$DASHES Deploying new IPC contracts..."
  cd "${IPC_FOLDER}"/contracts
  npm install

  if ! $local_deploy ; then
    export RPC_URL=https://calibration.filfox.info/rpc/v1
  else 
    export RPC_URL=http://localhost:8545
  fi
  pk=$(cat "${IPC_CONFIG_FOLDER}"/validator_0.sk)
  export PRIVATE_KEY=$pk

  if ! $local_deploy ; then
    deploy_contracts_output=$(make deploy-ipc NETWORK=calibrationnet)
  else
    deploy_contracts_output=$(make deploy-ipc NETWORK=localnet)
  fi

echo "$DASHES deploy contracts output $DASHES"
echo ""
echo "$deploy_contracts_output"
echo ""

  PARENT_GATEWAY_ADDRESS=$(echo "$deploy_contracts_output" | grep '"Gateway"' | awk -F'"' '{print $4}')
# TODO: remove echos in this `if`` block, they are for debug
echo "Gateway address: $PARENT_GATEWAY_ADDRESS"

  PARENT_REGISTRY_ADDRESS=$(echo "$deploy_contracts_output" | grep '"SubnetRegistry"' | awk -F'"' '{print $4}')
echo "Registry address: $PARENT_REGISTRY_ADDRESS"
  # TODO: for local we need to deploy the erc20 supply source token to the anvil parent

  # TODO: need to decide how to handle local contracts repo. This just assumes the use has already cloned `contracts`
  #       then run  `forge install` and `forge build`
  cd "${IPC_FOLDER}/../contracts"
  # need to run clean or we hit upgradeable saftey validation errors resulting from contracts with the same name
  forge clean
  # TODO: dockerize this command?
  # privkey is anvil #4
  deploy_supply_source_token_out="$(PRIVATE_KEY=0x47e179ec197488593b187f80a00eb0da91f1b9d0b13f8733639f19c30a34926a forge script script/Hoku.s.sol --tc DeployScript 0 --sig 'run(uint8)' --rpc-url "http://localhost:${ANVIL_HOST_PORT}" --broadcast -vv)"

echo "$DASHES deploy suppply source token output $DASHES"
echo ""
echo "$deploy_supply_source_token_out"
echo ""
# 0x2910E325cf29dd912E3476B61ef12F49cb931096
  SUPPLY_SOURCE_ADDRESS=$(echo "$deploy_supply_source_token_out" | grep -oP 'contract Hoku\s\K\w+')

  cd "$IPC_FOLDER"
fi
echo "Gateway address: $PARENT_GATEWAY_ADDRESS"
echo "Registry address: $PARENT_REGISTRY_ADDRESS"
echo "Supply source address: $SUPPLY_SOURCE_ADDRESS"

# Use the parent gateway and registry address to update IPC config file
toml set "${IPC_CONFIG_FOLDER}"/config.toml subnets[0].config.gateway_addr "$PARENT_GATEWAY_ADDRESS" > /tmp/config.toml.1
toml set /tmp/config.toml.1 subnets[0].config.registry_addr "$PARENT_REGISTRY_ADDRESS" > /tmp/config.toml.2
cp /tmp/config.toml.2 "${IPC_CONFIG_FOLDER}"/config.toml

# Create a subnet
echo "$DASHES Creating a child subnet..."
root_id=$(toml get "${IPC_CONFIG_FOLDER}"/config.toml subnets[0].id | tr -d '"')
echo "Using root: $root_id"
create_subnet_output=$(ipc-cli subnet create --from $default_wallet_address --parent $root_id --min-validators 2 --min-validator-stake 1 --bottomup-check-period 600 --active-validators-limit 3 --permission-mode federated --supply-source-kind erc20 --supply-source-address $SUPPLY_SOURCE_ADDRESS 2>&1)

echo "$DASHES create subnet output $DASHES"
echo ""
echo "$create_subnet_output"
echo ""

# shellcheck disable=SC2086
subnet_id=$(echo $create_subnet_output | sed 's/.*with id: \([^ ]*\).*/\1/')
echo "Created new subnet id: $subnet_id"

# Use the new subnet ID to update IPC config file
toml set "${IPC_CONFIG_FOLDER}"/config.toml subnets[1].id "$subnet_id" > /tmp/config.toml.3
cp /tmp/config.toml.3 "${IPC_CONFIG_FOLDER}"/config.toml

# Set federated power
ipc-cli subnet set-federated-power --from "$default_wallet_address" --subnet "$subnet_id" --validator-addresses "${wallet_addresses[@]}" --validator-pubkeys "${public_keys[@]}" --validator-power 1 1 1

if [[ -z ${SKIP_BUILD+x} || "$SKIP_BUILD" == "" || "$SKIP_BUILD" == "false" ]]; then
  # Rebuild fendermint docker
  cd "${IPC_FOLDER}"/fendermint
  make clean
  make docker-build
fi

# Start the bootstrap validator node
echo "$DASHES Start the first validator node as bootstrap"
echo "First we need to force a wait to make sure the subnet is confirmed as created in the parent contracts"
echo "Wait for 30 seconds"
sleep 30
echo "Finished waiting"
cd "${IPC_FOLDER}"

bootstrap_output=$(cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=validator-0 \
    -e PRIVATE_KEY_PATH="${IPC_CONFIG_FOLDER}"/validator_0.sk \
    -e SUBNET_ID="${subnet_id}" \
    -e PARENT_ENDPOINT="${PARENT_ENDPOINT}" \
    -e CMT_P2P_HOST_PORT="${CMT_P2P_HOST_PORTS[0]}" \
    -e CMT_RPC_HOST_PORT="${CMT_RPC_HOST_PORTS[0]}" \
    -e ETHAPI_HOST_PORT="${ETHAPI_HOST_PORTS[0]}" \
    -e RESOLVER_HOST_PORT="${RESOLVER_HOST_PORTS[0]}" \
    -e OBJECTS_HOST_PORT="${OBJECTS_HOST_PORTS[0]}" \
    -e IROH_RPC_HOST_PORT="${IROH_RPC_HOST_PORTS[0]}" \
    -e FENDERMINT_METRICS_HOST_PORT="${FENDERMINT_METRICS_HOST_PORTS[0]}" \
    -e IROH_METRICS_HOST_PORT="${IROH_METRICS_HOST_PORTS[0]}" \
    -e PROMTAIL_AGENT_HOST_PORT="${PROMTAIL_AGENT_HOST_PORTS[0]}" \
    -e PROMTAIL_CONFIG_FOLDER="${IPC_CONFIG_FOLDER}" \
    -e IROH_CONFIG_FOLDER="${IPC_FOLDER}/infra/iroh/" \
    -e PARENT_HTTP_AUTH_TOKEN="${PARENT_HTTP_AUTH_TOKEN}" \
    -e PARENT_REGISTRY="${PARENT_REGISTRY_ADDRESS}" \
    -e PARENT_GATEWAY="${PARENT_GATEWAY_ADDRESS}" \
    -e FM_PULL_SKIP=1 \
    -e FM_LOG_LEVEL="info" \
    child-validator 2>&1)
echo "$bootstrap_output"
bootstrap_node_id=$(echo "$bootstrap_output" | sed -n '/CometBFT node ID:/ {n;p;}' | tr -d "[:blank:]")
bootstrap_peer_id=$(echo "$bootstrap_output" | sed -n '/IPLD Resolver Multiaddress:/ {n;p;}' | tr -d "[:blank:]" | sed 's/.*\/p2p\///')
echo "Bootstrap node started. Node id ${bootstrap_node_id}, peer id ${bootstrap_peer_id}"
# TODO: what are these endpoint?  Do they work for localnet?
bootstrap_node_endpoint=${bootstrap_node_id}@validator-0-cometbft:${CMT_P2P_HOST_PORTS[0]}
echo "Bootstrap node endpoint: ${bootstrap_node_endpoint}"
bootstrap_resolver_endpoint="/dns/validator-0-fendermint/tcp/${RESOLVER_HOST_PORTS[0]}/p2p/${bootstrap_peer_id}"
echo "Bootstrap resolver endpoint: ${bootstrap_resolver_endpoint}"

# Start other validator node
echo "$DASHES Start the other validator nodes"
cd "${IPC_FOLDER}"
for i in {1..2}
do
  cargo make --makefile infra/fendermint/Makefile.toml \
      -e NODE_NAME=validator-"${i}" \
      -e PRIVATE_KEY_PATH="${IPC_CONFIG_FOLDER}"/validator_"${i}".sk \
      -e SUBNET_ID="${subnet_id}" \
      -e PARENT_ENDPOINT="${PARENT_ENDPOINT}" \
      -e CMT_P2P_HOST_PORT="${CMT_P2P_HOST_PORTS[i]}" \
      -e CMT_RPC_HOST_PORT="${CMT_RPC_HOST_PORTS[i]}" \
      -e ETHAPI_HOST_PORT="${ETHAPI_HOST_PORTS[i]}" \
      -e RESOLVER_HOST_PORT="${RESOLVER_HOST_PORTS[i]}" \
      -e OBJECTS_HOST_PORT="${OBJECTS_HOST_PORTS[i]}" \
      -e IROH_RPC_HOST_PORT="${IROH_RPC_HOST_PORTS[i]}" \
      -e FENDERMINT_METRICS_HOST_PORT="${FENDERMINT_METRICS_HOST_PORTS[i]}" \
      -e IROH_METRICS_HOST_PORT="${IROH_METRICS_HOST_PORTS[i]}" \
      -e PROMTAIL_AGENT_HOST_PORT="${PROMTAIL_AGENT_HOST_PORTS[i]}" \
      -e PROMTAIL_CONFIG_FOLDER="${IPC_CONFIG_FOLDER}" \
      -e IROH_CONFIG_FOLDER="${IPC_FOLDER}/infra/iroh/" \
      -e RESOLVER_BOOTSTRAPS="${bootstrap_resolver_endpoint}" \
      -e BOOTSTRAPS="${bootstrap_node_endpoint}" \
      -e PARENT_HTTP_AUTH_TOKEN="${PARENT_HTTP_AUTH_TOKEN}" \
      -e PARENT_REGISTRY="${PARENT_REGISTRY_ADDRESS}" \
      -e PARENT_GATEWAY="${PARENT_GATEWAY_ADDRESS}" \
      -e FM_PULL_SKIP=1 \
      -e FM_LOG_LEVEL="info" \
      child-validator
done

# Start prometheus
cd "$IPC_FOLDER"
cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=prometheus \
    -e SUBNET_ID="$subnet_id" \
    -e PROMETHEUS_HOST_PORT="${PROMETHEUS_HOST_PORT}" \
    -e PROMETHEUS_CONFIG_FOLDER="${IPC_CONFIG_FOLDER}" \
    prometheus-start

# Start grafana
cd "$IPC_FOLDER"
cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=grafana \
    -e SUBNET_ID="$subnet_id" \
    -e GRAFANA_HOST_PORT="${GRAFANA_HOST_PORT}" \
    grafana-start

# Start loki
cd "$IPC_FOLDER"
cargo make --makefile infra/fendermint/Makefile.toml \
    -e NODE_NAME=loki \
    -e SUBNET_ID="$subnet_id" \
    -e LOKI_HOST_PORT="${LOKI_HOST_PORT}" \
    -e LOKI_CONFIG_FOLDER="${IPC_CONFIG_FOLDER}" \
    loki-start

# Test ETH API endpoint
echo "$DASHES Test ETH API endpoints of validator nodes"
for i in {0..2}
do
  curl --location http://localhost:"${ETHAPI_HOST_PORTS[i]}" \
  --header 'Content-Type: application/json' \
  --data '{
    "jsonrpc":"2.0",
    "method":"eth_blockNumber",
    "params":[],
    "id":83
  }'
done

# Test Object API endpoint
echo "$DASHES Test Object API endpoints of validator nodes"
for i in {0..2}
do
  curl --location http://localhost:"${OBJECTS_HOST_PORTS[i]}"/health
done

# Test Prometheus endpoints
printf "\n%s Test Prometheus endpoints of validator nodes\n" $DASHES
curl --location http://localhost:"${PROMETHEUS_HOST_PORT}"/graph
for i in {0..2}
do
  curl --location http://localhost:"${FENDERMINT_METRICS_HOST_PORTS[i]}"/metrics | grep succes
done

# Kill existing relayer if there's one
# TODO: does this kill all processes with relayer in the name?  This might have unintended side effects.
pkill -fe "relayer" || true
# Start relayer
echo "$DASHES Start relayer process (in the background)"
nohup ipc-cli checkpoint relayer --subnet "$subnet_id" --submitter "$default_wallet_address" > relayer.log &

# Print a summary of the deployment
cat << EOF
############################
#                          #
# IPC deployment ready! 🚀 #
#                          #
############################
Subnet ID:
$subnet_id

Chain ID:
$(curl -s --location --request POST http://localhost:"${ETHAPI_HOST_PORTS[0]}" --header 'Content-Type: application/json' --data-raw '{ "jsonrpc":"2.0", "method":"eth_chainId", "params":[], "id":1 }' | jq -r '.result' | xargs printf "%d")

Object API:
http://localhost:${OBJECTS_HOST_PORTS[0]}
http://localhost:${OBJECTS_HOST_PORTS[1]}
http://localhost:${OBJECTS_HOST_PORTS[2]}

Iroh API:
http://localhost:${IROH_RPC_HOST_PORTS[0]}
http://localhost:${IROH_RPC_HOST_PORTS[1]}
http://localhost:${IROH_RPC_HOST_PORTS[2]}

ETH API:
http://localhost:${ETHAPI_HOST_PORTS[0]}
http://localhost:${ETHAPI_HOST_PORTS[1]}
http://localhost:${ETHAPI_HOST_PORTS[2]}

CometBFT API:
http://localhost:${CMT_RPC_HOST_PORTS[0]}
http://localhost:${CMT_RPC_HOST_PORTS[1]}
http://localhost:${CMT_RPC_HOST_PORTS[2]}

Prometheus API:
http://localhost:${PROMETHEUS_HOST_PORT}

Loki API:
http://localhost:${LOKI_HOST_PORT}

Grafana API:
http://localhost:${GRAFANA_HOST_PORT}
EOF

echo "Done"
exit 0
