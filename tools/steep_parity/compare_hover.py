#!/usr/bin/env python3
import argparse
import collections
import fnmatch
import json
import pathlib
import random
import re
import shlex
import subprocess
import sys
import time

from lsp_client import LanguageServerClient, LanguageServerError
from compare_diagnostics import UNEXPECTED_ERROR_CODE, parse_steep_github
from type_normalization import NORMALIZATION_RULES, method_types_return, normalize_type

TOOL_DIRECTORY = pathlib.Path(__file__).resolve().parent
KINDS = ["local_variable", "instance_variable", "block_parameter", "method_return", "method_call"]
KIND_LABELS = {
    "local_variable": "ローカル変数の代入",
    "instance_variable": "インスタンス変数の代入",
    "block_parameter": "ブロック引数",
    "method_return": "def の戻り値",
    "method_call": "メソッド呼び出し",
}
RUBY_LSP_RS_INITIALIZATION_OPTIONS = {
    "typeChecker": {"inlayHints": {"hideObviousTypes": False, "hideUntyped": False, "maxLength": None}}
}
RBS_CODE_BLOCK = re.compile(r"```rbs\n(.*?)\n```", re.DOTALL)
STEEP_VARIABLE_HOVER = re.compile(r"^\*\*(?:Local|Instance) variable\*\* `[^:`]+: (.*)`\s*$", re.DOTALL)
STEEP_NO_COMPATIBLE_METHOD = "No compatible method type found"
STEEP_INTERNAL_METHOD_TYPE = re.compile(r"\*\*Internal method type\*\*\n```rbs\n(.*?)\n```", re.DOTALL)
IDENTIFIER_METHOD_NAME = re.compile(r"^[A-Za-z_]\w*[?!=]?$")
RUBY_LSP_RS_CALL_SUMMARY = re.compile(r"^`\((.*)\) → (.*)`$", re.MULTILINE)
VARIABLE_PREFIX = re.compile(r"^[@$]{0,2}[A-Za-z_]\w*[?!=]?: (.*)$", re.DOTALL)
STEEPFILE_DIRECTIVE = re.compile(r"^\s*(check|ignore)\s+(.+?)\s*$")
STEEPFILE_STRING = re.compile(r"""['"]([^'"]+)['"]""")
STEEP_UNAVAILABLE = "(Steep の hover なし)"
RUBY_LSP_RS_UNAVAILABLE = "(ruby-lsp-rs の hover なし)"


