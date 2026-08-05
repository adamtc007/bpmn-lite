#!/usr/bin/env bash

set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/.." && pwd)
app_revision=$(git -C "$repo_root" rev-parse HEAD)
short_revision=$(git -C "$repo_root" rev-parse --short=12 HEAD)
image_ref=${1:-"bpmn-lite-designer:rc-$short_revision"}
output_dir=${2:-"$repo_root/target/release-candidate/$short_revision"}

shared_revisions=$(sed -n '/github.com\/adamtc007\/dsl/ s/.*rev = "\([^"]*\)".*/\1/p' "$repo_root/Cargo.toml" | sort -u)
shared_revision_count=$(printf '%s\n' "$shared_revisions" | sed '/^$/d' | wc -l | tr -d ' ')
if [ "$shared_revision_count" -ne 1 ]; then
  echo "expected exactly one shared DSL revision, found $shared_revision_count" >&2
  exit 1
fi
shared_revision=$shared_revisions

if [ -n "$(git -C "$repo_root" status --porcelain --untracked-files=no)" ]; then
  echo "tracked worktree changes would make the release candidate unrepeatable" >&2
  exit 1
fi

mkdir -p "$output_dir"

docker buildx build \
  --load \
  --file "$repo_root/Dockerfile.designer" \
  --tag "$image_ref" \
  --build-arg "APP_REVISION=$app_revision" \
  --build-arg "SHARED_DSL_REVISION=$shared_revision" \
  "$repo_root"

recorded_app_revision=$(docker image inspect --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' "$image_ref")
recorded_shared_revision=$(docker image inspect --format '{{ index .Config.Labels "io.bpmn-lite.shared-dsl-revision" }}' "$image_ref")
if [ "$recorded_app_revision" != "$app_revision" ] || [ "$recorded_shared_revision" != "$shared_revision" ]; then
  echo "image revision labels do not match the release inputs" >&2
  exit 1
fi

docker image inspect "$image_ref" > "$output_dir/image-inspect.json"
docker scout sbom --format cyclonedx --output "$output_dir/sbom.cdx.json" "local://$image_ref"

image_id=$(docker image inspect --format '{{.Id}}' "$image_ref")
image_size_bytes=$(docker image inspect --format '{{.Size}}' "$image_ref")
sbom_sha256=$(shasum -a 256 "$output_dir/sbom.cdx.json" | awk '{print $1}')

{
  echo "image_ref=$image_ref"
  echo "image_id=$image_id"
  echo "image_size_bytes=$image_size_bytes"
  echo "application_revision=$app_revision"
  echo "shared_dsl_revision=$shared_revision"
  echo "sbom_sha256=$sbom_sha256"
} > "$output_dir/release-receipt.env"

echo "release_candidate=$image_ref"
echo "receipt=$output_dir/release-receipt.env"
