#!/usr/bin/env bash

set -e

KEYRING=/etc/apt/keyrings/docker.gpg

# Download Docker source keyring
mkdir -p /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/debian/gpg \
  | gpg --dearmor -o ${KEYRING}

# Set Docker apt source
echo "deb [arch=$(dpkg --print-architecture) signed-by=${KEYRING}] https://download.docker.com/linux/debian $(lsb_release -cs) stable" \
  | sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# Update sources
apt-get update
