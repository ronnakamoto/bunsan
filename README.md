# Bunsan RPC Loadbalancer

Bunsan is a high-performance, multi-chain RPC (Remote Procedure Call) load balancer designed for Ethereum and other EVM-compatible blockchain networks. It efficiently distributes incoming RPC requests across multiple nodes, ensuring optimal resource utilization and improved reliability for blockchain applications.

## Features

- **Multi-Chain Support**: Seamlessly handle requests for multiple EVM-compatible chains (e.g., Ethereum, Optimism, Arbitrum, Base, BNB Chain).
- **Dynamic Load Balancing**: Choose from multiple load balancing strategies (Round Robin, Least Connections, Random) for each chain.
- **Health Checking**: Continuously monitor node health and automatically route traffic to healthy nodes.
- **Configuration Hot-Reloading**: Update configuration without restarting the service.
- **Detailed Metrics**: Access real-time health and performance metrics for all configured chains and nodes.
- **Benchmarking Tools**: Evaluate and compare the performance of different load balancing strategies.

## Installation

### Prerequisites

- Rust 1.54 or later
- Cargo (Rust's package manager)

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
load_balancing_strategy = "RoundRobin"
nodes = [
    "https://1rpc.io/op",
    "https://optimism.blockpi.network/v1/rpc/public",
]

[[chains]]
name = "Arbitrum One"
chain_id = 42161
load_balancing_strategy = "Random"
nodes = [
    "https://1rpc.io/arb",
    "https://arbitrum.blockpi.network/v1/rpc/public",
]
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
