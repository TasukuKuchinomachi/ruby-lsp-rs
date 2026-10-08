#!/usr/bin/env python3
import argparse
import collections
import dataclasses
import json
import pathlib
import re
import sys

from type_normalization import normalize_message

STEEP_GITHUB_LINE = re.compile(
    r"^::(?P<severity>\w+) file=(?P<path>[^,]+),line=(?P<line>\d+),endLine=(?P<end_line>\d+),"
    r"col=(?P<column>\d+),endColumn=(?P<end_column>\d+)::\[(?P<code>[^\]]+)\] (?P<message>.*)$"
)
RUBY_LSP_RS_LINE_START = re.compile(
    r"^(?P<path>\S[^:]*\.rbs?):(?P<line>\d+):(?P<column>\d+): \[(?P<severity>\w+)\] (?P<rest>.*)$"
)
RUBY_LSP_RS_CODE_SUFFIX = re.compile(r"^(?P<message>.*) \((?P<code>[A-Z]\w*(?:::\w+)+)\)$", re.DOTALL)
GITHUB_UNESCAPES = {"%0A": "\n", "%0D": "\r", "%25": "%"}

RUBY_LSP_RS_SEVERITY_TO_GITHUB = {"error": "error", "warning": "warning", "information": "notice", "hint": "notice"}
UNEXPECTED_ERROR_CODE = "Ruby::UnexpectedError"
GITHUB_SEVERITY_ORDER = ["error", "warning", "notice"]
BLOCKING_SEVERITIES = ("error", "warning")
SOURCE_SUFFIXES = (".rb", ".rbs")
IMPLEMENTED_CODE_LITERAL = re.compile(r'"(Ruby::[A-Za-z]+)"')


@dataclasses.dataclass
class Diagnostic:
    path: str
    line: int
    column: int
    code: str
    severity: str
    message: str


def github_unescape(text):
    return re.sub("%0A|%0D|%25", lambda match: GITHUB_UNESCAPES[match.group(0)], text)


def parse_steep_github(text):
    diagnostics = []
    for line in text.splitlines():
        match = STEEP_GITHUB_LINE.match(line)
        if not match:
            continue
        diagnostics.append(
            Diagnostic(
                path=match["path"],
                line=int(match["line"]),
                column=int(match["column"]) + 1,
                code=match["code"],
                severity=match["severity"],
                message=github_unescape(match["message"]),
            )
        )
    return diagnostics


def parse_ruby_lsp_rs(text):
    diagnostics = []
    pending = None
    unparsed_lines = []

    def finish(pending_record):
        match = RUBY_LSP_RS_CODE_SUFFIX.match(pending_record["rest"])
        if match:
            code, message = match["code"], match["message"]
        else:
            code, message = "(unknown)", pending_record["rest"]
        diagnostics.append(
            Diagnostic(
                path=pending_record["path"],
                line=int(pending_record["line"]),
                column=int(pending_record["column"]),
                code=code,
                severity=RUBY_LSP_RS_SEVERITY_TO_GITHUB.get(pending_record["severity"], pending_record["severity"]),
                message=message,
            )
        )

    for line in text.splitlines():
        match = RUBY_LSP_RS_LINE_START.match(line)
        if match:
            if pending:
                finish(pending)
            pending = match.groupdict()
        elif pending and not RUBY_LSP_RS_CODE_SUFFIX.match(pending["rest"]):
            pending["rest"] += "\n" + line
        elif line.strip():
            unparsed_lines.append(line)
    if pending:
        finish(pending)
    return diagnostics, unparsed_lines


def match_diagnostics(steep_diagnostics, ruby_lsp_rs_diagnostics):
    def key(diagnostic):
        return (diagnostic.path, diagnostic.line, diagnostic.column, diagnostic.code)

    steep_by_key = collections.defaultdict(list)
    for diagnostic in steep_diagnostics:
        steep_by_key[key(diagnostic)].append(diagnostic)

    exact_matches = []
    ruby_lsp_rs_remaining = []
    for diagnostic in ruby_lsp_rs_diagnostics:
        candidates = steep_by_key.get(key(diagnostic))
        if candidates:
            exact_matches.append((candidates.pop(0), diagnostic))
        else:
            ruby_lsp_rs_remaining.append(diagnostic)
    steep_remaining = [diagnostic for candidates in steep_by_key.values() for diagnostic in candidates]

    steep_by_line = collections.defaultdict(list)
    for diagnostic in steep_remaining:
        steep_by_line[(diagnostic.path, diagnostic.line, diagnostic.code)].append(diagnostic)
    shifted_matches = []
    ruby_lsp_rs_only = []
    for diagnostic in ruby_lsp_rs_remaining:
        candidates = steep_by_line.get((diagnostic.path, diagnostic.line, diagnostic.code))
        if candidates:
            candidates.sort(key=lambda candidate: abs(candidate.column - diagnostic.column))
            shifted_matches.append((candidates.pop(0), diagnostic))
        else:
            ruby_lsp_rs_only.append(diagnostic)
    steep_only = [diagnostic for candidates in steep_by_line.values() for diagnostic in candidates]
    return exact_matches, shifted_matches, ruby_lsp_rs_only, steep_only


