#!/bin/bash
set -euo pipefail

# --- Auth setup ---
# Copy credentials from read-only host mount into writable config dir
mkdir -p /home/agent/.claude
if [ -f /host-auth/.credentials.json ]; then
    cp /host-auth/.credentials.json /home/agent/.claude/.credentials.json
    chmod 600 /home/agent/.claude/.credentials.json
fi
export CLAUDE_CONFIG_DIR=/home/agent/.claude

# --- Git config ---
# Set a basic git identity for commits (agent identity, not the human)
AGENT_ID="${AGENT_ID:-container-agent}"
git config --global user.name "crosslink-agent-${AGENT_ID}"
git config --global user.email "agent@crosslink.local"

# --- Toolchain detection ---
# Scan the first mounted workspace for project files
WORKSPACE=$(find /workspaces -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1)
if [ -n "$WORKSPACE" ]; then
    echo "[crosslink-entrypoint] Detected workspace: $WORKSPACE"

    if [ -f "$WORKSPACE/Cargo.toml" ] || [ -f "$WORKSPACE/crosslink/Cargo.toml" ]; then
        if ! command -v cargo &>/dev/null; then
            echo "[crosslink-entrypoint] Installing Rust toolchain..."
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --quiet 2>&1
            . "$HOME/.cargo/env"
        else
            echo "[crosslink-entrypoint] Rust toolchain already installed."
        fi
    fi

    if [ -f "$WORKSPACE/package.json" ]; then
        if ! command -v node &>/dev/null; then
            echo "[crosslink-entrypoint] Installing Node.js..."
            curl -fsSL https://deb.nodesource.com/setup_22.x | sudo bash - 2>&1
            sudo apt-get install -y --no-install-recommends nodejs 2>&1
        else
            echo "[crosslink-entrypoint] Node.js already installed."
        fi
    fi

    if [ -f "$WORKSPACE/pyproject.toml" ] || [ -f "$WORKSPACE/requirements.txt" ]; then
        if ! command -v uv &>/dev/null; then
            echo "[crosslink-entrypoint] Installing Python + uv..."
            sudo apt-get update && sudo apt-get install -y --no-install-recommends python3-pip python3-venv 2>&1
            pip install --user --break-system-packages uv 2>&1
        else
            echo "[crosslink-entrypoint] Python + uv already installed."
        fi
    fi

    if [ -f "$WORKSPACE/go.mod" ]; then
        if ! command -v go &>/dev/null; then
            echo "[crosslink-entrypoint] Installing Go..."
            curl -fsSL https://go.dev/dl/go1.23.4.linux-amd64.tar.gz | sudo tar -C /usr/local -xzf - 2>&1
            export PATH=$PATH:/usr/local/go/bin
        else
            echo "[crosslink-entrypoint] Go already installed."
        fi
    fi
fi

echo "[crosslink-entrypoint] Setup complete. Running command..."
exec "$@"