def log(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", file=sys.stderr, flush=True)


def steepfile_patterns(project_path):
    steepfile = project_path / "Steepfile"
    checks, ignores = [], []
    if not steepfile.exists():
        return ["."], []
    for line in steepfile.read_text(encoding="utf-8").splitlines():
        match = STEEPFILE_DIRECTIVE.match(line)
        if not match:
            continue
        patterns = STEEPFILE_STRING.findall(match.group(2))
        (checks if match.group(1) == "check" else ignores).extend(patterns)
    return checks or ["."], ignores


def matches_pattern(relative_path, pattern):
    pattern = pattern.rstrip("/")
    if any(character in pattern for character in "*?["):
        return fnmatch.fnmatch(relative_path, pattern) or fnmatch.fnmatch(relative_path, pattern + "/*")
    return relative_path == pattern or relative_path.startswith(pattern + "/") or pattern == "."


def target_files(project_path):
    checks, ignores = steepfile_patterns(project_path)
    files = []
    for path in sorted(project_path.rglob("*.rb")):
        relative_path = path.relative_to(project_path).as_posix()
        if not any(matches_pattern(relative_path, pattern) for pattern in checks):
            continue
        if any(matches_pattern(relative_path, pattern) for pattern in ignores):
            continue
        files.append(relative_path)
    return files


def collect_positions(project_path, relative_paths, ruby_command):
    absolute_paths = [str(project_path / relative_path) for relative_path in relative_paths]
    output = subprocess.run(
        shlex.split(ruby_command) + [str(TOOL_DIRECTORY / "collect_positions.rb")] + absolute_paths,
        cwd=TOOL_DIRECTORY,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    raw = json.loads(output)
    return {relative_path: raw[str(project_path / relative_path)] for relative_path in relative_paths}


def hint_label(hint):
    label = hint["label"]
    if isinstance(label, list):
        label = "".join(part["value"] for part in label)
    return label


def file_uri(path):
    return pathlib.Path(path).resolve().as_uri()


def ruby_lsp_rs_type(markdown):
    if not markdown:
        return None
    summary = RUBY_LSP_RS_CALL_SUMMARY.search(markdown)
    if summary:
        return summary.group(2)
    block = RBS_CODE_BLOCK.search(markdown)
    if not block:
        return None
    content = block.group(1).strip()
    if content.startswith("def "):
        return None
    variable = VARIABLE_PREFIX.match(content)
    if variable:
        return variable.group(1)
    return content


def ruby_lsp_rs_receiver(markdown):
    summary = RUBY_LSP_RS_CALL_SUMMARY.search(markdown or "")
    return summary.group(1) if summary else None


def steep_type(markdown, kind):
    if not markdown:
        return None, "missing"
    variable = STEEP_VARIABLE_HOVER.match(markdown.strip())
    if variable:
        return variable.group(1), "ok"
    first_block = RBS_CODE_BLOCK.search(markdown)
    if STEEP_NO_COMPATIBLE_METHOD in markdown and (not first_block or markdown.index(STEEP_NO_COMPATIBLE_METHOD) < first_block.start()):
        return "(Steep: No compatible method type found)", "error"
    if not first_block:
        return None, "missing"
    content = first_block.group(1).strip()
    if content.startswith("def "):
        internal = STEEP_INTERNAL_METHOD_TYPE.search(markdown)
        method_types = internal.group(1) if internal else content.split(": ", 1)[1]
        return_type = method_types_return(method_types)
        if return_type is None:
            return content, "unparsed"
        return return_type, "ok"
    return content, "ok"


def plan_positions(relative_path, collected, hints, position_limit, random_generator):
    anchors_by_position = collections.defaultdict(list)
    for anchor in collected["anchors"]:
        anchors_by_position[(anchor["anchor"]["line"], anchor["anchor"]["character"])].append(anchor)
    positions = []
    seen = set()
    unmatched_hints = 0
    for hint in hints:
        label = hint_label(hint)
        key = (hint["position"]["line"], hint["position"]["character"])
        candidates = anchors_by_position.get(key, [])
        if label.startswith("→"):
            candidates = [anchor for anchor in candidates if anchor["kind"] == "method_return"]
            hint_type = label[1:].strip()
        elif label.startswith(":"):
            candidates = [anchor for anchor in candidates if anchor["kind"] != "method_return"]
            hint_type = label[1:].strip()
        else:
            continue
        if not candidates:
            unmatched_hints += 1
            continue
        anchor = candidates[0]
        hover = (anchor["hover"]["line"], anchor["hover"]["character"])
        if (anchor["kind"], hover) in seen:
            continue
        seen.add((anchor["kind"], hover))
        positions.append(
            {"path": relative_path, "kind": anchor["kind"], "line": hover[0], "character": hover[1], "name": anchor["name"], "hint_type": hint_type}
        )
    skipped_operator_calls = 0
    for call in collected["calls"]:
        if not IDENTIFIER_METHOD_NAME.match(call["name"]):
            skipped_operator_calls += 1
            continue
        hover = (call["hover"]["line"], call["hover"]["character"])
        if ("method_call", hover) in seen:
            continue
        seen.add(("method_call", hover))
        positions.append(
            {"path": relative_path, "kind": "method_call", "line": hover[0], "character": hover[1], "name": call["name"], "hint_type": None}
        )
    if position_limit and len(positions) > position_limit:
        positions = sorted(random_generator.sample(positions, position_limit), key=lambda position: (position["line"], position["character"]))
    return positions, unmatched_hints, skipped_operator_calls


def hover_markdown(client, uri, position, timeout):
    result = client.request(
        "textDocument/hover",
        {"textDocument": {"uri": uri}, "position": {"line": position["line"], "character": position["character"]}},
        timeout=timeout,
    )
    if not result:
        return None
    contents = result.get("contents")
    if isinstance(contents, dict):
        return contents.get("value")
    if isinstance(contents, list):
        return "\n".join(item if isinstance(item, str) else item.get("value", "") for item in contents)
    return contents


def run_ruby_lsp_rs(project_path, binary, relative_paths, collected_positions, position_limit, seed, log_directory, timings, ruby_command):
    started = time.time()
    client = LanguageServerClient([binary], str(project_path), log_path=str(log_directory / "ruby-lsp-rs.lsp.log"))
    client.request(
        "initialize",
        {
            "processId": None,
            "rootUri": file_uri(project_path),
            "workspaceFolders": [{"uri": file_uri(project_path), "name": project_path.name}],
            "capabilities": {
                "general": {"positionEncodings": ["utf-16"]},
                "window": {"workDoneProgress": True},
                "workspace": {"inlayHint": {"refreshSupport": True}, "diagnostics": {"refreshSupport": True}},
            },
            "initializationOptions": {**RUBY_LSP_RS_INITIALIZATION_OPTIONS, "rubyCommand": shlex.split(ruby_command)},
        },
    )
    client.notify("initialized", {})
    refreshed = client.wait_for_notification(lambda message: message.get("method") == "workspace/inlayHint/refresh", timeout=300)
    if not refreshed:
        log("ruby-lsp-rs: workspace/inlayHint/refresh が 300 秒以内に来なかった。そのまま続行する")
    timings["ruby_lsp_rs_ready_seconds"] = round(time.time() - started, 1)
    random_generator = random.Random(seed)
    all_positions = []
    unmatched_hints = 0
    skipped_operator_calls = 0
    for index, relative_path in enumerate(relative_paths):
        uri = file_uri(project_path / relative_path)
        text = (project_path / relative_path).read_text(encoding="utf-8")
        client.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "ruby", "version": 1, "text": text}})
        line_count = text.count("\n") + 1
        hints = client.request(
            "textDocument/inlayHint",
            {"textDocument": {"uri": uri}, "range": {"start": {"line": 0, "character": 0}, "end": {"line": line_count, "character": 0}}},
        ) or []
        positions, unmatched, skipped = plan_positions(relative_path, collected_positions[relative_path], hints, position_limit, random_generator)
        unmatched_hints += unmatched
        skipped_operator_calls += skipped
        for position in positions:
            markdown = hover_markdown(client, uri, position, timeout=60)
            position["ruby_lsp_rs_markdown"] = markdown
            position["ruby_lsp_rs_type"] = ruby_lsp_rs_type(markdown)
            position["ruby_lsp_rs_receiver"] = ruby_lsp_rs_receiver(markdown)
            position["ruby_lsp_rs_source"] = "hover"
            if position["ruby_lsp_rs_type"] is None and position["hint_type"]:
                position["ruby_lsp_rs_type"] = position["hint_type"]
                position["ruby_lsp_rs_source"] = "inlay hint"
        client.notify("textDocument/didClose", {"textDocument": {"uri": uri}})
        all_positions.extend(positions)
        if (index + 1) % 20 == 0:
            log(f"ruby-lsp-rs: {index + 1}/{len(relative_paths)} files, {len(all_positions)} positions")
    client.shutdown()
    timings["ruby_lsp_rs_seconds"] = round(time.time() - started, 1)
    return all_positions, unmatched_hints, skipped_operator_calls


