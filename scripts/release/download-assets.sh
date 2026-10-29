#!/usr/bin/env bash
# Downloads every asset of a GitHub release into a folder, under its asset
# name, through the API by asset id (which also works for a draft release).
#
#   bash scripts/release/download-assets.sh RELEASE_JSON DEST
#
# RELEASE_JSON is the release's API record (gh api repos/OWNER/REPO/releases/ID).
# Needs GH_TOKEN and GITHUB_REPOSITORY. verify.mjs then checks each file's
# size against the record.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 RELEASE_JSON DEST" >&2
  exit 2
fi
release_json=$1
dest=$2
repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}
mkdir -p "$dest"

jq -r '.assets[] | "\(.id)\t\(.name)"' "$release_json" | while IFS=$'\t' read -r id name; do
  case "$name" in
    '' | . | .. | */* | *\\*)
      echo "::error::Release asset ${id} has an unsafe name: ${name}"
      exit 1
      ;;
  esac
  gh api -H 'Accept: application/octet-stream' "repos/${repo}/releases/assets/${id}" > "${dest}/${name}"
  printf '%12s  %s\n' "$(wc -c < "${dest}/${name}" | tr -d ' ')" "$name"
done
