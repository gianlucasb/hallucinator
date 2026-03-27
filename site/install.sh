#!/bin/sh
# Installs hallucinator-tui (recommended)
# Usage: curl -sSf https://hallucinator.science/install.sh | sh
set -e
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gianlucasb/hallucinator/releases/latest/download/hallucinator-tui-installer.sh | sh