def start_steep(project_path, steep_command, log_directory):
    client = LanguageServerClient(shlex.split(steep_command), str(project_path), log_path=str(log_directory / "steep.lsp.log"))
    client.request(
        "initialize",
        {
            "processId": None,
            "rootUri": file_uri(project_path),
            "workspaceFolders": [{"uri": file_uri(project_path), "name": project_path.name}],
            "capabilities": {"window": {"workDoneProgress": True}},
        },
        timeout=600,
    )
    client.notify("initialized", {})
    return client


def run_steep(client, project_path, positions, started, timings, first_hover_timeout):
    by_path = collections.defaultdict(list)
    for position in positions:
        by_path[position["path"]].append(position)
    ready = False
    for index, (relative_path, file_positions) in enumerate(by_path.items()):
        uri = file_uri(project_path / relative_path)
        text = (project_path / relative_path).read_text(encoding="utf-8")
        client.notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "ruby", "version": 1, "text": text}})
        for position in file_positions:
            try:
                markdown = hover_markdown(client, uri, position, timeout=60 if ready else first_hover_timeout)
            except LanguageServerError as error:
                log(f"steep: hover failed at {relative_path}:{position['line'] + 1}:{position['character']}: {error}")
                markdown = None
            if not ready:
                ready = True
                timings["steep_first_hover_seconds"] = round(time.time() - started, 1)
                log(f"steep: 最初の hover が返った ({timings['steep_first_hover_seconds']} 秒)")
            position["steep_markdown"] = markdown
            position["steep_type"], position["steep_status"] = steep_type(markdown, position["kind"])
        client.notify("textDocument/didClose", {"textDocument": {"uri": uri}})
        if (index + 1) % 10 == 0:
            log(f"steep: {index + 1}/{len(by_path)} files")
    client.shutdown()
    timings["steep_seconds"] = round(time.time() - started, 1)