class SourceLines:
    def __init__(self, project_path):
        self.project_path = pathlib.Path(project_path)
        self.cache = {}

    def line(self, path, line_number):
        if not path.endswith(SOURCE_SUFFIXES):
            return ""
        if path not in self.cache:
            try:
                self.cache[path] = (self.project_path / path).read_text(encoding="utf-8", errors="replace").splitlines()
            except OSError:
                self.cache[path] = []
        lines = self.cache[path]
        if 1 <= line_number <= len(lines):
            return lines[line_number - 1]
        return ""


def markdown_code(text):
    text = text.replace("\n", " ⏎ ")
    fence = "``" if "`" in text else "`"
    padding = " " if text.startswith("`") or text.endswith("`") else ""
    return f"{fence}{padding}{text}{padding}{fence}"


def source_snippet(source_lines, diagnostic):
    line_text = source_lines.line(diagnostic.path, diagnostic.line)
    stripped = line_text.strip()
    if len(stripped) > 160:
        stripped = stripped[:160] + "…"
    return stripped


def implemented_codes_from_source(source_directory):
    codes = set()
    if not source_directory:
        return codes
    for path in pathlib.Path(source_directory).rglob("*.rs"):
        if "target" in path.parts:
            continue
        codes.update(IMPLEMENTED_CODE_LITERAL.findall(path.read_text(encoding="utf-8", errors="replace")))
    return codes


def exclude_unexpected_error_files(steep_diagnostics, ruby_lsp_rs_diagnostics):
    excluded_paths = {diagnostic.path for diagnostic in steep_diagnostics if diagnostic.code == UNEXPECTED_ERROR_CODE}
    return (
        [diagnostic for diagnostic in steep_diagnostics if diagnostic.path not in excluded_paths],
        [diagnostic for diagnostic in ruby_lsp_rs_diagnostics if diagnostic.path not in excluded_paths],
        excluded_paths,
        sum(1 for diagnostic in ruby_lsp_rs_diagnostics if diagnostic.path in excluded_paths),
    )


UNIMPLEMENTED_EXAMPLE_LIMIT = 5


