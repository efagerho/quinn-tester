#!/bin/bash
set -euo pipefail

sudo systemctl stop firewalld
sudo nft flush ruleset
echo "Firewall disabled."