def classify(position):
    self_type = position.get("ruby_lsp_rs_receiver") if position["kind"] == "method_call" else None
    steep_normalized = normalize_type(position["steep_type"], self_type=self_type) if position.get("steep_status") == "ok" else None
    ruby_lsp_rs_normalized = normalize_type(position["ruby_lsp_rs_type"]) if position["ruby_lsp_rs_type"] else None
    position["steep_normalized"] = steep_normalized
    position["ruby_lsp_rs_normalized"] = ruby_lsp_rs_normalized
    if position.get("steep_status") == "error":
        return "steep_error"
    if steep_normalized is None and ruby_lsp_rs_normalized is None:
        return "both_missing"
    if steep_normalized is None:
        return "steep_missing"
    if ruby_lsp_rs_normalized is None:
        return "ruby_lsp_rs_missing"
    return "match" if steep_normalized == ruby_lsp_rs_normalized else "mismatch"


def strip_optional(type_text):
    if type_text.endswith("?"):
        type_text = type_text[:-1]
        if type_text.startswith("(") and type_text.endswith(")"):
            type_text = type_text[1:-1]
    return type_text


def head_name(type_text):
    match = re.match(r"^([A-Za-z_][\w:]*)\[", type_text)
    return match.group(1) if match else None


def mismatch_pattern(steep_normalized, ruby_lsp_rs_normalized):
    if ruby_lsp_rs_normalized == "untyped":
        return "ruby-lsp-rs が untyped (Steep は型あり)"
    if steep_normalized == "untyped":
        return "Steep が untyped (ruby-lsp-rs は型あり)"
    if "untyped" in ruby_lsp_rs_normalized and "untyped" not in steep_normalized:
        return "ruby-lsp-rs の型の一部が untyped"
    if strip_optional(steep_normalized) == strip_optional(ruby_lsp_rs_normalized):
        if steep_normalized.endswith("?"):
            return "Steep だけ nil を含む (ruby-lsp-rs は nil を外している)"
        return "ruby-lsp-rs だけ nil を含む"
    if steep_normalized == "nil" or ruby_lsp_rs_normalized == "nil":
        return "片方が nil"
    if "void" in (steep_normalized, ruby_lsp_rs_normalized):
        return "片方が void"
    if steep_normalized in ("bot", "top") or ruby_lsp_rs_normalized in ("bot", "top"):
        return "片方が bot / top"
    steep_head, ruby_lsp_rs_head = head_name(strip_optional(steep_normalized)), head_name(strip_optional(ruby_lsp_rs_normalized))
    if steep_head and steep_head == ruby_lsp_rs_head:
        return f"同じ総称型 {steep_head}[...] で型引数が違う"
    if " | " in steep_normalized or " | " in ruby_lsp_rs_normalized:
        return "union の要素が違う"
    if "ActiveRecord_" in steep_normalized or "ActiveRecord_" in ruby_lsp_rs_normalized:
        return "ActiveRecord のリレーション / 関連の型が違う"
    if steep_normalized.startswith("^") or ruby_lsp_rs_normalized.startswith("^"):
        return "proc 型が違う"
    if any(character in steep_normalized + ruby_lsp_rs_normalized for character in '"\':0123456789') and (
        re.fullmatch(r'[":\'\d].*', steep_normalized) or re.fullmatch(r'[":\'\d].*', ruby_lsp_rs_normalized)
    ):
        return "リテラル型と一般の型の違い"
    return "その他 (別の型)"