def render_report(project_name, steep_diagnostics, ruby_lsp_rs_diagnostics, unparsed_lines, results, source_lines,
                  implemented_codes, excluded_paths, excluded_ruby_lsp_rs_count):
    exact_matches, shifted_matches, ruby_lsp_rs_only, steep_only = results
    codes = sorted(
        {diagnostic.code for diagnostic in steep_diagnostics} | {diagnostic.code for diagnostic in ruby_lsp_rs_diagnostics}
    )
    implemented_codes = implemented_codes | {diagnostic.code for diagnostic in ruby_lsp_rs_diagnostics}
    steep_only_implemented = [diagnostic for diagnostic in steep_only if diagnostic.code in implemented_codes]
    steep_only_unimplemented = [diagnostic for diagnostic in steep_only if diagnostic.code not in implemented_codes]
    counters = {
        "exact": collections.Counter(steep.code for steep, _ in exact_matches),
        "shifted": collections.Counter(steep.code for steep, _ in shifted_matches),
        "ruby_lsp_rs_only": collections.Counter(diagnostic.code for diagnostic in ruby_lsp_rs_only),
        "steep_only": collections.Counter(diagnostic.code for diagnostic in steep_only),
        "steep_total": collections.Counter(diagnostic.code for diagnostic in steep_diagnostics),
        "ruby_lsp_rs_total": collections.Counter(diagnostic.code for diagnostic in ruby_lsp_rs_diagnostics),
    }
    steep_severities = collections.defaultdict(collections.Counter)
    for diagnostic in steep_diagnostics:
        steep_severities[diagnostic.code][diagnostic.severity] += 1
    message_mismatches = [
        (steep, ruby_lsp_rs)
        for steep, ruby_lsp_rs in exact_matches + shifted_matches
        if normalize_message(steep.message) != normalize_message(ruby_lsp_rs.message)
    ]
    severity_mismatches = collections.Counter(
        (steep.code, steep.severity, ruby_lsp_rs.severity)
        for steep, ruby_lsp_rs in exact_matches + shifted_matches
        if steep.severity != ruby_lsp_rs.severity
    )

    def severity_summary(code):
        return ", ".join(f"{severity} {count}" for severity, count in sorted(steep_severities[code].items())) or "-"

    missed_blocking = [diagnostic for diagnostic in steep_only if diagnostic.severity in BLOCKING_SEVERITIES]
    matched_pairs = exact_matches + shifted_matches
    severity_rows = []
    for severity in GITHUB_SEVERITY_ORDER:
        steep_count = sum(1 for diagnostic in steep_diagnostics if diagnostic.severity == severity)
        pairs = [(steep, ruby_lsp_rs) for steep, ruby_lsp_rs in matched_pairs if steep.severity == severity]
        same = sum(1 for steep, ruby_lsp_rs in pairs if ruby_lsp_rs.severity == severity)
        missed = sum(1 for diagnostic in steep_only if diagnostic.severity == severity)
        ruby_lsp_rs_only_count = sum(1 for diagnostic in ruby_lsp_rs_only if diagnostic.severity == severity)
        severity_rows.append((severity, steep_count, len(pairs), same, len(pairs) - same, missed, ruby_lsp_rs_only_count))

    output = []
    output.append(f"# 診断の比較: {project_name}\n")
    output.append("## 最重要指標: Steep の error / warning のうち ruby-lsp-rs が出していないもの\n")
    output.append(
        f"**{len(missed_blocking)} 件** (Steep の error / warning は {sum(row[1] for row in severity_rows if row[0] in BLOCKING_SEVERITIES)} 件。"
        "同じ行・同じ code で column だけ違うものは出しているとみなす。ruby-lsp-rs が Steep より厳しいのは許容)\n"
    )
    if missed_blocking:
        output.append("| code | ruby-lsp-rs | Steep の severity | 件数 |")
        output.append("|---|---|---|---:|")
        missed_by_code = collections.Counter((diagnostic.code, diagnostic.severity) for diagnostic in missed_blocking)
        for (code, severity), count in sorted(missed_by_code.items(), key=lambda item: (-item[1], item[0])):
            status = "実装済み" if code in implemented_codes else "未実装"
            output.append(f"| {code} | {status} | {severity} | {count} |")
        output.append("\n個別の一覧は後半の「Steep だけが出した」の節にある。\n")
    output.append("## severity 別の集計 (github 形式の名前。ruby-lsp-rs の information / hint は notice に読み替える)\n")
    output.append("| Steep の severity | Steep | ruby-lsp-rs も出した | うち severity 一致 | うち severity 不一致 | ruby-lsp-rs が出していない | ruby-lsp-rs だけが出した (ruby-lsp-rs の severity) |")
    output.append("|---|---:|---:|---:|---:|---:|---:|")
    for severity, steep_count, matched_count, same, different, missed, ruby_lsp_rs_only_count in severity_rows:
        output.append(f"| {severity} | {steep_count} | {matched_count} | {same} | {different} | {missed} | {ruby_lsp_rs_only_count} |")
    output.append("")
    output.append("## 全体\n")
    output.append(
        f"- 比較から除外したファイル (Steep の結果に {UNEXPECTED_ERROR_CODE} があり、Steep が検査できていない): "
        f"{len(excluded_paths)} ファイル (そこでの ruby-lsp-rs の診断 {excluded_ruby_lsp_rs_count} 件も除外)"
    )
    output.append(f"- Steep の診断数 (除外後): {len(steep_diagnostics)}")
    output.append(f"- ruby-lsp-rs の診断数 (除外後): {len(ruby_lsp_rs_diagnostics)}")
    output.append(f"- 一致 (path, line, column, code): {len(exact_matches)}")
    output.append(f"- 位置ずれ (同じ行・同じ code で column が違う): {len(shifted_matches)}")
    output.append(f"- ruby-lsp-rs だけが出した (誤検知候補): {len(ruby_lsp_rs_only)}")
    output.append(f"- Steep だけが出した (取りこぼし、ruby-lsp-rs が実装済みの code): {len(steep_only_implemented)}")
    output.append(f"- Steep だけが出した (ruby-lsp-rs が未実装の code): {len(steep_only_unimplemented)}")
    output.append(f"- 一致・位置ずれのうちメッセージが異なるもの (正規化後): {len(message_mismatches)}")
    if unparsed_lines:
        output.append(f"- ruby-lsp-rs 出力のうち診断として解釈できなかった行: {len(unparsed_lines)}")
    output.append("")
    output.append("「未実装」は ruby-lsp-rs のソースに code の文字列が無く、出力にも現れない code。\n")
    output.append("## code 別の集計\n")
    output.append("| code | ruby-lsp-rs | Steep の severity | Steep | ruby-lsp-rs | 一致 | 位置ずれ | ruby-lsp-rs のみ | Steep のみ |")
    output.append("|---|---|---|---:|---:|---:|---:|---:|---:|")
    for code in sorted(codes, key=lambda code: (code not in implemented_codes, -counters["steep_total"][code] - counters["ruby_lsp_rs_total"][code], code)):
        status = "実装済み" if code in implemented_codes else "未実装"
        output.append(
            f"| {code} | {status} | {severity_summary(code)} | {counters['steep_total'][code]} | {counters['ruby_lsp_rs_total'][code]} | "
            f"{counters['exact'][code]} | {counters['shifted'][code]} | {counters['ruby_lsp_rs_only'][code]} | {counters['steep_only'][code]} |"
        )
    output.append("")

    if severity_mismatches:
        output.append("## severity の違い (一致・位置ずれのうち、github 形式の名前で比較)\n")
        output.append("| code | Steep | ruby-lsp-rs | 件数 |")
        output.append("|---|---|---|---:|")
        for (code, steep_severity, ruby_lsp_rs_severity), count in severity_mismatches.most_common():
            output.append(f"| {code} | {steep_severity} | {ruby_lsp_rs_severity} | {count} |")
        output.append("")

    def sort_key(diagnostic):
        return (diagnostic.code, diagnostic.path, diagnostic.line, diagnostic.column)

    def render_diagnostic(diagnostic):
        output.append(
            f"- `{diagnostic.path}:{diagnostic.line}:{diagnostic.column}` [{diagnostic.severity}] {markdown_code(diagnostic.message)}"
        )
        snippet = source_snippet(source_lines, diagnostic)
        if snippet:
            output.append(f"  - {markdown_code(snippet)}")

    output.append("## 位置ずれ\n")
    for steep, ruby_lsp_rs in sorted(shifted_matches, key=lambda pair: sort_key(pair[0])):
        snippet = source_snippet(source_lines, steep)
        output.append(
            f"- `{steep.path}:{steep.line}` {steep.code} Steep col {steep.column} / ruby-lsp-rs col {ruby_lsp_rs.column}"
            + (f" — {markdown_code(snippet)}" if snippet else "")
        )
    output.append("")

    for title, diagnostics in (
        ("ruby-lsp-rs だけが出した (誤検知候補)", ruby_lsp_rs_only),
        ("Steep だけが出した (取りこぼし、ruby-lsp-rs が実装済みの code)", steep_only_implemented),
    ):
        output.append(f"## {title}\n")
        current_code = None
        for diagnostic in sorted(diagnostics, key=sort_key):
            if diagnostic.code != current_code:
                current_code = diagnostic.code
                output.append(f"\n### {current_code}\n")
            render_diagnostic(diagnostic)
        output.append("")

    output.append(f"## Steep だけが出した (ruby-lsp-rs が未実装の code。error / warning は全件、notice は各 code {UNIMPLEMENTED_EXAMPLE_LIMIT} 件まで)\n")
    unimplemented_by_code = collections.defaultdict(list)
    for diagnostic in sorted(steep_only_unimplemented, key=sort_key):
        unimplemented_by_code[diagnostic.code].append(diagnostic)
    for code, diagnostics in sorted(unimplemented_by_code.items(), key=lambda item: (-len(item[1]), item[0])):
        output.append(f"\n### {code} ({len(diagnostics)} 件)\n")
        blocking = [diagnostic for diagnostic in diagnostics if diagnostic.severity in BLOCKING_SEVERITIES]
        others = [diagnostic for diagnostic in diagnostics if diagnostic.severity not in BLOCKING_SEVERITIES]
        for diagnostic in blocking + others[:UNIMPLEMENTED_EXAMPLE_LIMIT]:
            render_diagnostic(diagnostic)
    output.append("")

    output.append("## メッセージが異なるもの (一致・位置ずれのうち)\n")
    for steep, ruby_lsp_rs in sorted(message_mismatches, key=lambda pair: sort_key(pair[0])):
        output.append(f"- `{steep.path}:{steep.line}:{steep.column}` {steep.code}")
        output.append(f"  - Steep: {markdown_code(steep.message)}")
        output.append(f"  - ruby-lsp-rs: {markdown_code(ruby_lsp_rs.message)}")
    output.append("")

    if unparsed_lines:
        output.append("## 解釈できなかった ruby-lsp-rs の出力行\n")
        for line in unparsed_lines[:50]:
            output.append(f"- {markdown_code(line)}")
        output.append("")

    output.append("## 除外したファイル\n")
    for path in sorted(excluded_paths):
        output.append(f"- `{path}`")
    output.append("")

    summary = {
        "steep_error_or_warning_missed_by_ruby_lsp_rs": len(missed_blocking),
        "by_severity": {
            row[0]: dict(zip(["steep", "matched", "same_severity", "different_severity", "missed", "ruby_lsp_rs_only"], row[1:]))
            for row in severity_rows
        },
        "excluded_files": len(excluded_paths),
        "steep_total": len(steep_diagnostics),
        "ruby_lsp_rs_total": len(ruby_lsp_rs_diagnostics),
        "exact": len(exact_matches),
        "shifted": len(shifted_matches),
        "ruby_lsp_rs_only": len(ruby_lsp_rs_only),
        "steep_only_implemented": len(steep_only_implemented),
        "steep_only_unimplemented": len(steep_only_unimplemented),
        "message_mismatches": len(message_mismatches),
        "severity_mismatches": sum(severity_mismatches.values()),
        "by_code": {
            code: dict(
                {name: counters[name][code] for name in counters},
                implemented=code in implemented_codes,
                steep_severities=dict(steep_severities[code]),
            )
            for code in codes
        },
    }
    return "\n".join(output), summary


