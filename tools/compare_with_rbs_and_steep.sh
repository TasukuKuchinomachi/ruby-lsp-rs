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
    steep_*) echo steep-interface ;;
    parser_*) echo steep ;;
  esac
}

steep_tag_commit() {
  case $1 in
    2.0.0) echo 652568f78160da8cffefc6f62ac1dd6fca3d36a7 ;;
    2.1.0) echo 3b75b367c9ad3d07f8f053c0c1f28e4bab560f5d ;;
  esac
}

fetch_steep_smoke_tests() {
  version_directory=$repository/target/steep-smoke/$steep_version
  if [ -d "$version_directory/smoke" ]; then
    return
  fi
  echo "Fetching the smoke tests of Steep v$steep_version into $version_directory" >&2
  rm -rf "$version_directory/clone"
  mkdir -p "$version_directory"
  git -c advice.detachedHead=false clone --quiet --depth=1 --filter=blob:none --sparse --branch="v$steep_version" https://github.com/soutaro/steep.git "$version_directory/clone"
  git -C "$version_directory/clone" sparse-checkout set smoke
  commit=$(git -C "$version_directory/clone" rev-parse HEAD)
  if [ "$commit" != "$(steep_tag_commit "$steep_version")" ]; then
    echo "v$steep_version of Steep points to $commit, not $(steep_tag_commit "$steep_version")" >&2
    exit 1
  fi
  mv "$version_directory/clone/smoke" "$version_directory/smoke"
  rm -rf "$version_directory/clone"
}

compare() {
  label=$1
  kind=$2
  shift 2
  ruby_version_argument=
  rust_version_argument=
  case $kind in
    steep_*) ruby_version_argument=--steep-version=$steep_version ;;
  esac
  if [ "$kind" = steep_shapes ]; then
    rust_version_argument=--steep-version=$steep_version
  fi
  $ruby_command "$repository/tools/dump_$kind.rb" $ruby_version_argument "$@" > "$output/expected.txt"
  cargo run --quiet --release --manifest-path "$repository/Cargo.toml" -p "$(package_of "$kind")" --bin "dump_$kind" -- $rust_version_argument "$rbs_gem" "$@" > "$output/actual.txt"
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

kinds="rbs_environment rbs_ancestors rbs_definitions steep_shapes"
fixtures="environment ancestors definitions shapes"

for kind in $kinds; do
  compare "$kind (core)" "$kind"
  for fixture in $fixtures; do
    compare "$kind (core + fixtures/$fixture)" "$kind" "$repository/fixtures/$fixture"
  done
done

compare "steep_shapes --implicitly-returns-nil (core + fixtures/shapes)" steep_shapes --implicitly-returns-nil "$repository/fixtures/shapes"
compare "steep_subtyping (core + fixtures/subtyping)" steep_subtyping "$repository/fixtures/subtyping"
compare_parser_ast() {
  label=$1
  directory=$2
  find "$directory" -name '*.rb' -print0 | sort -z | xargs -0 $ruby_command "$repository/tools/dump_parser_ast.rb" --steep-version="$steep_version" > "$output/expected.txt"
  find "$directory" -name '*.rb' -print0 | sort -z | xargs -0 cargo run --quiet --release --manifest-path "$repository/Cargo.toml" -p "$(package_of parser_ast)" --bin dump_parser_ast -- --steep-version="$steep_version" > "$output/actual.txt"
  report "$label"
}

fetch_steep_smoke_tests
compare_parser_ast "parser_ast (Steep's smoke tests)" "$repository/target/steep-smoke/$steep_version/smoke"
compare_parser_ast "parser_ast (e2e/cases)" "$repository/e2e/cases"
compare_parser_ast "parser_ast (fixtures/parser)" "$repository/fixtures/parser"

compare_services() {
  label=$1
  kind=$2
  shift 2
  $ruby_command "$repository/tools/dump_steep_$kind.rb" --steep-version="$steep_version" "$@" > "$output/expected.txt"
  cargo run --quiet --release --manifest-path "$repository/Cargo.toml" -p steep --bin "dump_steep_$kind" -- --steep-version="$steep_version" "$@" > "$output/actual.txt"
  report "$label"
}

cd "$repository"
for kind in hover; do
  compare_services "steep_$kind (e2e/cases)" "$kind" e2e/cases/*/
done

echo "compared with rbs $rbs_version (Steep $steep_version)"
rm -r "$output"
exit $status