def markdown_code(text):
    text = (text or "").replace("\n", " ⏎ ").replace("|", "\\|")
    if len(text) > 200:
        text = text[:200] + "…"
    fence = "``" if "`" in text else "`"
    padding = " " if text.startswith("`") or text.endswith("`") else ""
    return f"{fence}{padding}{text}{padding}{fence}"


def render_report(project_name, positions, settings, timings, unmatched_hints, skipped_operator_calls, example_limit):
    categories = ["match", "mismatch", "steep_missing", "ruby_lsp_rs_missing", "both_missing", "steep_error"]
    counts = collections.defaultdict(collections.Counter)
    for position in positions:
        counts[position["kind"]][position["category"]] += 1
        counts["all"][position["category"]] += 1
    mismatches = [position for position in positions if position["category"] == "mismatch"]
    patterns = collections.Counter(mismatch_pattern(position["steep_normalized"], position["ruby_lsp_rs_normalized"]) for position in mismatches)
    pattern_by_kind = collections.defaultdict(collections.Counter)
    for position in mismatches:
        pattern_by_kind[mismatch_pattern(position["steep_normalized"], position["ruby_lsp_rs_normalized"])][position["kind"]] += 1
    pairs = collections.Counter((position["steep_normalized"], position["ruby_lsp_rs_normalized"]) for position in mismatches)
    source_cache = {}

    def source_line(position):
        path = position["path"]
        if path not in source_cache:
            source_cache[path] = (settings["project_path"] / path).read_text(encoding="utf-8", errors="replace").splitlines()
        lines = source_cache[path]
        return lines[position["line"]].strip() if position["line"] < len(lines) else ""

    def rate(counter):
        compared = counter["match"] + counter["mismatch"]
        return f"{counter['match'] / compared * 100:.1f}%" if compared else "-"

    output = [f"# 型 (hover) の比較: {project_name}\n"]
    output.append(f"- 対象ファイル数: {settings['file_count']} (Steepfile の check 対象 {settings['target_file_count']} から seed={settings['seed']} で抽出)")
    output.append(f"- 除外したファイル数 (Steep の結果に Ruby::UnexpectedError があるもの): {settings['excluded_file_count']}")
    output.append(f"- 位置数: {len(positions)} (ファイルあたり上限 {settings['position_limit'] or 'なし'})")
    output.append(f"- ruby-lsp-rs の型 hint のうち、位置を対応付けられなかったもの: {unmatched_hints}")
    output.append(f"- 対象外にした演算子・添字呼び出し (`+`, `[]` など。Steep の hover が隣のノードを指すため): {skipped_operator_calls}")
    output.append(f"- 実行時間: {json.dumps(timings, ensure_ascii=False)}")
    output.append("")
    output.append("一致率 = 一致 / (一致 + 不一致)。片方の hover が無い位置は一致率に含めない。\n")
    output.append("## 種類別の一致率\n")
    output.append("| 種類 | 位置数 | 一致 | 不一致 | 一致率 | Steep の hover なし | ruby-lsp-rs の hover なし | 両方なし | Steep が No compatible method type |")
    output.append("|---|---:|---:|---:|---:|---:|---:|---:|---:|")
    for kind in KINDS + ["all"]:
        counter = counts[kind]
        label = KIND_LABELS.get(kind, "全体")
        output.append(
            f"| {label} | {sum(counter.values())} | {counter['match']} | {counter['mismatch']} | {rate(counter)} | "
            f"{counter['steep_missing']} | {counter['ruby_lsp_rs_missing']} | {counter['both_missing']} | {counter['steep_error']} |"
        )
    output.append("")
    output.append("## 不一致のパターン\n")
    output.append("| パターン | 件数 | 内訳 |")
    output.append("|---|---:|---|")
    for pattern, count in patterns.most_common():
        breakdown = ", ".join(f"{KIND_LABELS[kind]} {number}" for kind, number in pattern_by_kind[pattern].most_common())
        output.append(f"| {pattern} | {count} | {breakdown} |")
    output.append("")
    output.append("## 頻出する不一致の組 (上位 30)\n")
    output.append("| 件数 | Steep | ruby-lsp-rs |")
    output.append("|---:|---|---|")
    for (steep_normalized, ruby_lsp_rs_normalized), count in pairs.most_common(30):
        output.append(f"| {count} | {markdown_code(steep_normalized)} | {markdown_code(ruby_lsp_rs_normalized)} |")
    output.append("")
    output.append(f"## 不一致の例 (種類ごとに {example_limit} 件まで)\n")
    for kind in KINDS:
        kind_mismatches = [position for position in mismatches if position["kind"] == kind]
        if not kind_mismatches:
            continue
        output.append(f"\n### {KIND_LABELS[kind]} ({len(kind_mismatches)} 件)\n")
        output.append("| 場所 | 名前 | Steep | ruby-lsp-rs | ソース行 |")
        output.append("|---|---|---|---|---|")
        for position in kind_mismatches[:example_limit]:
            output.append(
                f"| `{position['path']}:{position['line'] + 1}:{position['character'] + 1}` | {markdown_code(position['name'])} | "
                f"{markdown_code(position['steep_normalized'])} | {markdown_code(position['ruby_lsp_rs_normalized'])} | {markdown_code(source_line(position))} |"
            )
    output.append("")
    for category, title in (("steep_missing", "Steep の hover が無い位置の例"), ("ruby_lsp_rs_missing", "ruby-lsp-rs の hover が無い位置の例")):
        examples = [position for position in positions if position["category"] == category]
        if not examples:
            continue
        output.append(f"## {title} ({len(examples)} 件中 {min(len(examples), example_limit)} 件)\n")
        output.append("| 場所 | 種類 | 名前 | 相手側の型 | ソース行 |")
        output.append("|---|---|---|---|---|")
        for position in examples[:example_limit]:
            other = position["ruby_lsp_rs_normalized"] if category == "steep_missing" else position["steep_normalized"]
            output.append(
                f"| `{position['path']}:{position['line'] + 1}:{position['character'] + 1}` | {KIND_LABELS[position['kind']]} | "
                f"{markdown_code(position['name'])} | {markdown_code(other)} | {markdown_code(source_line(position))} |"
            )
        output.append("")
    output.append("## 型文字列の正規化ルール\n")
    for rule in NORMALIZATION_RULES:
        output.append(f"- {rule}")
    output.append("")
    summary = {
        "files": settings["file_count"],
        "positions": len(positions),
        "by_kind": {kind: dict(counts[kind]) for kind in KINDS + ["all"]},
        "patterns": dict(patterns.most_common()),
        "timings": timings,
    }
    return "\n".join(output), summary


