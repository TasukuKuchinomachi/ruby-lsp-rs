#!/bin/sh
set -eu

repository=$(cd "$(dirname "$0")/.." && pwd)
ruby_command=${RUBY_COMMAND:-ruby}
rbs_version=4.2.0
steep_version=2.1.0
for argument in "$@"; do
  case $argument in
    --steep-version=2.0.0|--steep-version=2.1.0) steep_version=${argument#*=} ;;
    *)
      echo "Usage: tools/compare_with_rbs_and_steep.sh [--steep-version=2.0.0|2.1.0]" >&2
      exit 2
      ;;
  esac
done
output=$(mktemp -d)
rbs_gem=$($ruby_command -e "gem \"rbs\", \"$rbs_version\"; puts Gem.loaded_specs[\"rbs\"].full_gem_path")
status=0

package_of() {
  case $1 in
    rbs_*) echo rbs-definition ;;
  esac
}

compare() {
  label=$1
  kind=$2
  shift 2
  $ruby_command "$repository/tools/dump_$kind.rb" "$@" > "$output/expected.txt"
  cargo run --quiet --release --manifest-path "$repository/Cargo.toml" -p "$(package_of "$kind")" --bin "dump_$kind" -- "$rbs_gem" "$@" > "$output/actual.txt"
  report "$label"
}

report() {
  if diff "$output/expected.txt" "$output/actual.txt" > /dev/null; then
    echo "same: $1 ($(wc -l < "$output/expected.txt" | tr -d ' ') lines)"
    return
  fi
  echo "different: $1"
  diff -u "$output/expected.txt" "$output/actual.txt" | head -40
  status=1
}

kinds="rbs_environment rbs_ancestors rbs_definitions"
fixtures="environment ancestors definitions"

for kind in $kinds; do
  compare "$kind (core)" "$kind"
  for fixture in $fixtures; do
    compare "$kind (core + fixtures/$fixture)" "$kind" "$repository/fixtures/$fixture"
  done
done

echo "compared with rbs $rbs_version (Steep $steep_version)"
rm -r "$output"
exit $status
