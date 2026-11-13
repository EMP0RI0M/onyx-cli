# Onyx CLI (`onyx`)

> Autonomous agentic engineering CLI built with Rust. Features deterministic BM25 capability retrieval, low-latency ReAct loop, sandboxed bash execution, interactive terminal UI (TUI), and multi-agent coordination.

[![Release](https://github.com/EMP0RI0M/onyx-cli/actions/workflows/release.yml/badge.svg)](https://github.com/EMP0RI0M/onyx-cli/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

---

## Quick Installation

### Linux & macOS (curl / bash)
Supports **Linux (x86_64, aarch64/ARM64, armv7/ARM32)** and **macOS (Apple Silicon & Intel)**:

```bash
curl -fsSL https://raw.githubusercontent.com/EMP0RI0M/onyx-cli/main/install.sh | bash
```

### Windows (PowerShell)
Supports **Windows x86_64 & Windows ARM64**:

```powershell
irm https://raw.githubusercontent.com/EMP0RI0M/onyx-cli/main/install.ps1 | iex
```

### From Source (Cargo)
```bash
git clone https://github.com/EMP0RI0M/onyx-cli.git
cd onyx-cli
cargo build --release -p onyx-cli
sudo cp target/release/onyx /usr/local/bin/
```

---

## Features

- **Interactive Terminal UI (TUI):** Built on ratatui/crossterm with collapsible thinking blocks (`ctrl+o`), streaming Markdown rendering, diff inspection, and real-time status bars.
- **Deterministic BM25 Capability Retrieval:** Narrows large tool catalogs (100+ tools and skills) down to top-$k$ relevant capabilities in sub-millisecond latencies ($p95 < 200\,\mu\text{s}$) with zero network dependencies.
- **Safe Sandboxing & Audit:** Native Linux namespaces and seccomp sandboxing for bash execution, coupled with type-erased policy gateways and structured approval requests.
- **Autonomous Task Engine:** Checkpoint recovery (`--resume <task-id>`), work memory compilation, and project architecture grounding.
- **Subagents & Swarm:** Parallel thread-isolated subagent spawning with depth-guarded recursion limits.

---

## Cross-Platform Prebuilt Binaries

Precompiled releases are automatically published for every tagged release:

| Platform | Architecture | Artifact Name |
| :--- | :--- | :--- |
| **Linux** | x86_64 / AMD64 | `onyx-linux-amd64.tar.gz` |
| **Linux** | ARM64 / aarch64 | `onyx-linux-arm64.tar.gz` |
| **Linux** | ARM32 / armv7 | `onyx-linux-arm32.tar.gz` |
| **macOS** | Apple Silicon (M1/M2/M3) | `onyx-darwin-arm64.tar.gz` |
| **macOS** | Intel x86_64 | `onyx-darwin-amd64.tar.gz` |
| **Windows** | x86_64 / AMD64 | `onyx-windows-amd64.exe.tar.gz` |
| **Windows** | ARM64 | `onyx-windows-arm64.exe.tar.gz` |

---

## Usage

```bash
# Launch interactive TUI
onyx

# Run one-shot query
onyx --print "Explain the project architecture"

# Resume an autonomous checkpoint
onyx --resume task-123456

# Enable auto-approval mode
onyx --allow-all
```

---

## Configuration & Environment Variables

- `ONYX_BASE_URL`: OpenAI-compatible API base URL (default: `http://127.0.0.1:20128/v1`).
- `ONYX_API_KEY`: API authentication key.
- `ONYX_MODEL`: Model name identifier.
- `ONYX_RETRIEVAL`: Capability retrieval mode (`bm25` by default, or `0`/`off` to disable).
- `ONYX_USAGE_RANKING`: Set to `1` to enable graph-based adaptive tool ranking.
- `ONYX_SANDBOX`: Set to `1` to enforce Linux kernel sandboxing on shell commands.

---

## Contributors

- [Tanay Soni](https://github.com/TNInjas)

---

## License

MIT License. See [LICENSE](LICENSE) for details.