def main():
    parser = argparse.ArgumentParser(description="Steep と ruby-lsp-rs の hover の型を比較する")
    parser.add_argument("--project", required=True, help="対象プロジェクトのパス")
    parser.add_argument("--ruby-lsp-rs-binary", required=True)
    parser.add_argument("--steep-command", default="mise exec -- bundle exec steep langserver --jobs=2")
    parser.add_argument("--ruby-command", default="mise exec -- ruby", help="collect_positions.rb (Prism) を動かす Ruby")
    parser.add_argument("--file-limit", type=int, default=100, help="抽出するファイル数 (0 で全ファイル)")
    parser.add_argument("--position-limit", type=int, default=60, help="ファイルあたりの位置数の上限 (0 で無制限)")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--files", nargs="*", help="抽出せずに対象ファイルを直接指定する (プロジェクトからの相対パス)")
    parser.add_argument("--first-hover-timeout", type=int, default=1800, help="Steep の最初の hover (RBS 読み込み待ち) のタイムアウト秒")
    parser.add_argument(
        "--exclude-steep-output",
        help="steep check --format=github の出力。Ruby::UnexpectedError が出たファイル (Steep が検査できていない) を対象から外す",
    )
    parser.add_argument("--example-limit", type=int, default=40)
    parser.add_argument("--report", required=True)
    parser.add_argument("--summary-json")
    parser.add_argument("--positions-json", help="全位置の生データの出力先")
    parser.add_argument("--log-directory", required=True)
    arguments = parser.parse_args()

    project_path = pathlib.Path(arguments.project).resolve()
    log_directory = pathlib.Path(arguments.log_directory)
    log_directory.mkdir(parents=True, exist_ok=True)
    for previous_log in log_directory.glob("*.log*"):
        previous_log.unlink()
    timings = {}
    started = time.time()

    all_targets = target_files(project_path)
    excluded_paths = set()
    if arguments.exclude_steep_output:
        steep_output = pathlib.Path(arguments.exclude_steep_output).read_text(encoding="utf-8")
        excluded_paths = {
            diagnostic.path for diagnostic in parse_steep_github(steep_output) if diagnostic.code == UNEXPECTED_ERROR_CODE
        }
        all_targets = [path for path in all_targets if path not in excluded_paths]
    if arguments.files:
        relative_paths = arguments.files
    elif arguments.file_limit and len(all_targets) > arguments.file_limit:
        relative_paths = sorted(random.Random(arguments.seed).sample(all_targets, arguments.file_limit))
    else:
        relative_paths = all_targets
    log(f"対象 {len(relative_paths)} / {len(all_targets)} files")

    steep_started = time.time()
    steep_client = start_steep(project_path, arguments.steep_command, log_directory)
    collected_positions = collect_positions(project_path, relative_paths, arguments.ruby_command)
    positions, unmatched_hints, skipped_operator_calls = run_ruby_lsp_rs(
        project_path, arguments.ruby_lsp_rs_binary, relative_paths, collected_positions, arguments.position_limit,
        arguments.seed, log_directory, timings, arguments.ruby_command,
    )
    log(f"ruby-lsp-rs: {len(positions)} positions ({timings['ruby_lsp_rs_seconds']} 秒)")
    run_steep(steep_client, project_path, positions, steep_started, timings, arguments.first_hover_timeout)
    log(f"steep: 完了 ({timings['steep_seconds']} 秒)")
    for position in positions:
        position["category"] = classify(position)
    timings["total_seconds"] = round(time.time() - started, 1)

    settings = {
        "project_path": project_path,
        "file_count": len(relative_paths),
        "target_file_count": len(all_targets),
        "seed": arguments.seed,
        "position_limit": arguments.position_limit,
        "excluded_file_count": len(excluded_paths),
    }
    report, summary = render_report(project_path.name, positions, settings, timings, unmatched_hints, skipped_operator_calls, arguments.example_limit)
    pathlib.Path(arguments.report).write_text(report + "\n", encoding="utf-8")
    if arguments.summary_json:
        pathlib.Path(arguments.summary_json).write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if arguments.positions_json:
        pathlib.Path(arguments.positions_json).write_text(json.dumps(positions, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    all_counts = summary["by_kind"]["all"]
    log(f"hover: match={all_counts.get('match', 0)} mismatch={all_counts.get('mismatch', 0)} total={len(positions)}")


if __name__ == "__main__":
    main()
