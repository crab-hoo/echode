# 0324. エージェントのフックを置かない

- 日付: 2026-11-12
- 状態: 採用
- 関連: [XCA-06](../reviews/2026-11-02-cross-cutting-audit.md#xca-06-防護がエージェントごとに違う)（ENV-04）、[統合表の M6](../reviews/2026-11-08-open-items-register.md#2-保守性)、
  [ADR 0278](0278-audit-backlog-order.md)（順番）、[ADR 0276](0276-priority-order-and-delegated-decisions.md)（優先順位）
- 置き換え: [ADR 0197](0197-verification-target-scopes.md) の `hook_timeout` を外側の上限の正本とする決定、
  [ADR 0212](0212-verification-tiers.md) の作業の段階を終了フックからも呼ぶ記述、[ADR 0269](0269-github-public-ci-scope.md) の `.codex/hooks.json` のパスの修正

## 背景

M6 の時点で、エージェントのホストごとに次のフックがあった。

| ホスト | 編集の前 | 停止の時 |
|---|---|---|
| Cursor | `preToolUse` が編集と `Shell` を検査のロックが外れるまで待たせる | `stop` が `scripts/verify.py` を呼ぶ |
| Claude Code | `PreToolUse` が Edit・MultiEdit・Write・NotebookEdit を待たせる | `Stop` が `scripts/verify.py` を呼び、失敗で 3 回まで続けさせる |
| Codex | `preToolUse` だけ。matcher は Cursor の道具名の写しで、発火を確かめていない | なし |

M6 は監査（XCA-06）どおり Codex の matcher と応答を直す項目だった。
2026-11-12、ユーザーは、フックの挙動が不安定なので Claude Code のものも含めて全て無くす方針を示した。

フックは次の費用を持っていた。

- ホストごとに入出力の形が違い、`--host` の変換と応答の形のテストが要った。同じ規則を層ごとに別の論理で持つ形で、ADR 0276 は堅牢性の損失に数える。
- 外側の上限 `hook_timeout` は全自動検査の直列予算を覆う必要があり、検査を足すたびに上げた（15600 → 17460 秒、[ADR 0290](0290-fuzz-targets-for-external-input.md)）。
- Cursor の `Shell` の待機は、読むだけのコマンドも最大で 15,590 秒止めた（XCA-06）。
- 停止のフックは、質問への回答のような作業の完了でない停止でも `make verify` を走らせた。
- 発火そのものはホストの挙動で、リポジトリの検査では確かめられない。回帰は上限の秒数と応答の形だけを照合していた（XCA-06）。
- ホストはセッションの開始時にフックを読み込む。この作業の途中でも、読み込み済みの Claude Code のフックが新しい引数の
  `wait_for_verify.py` を古い引数（`--host claude`）で呼び、終了コード 2 で編集を拒否した。リポジトリの変更とホストの状態がずれると、編集そのものが止まる。

フックが守っていた性質は、フックの外にもある。

1. 完了時の `make verify` は AGENTS.md の必須規則である。フックは補助だった。
2. 検査中の編集で誤った合格は残らない。`scripts/verify.py` は検査の後に入力の識別値を取り直し、変わっていれば結果を `stale` にする
   （`source or toolchain changed during verification`）。フックがなくて失うのは、検査をやり直す時間である。

## 比べた案

| 案 | 内容 | 判定 |
|---|---|---|
| 1 | 監査どおり Codex の matcher と応答を直し、3 つのホストのフックを保つ | 採らない |
| 2 | 発火を確かめた Claude Code と Cursor だけ残し、Codex には置かない | 採らない |
| 3 | フックを全て外す。待機と完了時の検査は AGENTS の規則と `make wait-verify`、検査中の編集は `stale` で扱う | 採る |

- 案 1: Codex の道具名と応答の形は確かめられず、設定を変えるたびに Codex で設定を再び信頼する手順が要る。ホストごとの変換と予算の整合も残る。
- 案 2: ホストによって防護が違う状態（XCA-06 の指摘そのもの）が残る。ユーザーが報告した不安定さも残る。
- 案 3: 防護はホストに依らず同じになる。誤った合格を止めるのは `verify.py` の識別値で、どのホストでも同じ規則である。

## 決定

- 次を削除した。`.claude/settings.json`（中身はフックだけ）と `.claude/hooks/`、`.cursor/hooks.json` と `.cursor/hooks/`、
  `.codex/hooks.json` と `.codex/hooks/`、`scripts/check_on_stop.py`。
- `scripts/verification.json` の `hook_timeout` と、`verify.py` のその予算の検査を外した。各検査の `timeout` は変えない。
- `scripts/wait_for_verify.py` はフックの JSON の入出力をやめ、エージェントが自分で呼ぶコマンドにした（`make wait-verify`）。
  ロックが外れれば 0、外れなければ 1 で終わる。既定の 540 秒は、エージェントの前景のシェルの上限（Claude Code の Bash は 600 秒）より短くした。長い待ちは呼び直す。
- `check-verification` の `NoAgentHooksTests` が、3 つのホストのフックの設定と `hook_timeout` の再導入を止める。
- `target/agent-check.log` は書くものがなくなったので、`make debug-report` の収集から外した。
- 2026-11-12 の時点で、Git のフック（`core.hooksPath`、`.git/hooks` の見本以外）もない。手元の設定なので、テストでは確かめない。
- `scripts/tests/test_agent_hooks.py` はフックのテストを除き、残りの内容に合わせて `test_audit_reproductions.py` へ名前を変えた。

## 影響

- エージェントが規則を守らずに停止しても、止めるものはない。完了時の `make verify` と最終報告での記載（AGENTS）、
  `main` へのプッシュごとの GitHub の `full`（[ADR 0269](0269-github-public-ci-scope.md)）が残る。
- 他のセッションの検査中に編集すると、その検査は `stale` になり、合格として残らない。
- 既に開いているセッションは、開始時に読み込んだフックを持ち続けることがある。削除したスクリプトを呼ぶフックは失敗するので、
  編集が止まる時はセッションを開き直す。
- XCA-06・ENV-04（M6）を閉じた。ADR 0276 でユーザーに残していた、Codex で設定を再び信頼する手順も要らなくなった。
- 監査と ADR 0093・0197・0212・0290 のフックの記述は、当時の記録として残す。

## 再開の条件

次のどちらかの時に検討し直す。

- フックの入出力の形と発火をリポジトリの検査で確かめられ、全てのホストで同じ規則を一つの変換で置ける。
- `make verify` を省いて終えた作業が、最終報告の確認や GitHub の `full` で続けて見つかる。
