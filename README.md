# Bunsan RPC Loadbalancer

![Dynamic TOML Badge](https://img.shields.io/badge/dynamic/toml?url=https%3A%2F%2Fraw.githubusercontent.com%2Fronnakamoto%2Fbunsan%2Frefs%2Fheads%2Fmaster%2FCargo.toml&query=%24.package.version&style=for-the-badge&label=crates.io%20-%20bunsan)

Bunsan(分散) is a high-performance, multi-chain RPC (Remote Procedure Call) load balancer designed for Ethereum and other EVM-compatible blockchain networks. It efficiently distributes incoming RPC requests across multiple nodes, ensuring optimal resource utilization and improved reliability for blockchain applications.

## Features

- **Multi-Chain Support**: Seamlessly handle requests for multiple EVM-compatible chains (e.g., Ethereum, Optimism, Arbitrum, BNB Smart Chain).
- **Flexible Chain Selection**: Support multiple methods for specifying the target chain in requests.
- **Dynamic Load Balancing**: Choose from multiple load balancing strategies (Round Robin, Least Connections, Random) for each chain.
- **Health Checking**: Continuously monitor node health and automatically route traffic to healthy nodes.
- **Configuration Hot-Reloading**: Update configuration without restarting the service.
- **Detailed Metrics**: Access real-time health and performance metrics for all configured chains and nodes.
- **Benchmarking Tools**: Evaluate and compare the performance of different load balancing strategies.

## Supported Chains
Bunsan supports the following chains out of the box:

- Ethereum Mainnet (Chain ID: 1)
- Optimism (Chain ID: 10)
- Arbitrum One (Chain ID: 42161)
- BNB Smart Chain (Chain ID: 56)
- BNB Smart Chain Testnet (Chain ID: 97)
- Sepolia Testnet (Chain ID: 11155111)
- OP Sepolia Testnet (Chain ID: 11155420)
- Arbitrum Sepolia Testnet (Chain ID: 421614)
- Base Sepolia Testnet (Chain ID: 84532)
- Base (Chain ID: 8453)
- Polygon Mainnet (Chain ID: 137)
- Polygon zkEVM (Chain ID: 1101)
- Polygon Amoy (Chain ID: 80002)
- Polygon zkEVM Testnet (Chain ID: 1442)
- Scroll (Chain ID: 534352)
- Scroll Sepolia Testnet (Chain ID: 534351)
- Taiko Mainnet (Chain ID: 167000)
- Neon EVM Mainnet (Chain ID: 245022934)
- Neon EVM Devnet (Chain ID: 245022926)

## Installation

### Using Cargo

If you have Rust installed on your system, you can globally install Bunsan via

```bash
cargo install bunsan
```

### Pre-built Binaries

You can download pre-built binaries for Bunsan from the [Releases](https://github.com/ronnakamoto/bunsan/releases) page. We provide binaries for:

- Linux (x86_64)
- Windows (x86_64)
- macOS (x86_64 / Intel)
- macOS (arm64 / Apple Silicon)

Download the appropriate binary for your system and make it executable (on Unix-based systems):

```bash
chmod +x bunsan-*
```

### Building from Source

1. Clone the repository:
   ```
   git clone https://github.com/ronnakamoto/bunsan.git
   cd bunsan
   ```

2. Build the project:
   ```
   cargo build --release
   ```

3. The compiled binary will be available at `target/release/bunsan`.

## Configuration

Bunsan uses a TOML configuration file. By default, it looks for `config.toml` in the current directory. You can specify a custom configuration file path using the `--config` option.

Example configuration:

```toml
server_addr = "127.0.0.1:8080"
update_interval = 60

[[chains]]
name = "Ethereum"
chain_id = 1
chain = "Ethereum"
load_balancing_strategy = "LeastConnections"
nodes = [
    "https://1rpc.io/eth",
    "https://eth.drpc.org",
    "https://eth.llamarpc.com",
    "https://ethereum.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Optimism"
chain_id = 10
chain = "Optimism"
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://1rpc.io/op",
    "https://optimism.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Arbitrum One"
chain_id = 42161
chain = "Arbitrum"
load_balancing_strategy = "Random"
nodes = [
    "https://1rpc.io/arb",
    "https://arbitrum.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "BNB Smart Chain Mainnet"
chain_id = 56
chain = "BNBSmartChain"
load_balancing_strategy = "LeastConnections"
nodes = ["https://1rpc.io/bnb", "https://bsc.blockpi.network/v1/rpc/public"]
```

## Usage

### Starting the Server

To start the Bunsan RPC Loadbalancer:

```
./bunsan start
```

Or with a custom configuration file:

```
./bunsan start --config /path/to/your/config.toml
```

### Command-Line Interface

Bunsan provides several CLI commands for management and monitoring:

- `start`: Start the RPC loadbalancer server
- `health`: Check the health of all configured nodes
- `config`: Display the current configuration
- `validate`: Validate the configuration file
- `nodes`: List all connected nodes and their status
- `benchmark`: Run performance benchmarks

For more information on each command, use the `--help` option:

```
./bunsan --help
```

If you plan to run the benchmarks, please make sure to first auto-generate the config by running Bunsan first via `start` option.

### Making RPC Requests

Bunsan supports multiple methods for specifying the target chain in your RPC requests:

1. **Chain-specific endpoints**: Use dedicated endpoints for each chain.
   ```
   POST http://localhost:8080/eth
   POST http://localhost:8080/op
   POST http://localhost:8080/arb
   POST http://localhost:8080/bnb
   ```

2. **General endpoint with chain parameter**: Specify the chain in the URL path.
   ```
   POST http://localhost:8080/ethereum
   POST http://localhost:8080/optimism
   POST http://localhost:8080/arbitrum
   POST http://localhost:8080/bnbsmartchain
   ```

3. **Custom header**: Use the `X-Chain-ID` header to specify the chain.
   ```
   POST http://localhost:8080
   X-Chain-ID: ethereum
   ```
4. **Default chain**: If no chain is specified, Bunsan defaults to Ethereum.
   ```
   POST http://localhost:8080
   ```

Example using curl:

```bash
curl -X POST -H "Content-Type: application/json" --data '{"jsonrpc":"2.0","method":"eth_blockNumber","params":[],"id":1}' http://localhost:8080/eth
```

## Load Balancing Strategies

Bunsan supports the following load balancing strategies:

1. **Round Robin**: Distributes requests evenly across all healthy nodes in a circular order.
2. **Least Connections**: Sends new requests to the node with the fewest active connections.
3. **Random**: Randomly selects a healthy node for each request.

You can configure different strategies for each chain in the configuration file.

## Benchmarking

To run benchmarks and compare the performance of different load balancing strategies:

```
./bunsan benchmark --duration 60 --requests-per-second 100
```

This will simulate load on all configured chains and strategies, providing detailed performance metrics.

## Monitoring and Metrics

Bunsan exposes a `/health` endpoint that provides real-time information about the status of all chains and nodes. You can access this endpoint using a GET request:

```
curl http://localhost:8080/health
```

## License

Bunsan is released under the MIT License. See the `LICENSE` file for details.

## Support

If you encounter any issues or have questions, please file an issue on the GitHub repository.

---

Thank you for using Bunsan RPC Loadbalancer!
