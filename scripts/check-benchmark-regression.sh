#!/usr/bin/env bash
set -euo pipefail

output="$(cargo bench -p bpmn-lite-types --bench fiber_codec 2>&1)"
printf '%s\n' "$output"
result="$(printf '%s\n' "$output" | grep '^postcard_bytes=' | tail -1)"
if [ -z "$result" ]; then
  echo "fiber codec benchmark produced no machine-readable result" >&2
  exit 1
fi

read_metric() {
  printf '%s\n' "$1" | tr ' ' '\n' | awk -F= -v key="$2" '$1 == key { print $2 }'
}

baseline_file="benchmarks/fiber_codec.baseline"
for metric in postcard_ns_per_encode rkyv_ns_per_encode; do
  current="$(read_metric "$result" "$metric")"
  baseline="$(awk -F= -v key="$metric" '$1 == key { print $2 }' "$baseline_file")"
  if [ -z "$current" ] || [ -z "$baseline" ]; then
    echo "missing benchmark metric or baseline: $metric" >&2
    exit 1
  fi
  limit=$((baseline + baseline / 10))
  if [ "$current" -gt "$limit" ]; then
    if [ -s benchmarks/BENCHMARK_WAIVER.md ]; then
      echo "$metric regressed ($current > $limit); accepting explicit waiver"
    else
      echo "$metric regressed by more than 10% ($current > $limit)" >&2
      echo "add benchmarks/BENCHMARK_WAIVER.md with measured justification to waive" >&2
      exit 1
    fi
  fi
done
