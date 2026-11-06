#!/bin/sh
set -eu

repository=$(cd "$(dirname "$0")/.." && pwd)
helpers="$repository/tools/compare_project_diagnostics"
steep_version=2.1.0
rbs_version=4.2.0

usage() {
  cat <<USAGE
Usage: tools/compare_project_diagnostics.sh [--steep-version=VERSION] [--project-directory=DIRECTORY] [--ruby-type-check-steepfile=STEEPFILE] [--name=NAME] [--diagnostics=PRESET]

Type check a project with the real Steep VERSION (2.0.0 or 2.1.0, default $steep_version) with
rbs $rbs_version and with \`ruby-type-check check --steep-version=VERSION\`, and compare the diagnostics.
Without --project-directory, Steep's own repository at the tag of VERSION is fetched and compared.
With --diagnostics, every target and group of both Steepfiles uses the preset PRESET of
Steep::Diagnostic::Ruby (all_error, strict, default, lenient or silent).
Results are written in target/compare-project-diagnostics/NAME/, or in
target/compare-project-diagnostics/NAME/diagnostics-PRESET/ with --diagnostics.
USAGE
}

project_directory=
ruby_type_check_steepfile=
name=
diagnostics=
for argument in "$@"; do
  case $argument in
    --steep-version=2.0.0|--steep-version=2.1.0) steep_version=${argument#*=} ;;
    --project-directory=*) project_directory=${argument#*=} ;;
    --ruby-type-check-steepfile=*) ruby_type_check_steepfile=${argument#*=} ;;
    --name=*) name=${argument#*=} ;;
    --diagnostics=*)
      diagnostics=${argument#*=}
      case $diagnostics in
        all_error|strict|default|lenient|silent) ;;
        *) usage >&2; exit 2 ;;
      esac
      ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

steep_tag=v$steep_version
case $steep_version in
  2.0.0) steep_tag_commit=652568f78160da8cffefc6f62ac1dd6fca3d36a7 ;;
  2.1.0) steep_tag_commit=3b75b367c9ad3d07f8f053c0c1f28e4bab560f5d ;;
esac
activate_gems="gem \"rbs\", \"$rbs_version\"; gem \"steep\", \"$steep_version\""

work_root="$repository/target/compare-project-diagnostics"

if [ -z "$project_directory" ]; then
  name=${name:-steep-$steep_version}
  project_directory="$work_root/$name/project"
  ruby_type_check_steepfile=${ruby_type_check_steepfile:-$helpers/steep/Steepfile}
  if [ ! -d "$project_directory" ]; then
    echo "Fetching Steep $steep_tag into $project_directory" >&2
    rm -rf "$project_directory.partial"
    mkdir -p "$work_root/$name"
    git clone --quiet --depth=1 --branch="$steep_tag" https://github.com/soutaro/steep.git "$project_directory.partial"
    commit=$(git -C "$project_directory.partial" rev-parse HEAD)
    if [ "$commit" != "$steep_tag_commit" ]; then
      echo "$steep_tag of Steep points to $commit, not $steep_tag_commit" >&2
      exit 1
    fi
    mv "$project_directory.partial" "$project_directory"
  fi
fi

project_directory=$(cd "$project_directory" && pwd)
name=${name:-$(basename "$project_directory")}
output="$work_root/$name"
if [ -n "$diagnostics" ]; then
  output="$output/diagnostics-$diagnostics"
fi
ruby_type_check_project="$output/ruby-type-check-project"
steep_project="$output/steep-project"
mkdir -p "$output"

if ! ruby -e "$activate_gems" 2>/dev/null; then
  echo "\`ruby\` in PATH cannot load steep $steep_version with rbs $rbs_version (try \`mise exec ruby@3.4.11 -- $0\`)" >&2
  exit 1
fi
rbs_directory=$(ruby -e "$activate_gems; print Gem.loaded_specs[\"rbs\"].gem_dir")

cargo build --quiet --release --manifest-path "$repository/Cargo.toml" -p steep
ruby_type_check="${CARGO_TARGET_DIR:-$repository/target}/release/ruby-type-check"

seconds() {
  ruby -e 'print Process.clock_gettime(Process::CLOCK_MONOTONIC).round(1)'
}

elapsed() {
  ruby -e "print (ARGV[1].to_f - ARGV[0].to_f).round(1)" "$1" "$2"
}

for configuration in "$project_directory"/rbs_collection*.yaml; do
  case $configuration in
    *.lock.yaml|*'*'*) continue ;;
  esac
  echo "Installing the rbs collection of $(basename "$configuration")" >&2
  (cd "$project_directory" && ruby -e "$activate_gems; load Gem.bin_path(\"rbs\", \"rbs\", \"$rbs_version\")" -- --collection "$configuration" collection install --frozen > "$output/rbs-collection-install.log" 2>&1) || {
    cat "$output/rbs-collection-install.log" >&2
    exit 1
  }
done

copy_project() {
  rm -rf "$1"
  mkdir -p "$1"
  (cd "$project_directory" && tar --exclude=./.git -cf - .) | (cd "$1" && tar -xf -)
}

: > "$output/override-diagnostics.log"
override_diagnostics() {
  if [ -n "$diagnostics" ]; then
    ruby "$helpers/override_diagnostics.rb" "$1" "$diagnostics" >> "$output/override-diagnostics.log" 2>&1 || {
      cat "$output/override-diagnostics.log" >&2
      exit 1
    }
  fi
}

steep_directory=$project_directory
if [ -n "$diagnostics" ]; then
  copy_project "$steep_project"
  override_diagnostics "$steep_project/Steepfile"
  steep_directory=$steep_project
fi

echo "Type checking with Steep $steep_version (one worker)" >&2
rm -f "$output/steep.yml"
steep_start=$(seconds)
(cd "$steep_directory" && ruby -rbundler -e "$activate_gems; load Gem.bin_path(\"steep\", \"steep\", \"$steep_version\")" -- check --jobs=1 --save-expectations="$output/steep.yml" > "$output/steep.log" 2>&1) || true
steep_end=$(seconds)
if [ ! -f "$output/steep.yml" ]; then
  cat "$output/steep.log" >&2
  echo "Steep did not save the expectations" >&2
  exit 1
fi

copy_project "$ruby_type_check_project"
override_diagnostics "$ruby_type_check_project/Steepfile"

: > "$output/unsupported-by-ruby-type-check.txt"
ruby_type_check_seconds=

run_ruby_type_check() {
  stage=$1
  rm -f "$output/ruby-type-check.yml"
  ruby_type_check_start=$(seconds)
  (cd "$ruby_type_check_project" && RBS_GEM_DIRECTORY="$rbs_directory" "$ruby_type_check" check --steep-version="$steep_version" --save-expectations="$output/ruby-type-check.yml" 2> "$output/ruby-type-check-$stage.log" > /dev/null) || true
  ruby_type_check_end=$(seconds)
  if [ -f "$output/ruby-type-check.yml" ] && ! grep -q '^ruby-type-check: cannot load signatures of target' "$output/ruby-type-check-$stage.log"; then
    ruby_type_check_seconds=$(elapsed "$ruby_type_check_start" "$ruby_type_check_end")
    return 0
  fi
  {
    echo "ruby-type-check cannot check the project ($stage):"
    sed 's/^/  /' "$output/ruby-type-check-$stage.log"
  } >> "$output/unsupported-by-ruby-type-check.txt"
  return 1
}

adapt() {
  description=$1
  shift
  echo "Adapting the project for ruby-type-check: $description" >> "$output/unsupported-by-ruby-type-check.txt"
  if ! "$@" > "$output/adapt.log" 2>&1; then
    cat "$output/adapt.log" >&2
    exit 1
  fi
  sed 's/^/  /' "$output/adapt.log" >> "$output/unsupported-by-ruby-type-check.txt"
}

replace_steepfile() {
  cp "$ruby_type_check_steepfile" "$ruby_type_check_project/Steepfile"
  override_diagnostics "$ruby_type_check_project/Steepfile"
}

adapt_and_run_ruby_type_check() {
  next_stage=$1
  shift
  adapt "$@"
  run_ruby_type_check "$next_stage"
}

echo "Type checking with ruby-type-check" >&2
run_ruby_type_check original ||
  { [ -n "$ruby_type_check_steepfile" ] && adapt_and_run_ruby_type_check steepfile-replaced "use $ruby_type_check_steepfile as the Steepfile" replace_steepfile; } ||
  {
    cat "$output/unsupported-by-ruby-type-check.txt" >&2
    exit 1
  }

status=0
ruby "$helpers/compare_expectations.rb" "$output/steep.yml" "$output/ruby-type-check.yml" "$steep_directory" "$ruby_type_check_project" > "$output/differences.txt" || status=$?
if [ "$status" -gt 1 ]; then
  exit "$status"
fi
if grep -q 'failed inside ruby-type-check, so the result above is incomplete' "$output/ruby-type-check-$stage.log"; then
  status=1
fi

{
  if [ -n "$diagnostics" ]; then
    echo "Every target and group uses Steep::Diagnostic::Ruby.$diagnostics:"
    sed "s|^$output/|  |" "$output/override-diagnostics.log"
    echo
  fi
  if [ -s "$output/unsupported-by-ruby-type-check.txt" ]; then
    cat "$output/unsupported-by-ruby-type-check.txt"
    echo
  fi
  echo "ruby-type-check reported on the standard error ($output/ruby-type-check-$stage.log):"
  grep '^ruby-type-check: ' "$output/ruby-type-check-$stage.log" |
    sed -e 's/ ([^()]*:[0-9][^()]*)//' \
      -e 's/^ruby-type-check: type checking [^ ]* failed: /ruby-type-check: type checking a file failed: /' |
    sort | uniq -c | sort -rn || true
  echo "  $(grep -c 'panicked at' "$output/ruby-type-check-$stage.log" || true) panics"
  echo
  echo "Files ruby-type-check failed to type check (no diagnostics are compared in them):"
  sed -n 's/^ruby-type-check: type checking \([^ ]*\) failed: .*/  \1/p' "$output/ruby-type-check-$stage.log" | sort
  echo
  echo "Steep took $(elapsed "$steep_start" "$steep_end") seconds, ruby-type-check took $ruby_type_check_seconds seconds"
  echo
  cat "$output/differences.txt"
} > "$output/report.txt"

cat "$output/report.txt"
exit "$status"
