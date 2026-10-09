#!/usr/bin/env bash
set -euo pipefail

if [ $# -lt 1 ]; then
  echo "Usage: $0 <project path> [compare_hover.py の追加引数...]" >&2
  exit 2
fi

tool_directory="$(cd "$(dirname "$0")" && pwd)"
repository_directory="$(cd "$tool_directory/../.." && pwd)"
project_path="$(cd "$1" && pwd)"
shift
project_name="$(basename "$project_path")"
report_directory="$tool_directory/reports/$project_name"
ruby_lsp_rs_binary="${RUBY_LSP_RS_BINARY:-$repository_directory/target/release/ruby-lsp-rs}"
ruby_type_check_binary="${RUBY_TYPE_CHECK_BINARY:-$repository_directory/target/release/ruby-type-check}"
ruby_command="${RUBY_COMMAND:-mise exec -- ruby}"
mkdir -p "$report_directory"
timing_file="$report_directory/timing.txt"
: > "$timing_file"

record_timing() {
  echo "$1 seconds=$2" | tee -a "$timing_file" >&2
}

if [ "${SKIP_DIAGNOSTICS:-0}" != "1" ]; then
  started=$SECONDS
  type_check_arguments=(--ruby-command "$ruby_command" --severity-level hint --format line)
  compare_arguments=()
  if [ "${STEEP_COMPATIBLE:-0}" = "1" ]; then
    type_check_arguments+=(--steep-compatible)
    compare_arguments+=(--steep-compatible)
  fi
  (cd "$project_path" && "$ruby_type_check_binary" "${type_check_arguments[@]}") \
    > "$report_directory/ruby-lsp-rs.check.txt" 2> "$report_directory/ruby-lsp-rs.check.stderr" || true
  record_timing "ruby-type-check" $((SECONDS - started))

  if [ -n "${STEEP_CHECK_OUTPUT:-}" ]; then
    cp "$STEEP_CHECK_OUTPUT" "$report_directory/steep.check.txt"
  else
    started=$SECONDS
    (cd "$project_path" && mise exec -- bundle exec steep check --format=github --severity-level=hint) \
      > "$report_directory/steep.check.txt" 2> "$report_directory/steep.check.stderr" || true
    record_timing "steep check" $((SECONDS - started))
  fi

  started=$SECONDS
  python3 "$tool_directory/compare_diagnostics.py" \
    --project "$project_path" \
    --steep-output "$report_directory/steep.check.txt" \
    --ruby-lsp-rs-output "$report_directory/ruby-lsp-rs.check.txt" \
    --ruby-lsp-rs-source "$repository_directory" \
    --report "$report_directory/diagnostics.md" \
    --summary-json "$report_directory/diagnostics.json" \
    ${compare_arguments[@]+"${compare_arguments[@]}"}
  record_timing "compare diagnostics" $((SECONDS - started))
fi

if [ "${SKIP_HOVER:-0}" != "1" ]; then
  exclude_arguments=()
  if [ -s "$report_directory/steep.check.txt" ]; then
    exclude_arguments=(--exclude-steep-output "$report_directory/steep.check.txt")
  fi
  started=$SECONDS
  python3 "$tool_directory/compare_hover.py" \
    --project "$project_path" \
    --ruby-lsp-rs-binary "$ruby_lsp_rs_binary" \
    --ruby-command "$ruby_command" \
    --report "$report_directory/hover.md" \
    --summary-json "$report_directory/hover.json" \
    --positions-json "$report_directory/hover.positions.json" \
    --log-directory "$report_directory/logs" \
    ${exclude_arguments[@]+"${exclude_arguments[@]}"} \
    "$@"
  record_timing "compare hover" $((SECONDS - started))
fi

echo "reports: $report_directory" >&2
