# ADR 0307: 文書の単一編集も型付きで直接 JSON から読む

日付: 2026-11-10
状態: 採用
関連: M1（DEV-7・SPG-11・XCA-16）、ADR 0292・0306

## 問題と再現

RPC・一括編集は `Command::from_value` を使うが、文書ワーカーの単一編集は Serde の互換入口を使っていた。
そのため、ID・拍子・テンポの意味の拒否が `DocumentError::Malformed(serde_json::Error)` へ戻り、入力と原因の種類を失っていた。

公開の `DocumentService::submit(Operation::Perform, Ticket)` を通す回帰を先に追加した。
不正な ID `broken at line 7: meter does not divide into ticks\n音` は `Malformed(Error(...))` になり、`Error::source` から `ParameterError` が見つからず失敗した。
期待終了コード 101 と回帰固有のアサーションを `checked_commands.py` で照合した。
ログは `target/focused-checks/document-parameters-red/` に置いた。

## 比べた案

| 案 | 判断 |
|---|---|
| Serde のエラーの文を分類し直す | 採らない。ADR 0306 が除いた構造を別の層へ戻してしまう |
| JSON を `Value` にして既存の `from_value` を使う | 採らない。重複キーをまとめ、直接読む時の重複フィールドの拒否と JSON の位置情報が変わる |
| `FromStr` でも登録簿の wire と既存の変換を使う | 採る。意味の検証を重複せず、JSON を直接読む時の境界を保てる |

性能改善は主張しない。音声スレッド・保存形式・モデルの規則は変更しない。

## 決定と境界

- `Command` に `FromStr<Err = ParameterError>` を実装する。
  `WireCommand` を JSON テキストから直接読み、全体の形と終端を確認した後に同じ `TryFrom` でモデルの値へ変換する。
  終端と意味がともに不正なら、Serde 互換入口の意味の拒否でなく JSON の終端の拒否を優先する（ADR 0306 の形を先に検証する規則）。
- 文書ワーカーの単一編集はこの入口を使う。
  空入力と UTF-8 の拒否、一括編集の参照の解決・取消、履歴へ渡す順番は変えない。
- `DocumentError::Parameters` は意味の拒否と入力値を持ち、`Error::source` へつなぐ。
  `ParameterError::Json` は元の `serde_json::Error` のまま既存の `Malformed` へ移す。
  構文・形の診断、位置情報、呼出側の `Malformed` の分類は変えない。
- 表示文と C ABI の数値・コールバック、Swift の編集型、ジャーナル・Undo / Redo は変えない。
  一括編集の失敗は引き続き `History(Element { index, Parameters(...) })` で、添字も保持する。
- Serde の互換入口は保存・外部の汎用コードのため残すが、型付きの編集処理からは使わない。

## 回帰と基盤

- `echode-document/tests/edit_errors.rs` は製品ワーカーを通し、意味の原因・入力・表示文と、一括編集との一致を照合する。
  拒否後の同じスナップショット、Redo、ジャーナルの不変、再オープン、後続編集の受理も調べる。
- 同じ回帰で構文・形の拒否の元の JSON エラーと位置・文面、重複フィールド・余分な JSON・空入力・不正な UTF-8 を照合する。
- `echode-actions/tests/command_text.rs` は型付きのテキスト入口の意味の原因と順番、形・終端の優先、既定値・flatten・整数の端点を調べる。
- 設計書・コードの所在・既存のデバッグの節・監査の状態を更新する。
  既存の通常テストが両方の回帰を実行するため、Makefile と `scripts/verification.json` に新しい検査や選択条件は増やさない。
- 新しい外部入力・成果物・機器操作はなく、fuzz の対象と診断の収集先は変えない。
  利用者に残る制約と `status.md`、実機・実画面の確認待ちも変えない。

## 残る範囲

M1 全体の完了とはしない。次はヘルパーの起動失敗を `threads.rs::reject_ports` が `Refusal::Other` の文へ変える境界である。
ADR 0295 で残したローダー等の失敗と、設計上の手順の食い違いを分けて扱う。
検査結果と計測・実機の見送り理由は `target/verification/report.md` を正本とする。
