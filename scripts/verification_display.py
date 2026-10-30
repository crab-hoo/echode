"""Presentation only: never decide selection, evidence validity, or process results."""


def render_report(report, policy, selected, summarize):
    lines = ["# 変更に対する検査", "", f"更新: {report['updated_at']}",
             f"ソースの識別値: `{report['fingerprint']}`", ""]
    execution = report.get("execution", {})
    if execution:
        lines += ["## 今回の実行", "", "実行順（再利用を除く）: " + ", ".join(execution["order"]), ""]
        if "seconds" in execution:
            lines += [f"今回の所要時間: {execution['seconds']} 秒（{execution.get('status', 'running')}）", ""]
        for name, timing in execution.get("checks", {}).items():
            lines.append(f"- `{name}`: {timing.get('seconds', 0)} 秒（今回実行）")
        if execution.get("reused"):
            lines += ["", "再利用（今回の実行時間には加算しない）: " + ", ".join(execution["reused"]), ""]
        failure = execution.get("first_failure")
        if failure:
            lines += [f"最初の失敗: `{failure['kind']}:{failure['id']}` — 開始から {failure['seconds']} 秒", ""]
    for name, result in report.get("preflight", {}).items():
        lines.append(f"- 事前確認 `{name}`: **{result['status']}** ({result['seconds']} 秒)")
        if result["status"] != "passed":
            lines.append(f"  - 復旧: `{result['remedy']}`")
        lines.append(f"  - ログ: [preflight-{name}](preflight-{name}.log)")
    lines += ["", "## 検査結果", ""]
    for check in policy["checks"]:
        name = check["id"]
        if name not in selected and not (check.get("extended") and name in report["results"]):
            continue
        kind = "自動検査" if check["mode"] == "automatic" else "追加の長時間検査" if check.get("extended") else "計測などの候補"
        result = report["results"][name]
        status = result["status"]
        if result.get("fingerprint") != report["input_fingerprints"][name]:
            status = "stale"
        if name not in selected:
            kind += "・通常の完了判定外"
        lines.append(f"- `{name}` ({kind}): **{status}** — {check['purpose']}")
        if result.get("reason"):
            lines.append(f"  - 未実行の理由: {result['reason']}")
        if result.get("log"):
            lines.append(f"  - ログ: [{name}]({name}.log)")
        lines.append(f"  - 検査入力の識別値: `{result['fingerprint']}`")
        reasons = selected.get(name, ["以前に明示実行した追加検査の記録"])
        lines.append("  - 選択の根拠: " + summarize(reasons))
    lines += ["", "## 対象の変更", ""] + [f"- `{path}`" for path in report["changed_paths"]]
    lines += ["", "passed と deferred は各検査の入力・ツールが一致する間だけ有効。全体の識別値は変更監視用。not_run は実行か理由の記録が必要。deferred は合格ではない。", ""]
    return "\n".join(lines)


def print_plan(changed, ordered, selected, summarize):
    if changed:
        print(f"対象の変更 ({len(changed)} 件): " + summarize(changed, limit=8))
    for check in ordered:
        name = check["id"]
        kind = "自動検査" if check["mode"] == "automatic" else "追加の長時間検査" if check.get("extended") else "計測などの候補"
        print(f"{name} [{kind}]: {check['purpose']} (根拠: {summarize(selected[name])})")
    if not selected:
        print("対象の変更はありません。コミット済みの変更には --base、全検査には --all を指定します。")
