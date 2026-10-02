#!/bin/bash
# Runs on the VM as root: installs this Compose project and runs one Studio image.
# The Hosted image workflow copies compose.yaml and this script to the working
# directory, then runs:
#
#   bash deploy.sh STUDIO_COMMIT REGISTRY_USER
#
# with two lines on stdin: a token that can pull from ghcr.io, then the tunnel token.
set -euo pipefail

studio_tag=$1
registry_user=$2
read -r registry_token
read -r tunnel_token

project_dir=/opt/spiral-robotics-studio
install -D --mode=644 compose.yaml "$project_dir/compose.yaml"
(umask 077 && printf 'STUDIO_TAG=%s\nTUNNEL_TOKEN=%s\n' "$studio_tag" "$tunnel_token" > "$project_dir/.env")

cd "$project_dir"
printf '%s' "$registry_token" | docker login ghcr.io --username "$registry_user" --password-stdin
docker compose pull
docker compose up --detach --remove-orphans
# Each deploy leaves the previous image behind.
docker image prune --all --force