def main():
    parser = argparse.ArgumentParser(description="Steep と ruby-lsp-rs の診断を比較する")
    parser.add_argument("--project", required=True, help="対象プロジェクトのパス (ソース行の表示に使う)")
    parser.add_argument("--steep-output", required=True, help="steep check --format=github --severity-level=hint の出力")
    parser.add_argument("--ruby-lsp-rs-output", required=True, help="ruby-lsp-rs type-check の出力")
    parser.add_argument("--ruby-lsp-rs-source", help="ruby-lsp-rs のリポジトリ (実装済み code の判定に使う)")
    parser.add_argument("--report", required=True, help="Markdown レポートの出力先")
    parser.add_argument("--summary-json", help="集計 JSON の出力先")
    parser.add_argument(
        "--steep-compatible",
        action="store_true",
        help="ruby-lsp-rs type-check --steep-compatible の出力と比べる。Ruby::UnexpectedError のあるファイルも除外しない",
    )
    arguments = parser.parse_args()

    steep_diagnostics = parse_steep_github(pathlib.Path(arguments.steep_output).read_text(encoding="utf-8"))
    ruby_lsp_rs_diagnostics, unparsed_lines = parse_ruby_lsp_rs(
        pathlib.Path(arguments.ruby_lsp_rs_output).read_text(encoding="utf-8")
    )
    if arguments.steep_compatible:
        excluded_paths, excluded_ruby_lsp_rs_count = set(), 0
    else:
        steep_diagnostics, ruby_lsp_rs_diagnostics, excluded_paths, excluded_ruby_lsp_rs_count = exclude_unexpected_error_files(
            steep_diagnostics, ruby_lsp_rs_diagnostics
        )
    results = match_diagnostics(steep_diagnostics, ruby_lsp_rs_diagnostics)
    project_name = pathlib.Path(arguments.project).resolve().name
    report, summary = render_report(
        project_name, steep_diagnostics, ruby_lsp_rs_diagnostics, unparsed_lines, results, SourceLines(arguments.project),
        implemented_codes_from_source(arguments.ruby_lsp_rs_source), excluded_paths, excluded_ruby_lsp_rs_count,
    )
    pathlib.Path(arguments.report).write_text(report + "\n", encoding="utf-8")
    if arguments.summary_json:
        pathlib.Path(arguments.summary_json).write_text(json.dumps(summary, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(
        f"diagnostics: excluded_files={summary['excluded_files']} exact={summary['exact']} "
        f"severity_mismatches={summary['severity_mismatches']} shifted={summary['shifted']} "
        f"ruby_lsp_rs_only={summary['ruby_lsp_rs_only']} steep_only_implemented={summary['steep_only_implemented']} "
        f"steep_only_unimplemented={summary['steep_only_unimplemented']}",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
