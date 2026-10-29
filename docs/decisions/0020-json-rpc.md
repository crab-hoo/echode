# 0020. JSON-RPC は本人だけのソケットでアクションを受ける

- 日付: 2026-10-08
- 状態: 採用

## 背景

UI はクライアントの 1 つで、同じ操作を外からも呼べるようにする（ARCHITECTURE §15）。Phase 1 では、その最小の形を先に置く。

## 決定

- プロトコルは JSON-RPC 2.0。1 行に 1 件
- メソッドは `actions.list`、`actions.perform`、`actions.transaction`、`edit.undo`、`edit.redo`、`project.summary`、`protocol.version` である。複数の操作、未採番の ID への参照、Undo と Redo、プロトコルの版は [ADR 0034](0034-api-transaction.md)
- `actions.perform` のパラメータは、登録簿の ID を `type` に使う。ジャーナルの `type` も同じ ID である（[ADR 0030](0030-action-catalog.md)）。作られた ID は結果に入る
- モデルが拒否した編集は、プロジェクトを変えずにエラー `-32000` で返す
- `echode serve PACKAGE SOCKET` がソケットを開く。権限は本人の読み書きだけ（`0600`）にする。接続は 1 本ずつ扱い、後の接続は前の編集が見える

## 結果

トラック追加が一覧に残ること、拒否された拍子変更がプロジェクトを変えないこと、ソケットの権限が本人だけであることをテストで確かめる。

## 要求の外枠の検証（2026-10-29）

`cca5a35` を基準に、`actions.perform` の `id` が `true` でもトラックを追加し、
ジャーナルへ編集を書き込むことを公開 `ApiSession::reply` で再現した。
`edit.undo`・`edit.redo` も不正な `id` やスカラーの `params` を検証せず処理する経路だった。
また、版・メソッドが不正でも `id` がなければ通知として応答を省略していた。

- ディスパッチ前に要求オブジェクトの外枠を検証する。
  `jsonrpc` は `"2.0"`、`method` は文字列、`id` は省略・文字列・数値・null だけとする。
  `params` は省略またはオブジェクト・配列で、明示した null・真偽値・数値・文字列は拒否する。
- 外枠が不正なら、`id` の省略にかかわらず `-32600` を返して編集は実行しない。
  読める正当な `id` は応答へ写し、不正な型の `id` は null にする。
  正しい要求で `id` がない場合だけ通知とし、メソッド・引数のエラーも含めて応答を省略する。
- 配列は外枠として正しい `params` だが、個別メソッドが受けない場合は従来どおり `-32602` になる。
  空行の無視、1 行 1 件の規則、正しい要求・通知の編集内容と `protocol.version` の値は変えない。
  JSON-RPC のバッチ要求は今回追加しない。
- `request_validation.rs` は公開 API と実ソケットの両入口を使う。
  不正要求でモデル・ジャーナル・Undo / Redo が変わらないこと、再オープン、
  不正な通知との区別・後続要求・null と数値を含む ID の応答を検査する。
  必須契約 `rpc-request-envelope`・`rpc-notification-boundary` に検証を外す負の対照を置く。
- 修正前は終了コード 101 と `invalid RPC envelope must not execute an edit` で失敗した。
  作業時の記録は `target/focused-checks/rpc-envelope-red/` に残す。

この変更は制御側の入力境界だけを直し、保存形式、音声処理、MCP の独自の外枠検証は変更しない。
