#!/bin/sh
# Installs hallucinator-cli
# Usage: curl -sSf https://hallucinator.science/install-cli.sh | sh
set -e
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gianlucasb/hallucinator/releases/latest/download/hallucinator-cli-installer.sh | sh
