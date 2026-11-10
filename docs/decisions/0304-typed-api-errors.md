# ADR 0304: API・MCP の原因を応答の直前まで型で保つ

日付: 2026-11-10
状態: 採用
対象: M1（DEV-7・SPG-11・XCA-16）の `echode-api`
関連: [ADR 0299](0299-typed-document-journal-and-settings-errors.md)、[ADR 0303](0303-typed-model-invariant-errors.md)

## 問題と再現

`ApiError` は文だけを持ち、ソケット・MCP の I/O とパッケージの原因を `to_string()` で失っていた。
`PerformError` も、文書ワーカーから受け取った `HostError` を文と添字へ写していた。
MCP のリソース要求では、接続先の失敗を `ProtocolError` の文にしてから応答を作っていた。
保存の不確定や errno を診断するには、この境界の手前まで戻る必要があった。

公開入口の `tests/typed_errors.rs` を先に追加し、修正前の製品経路で再現した。
存在しない親への `bind_socket`、存在しないパッケージへの `serve`、不正な UTF-8 を読む `serve_mcp` は、元の原因を `Error::source` から取得できなかった。
期待した終了コード 101 と、原因の保持を要求する各回帰のアサーションを照合した。
MCP の書込み・flush の故障注入も同じ回帰に含める。

## 比べた案

| 案 | 判断 |
|---|---|
| 文だけの `ApiError` を残し、ログへ補助情報を足す | 採らない。呼出し元で原因を分類できず、ログとエラーの二つを照合する必要がある |
| I/O・保存の原因を新しい分類へ写す | 採らない。errno・JSON・保存の不確定の正本を重複して持つ |
| 元の原因と、API 固有の拒否を列挙型にする | 採る。公開入口から `Error::source` をたどれ、表示文を変えずに済む |
| MCP の `ProtocolMessage::Session` に原因を値で内包する | 採らない。`ProtocolError` が 144 バイトになり、`result_large_err` が拒否した |
| MCP の接続先の原因だけを `Box` にする | 採る。失敗時だけの確保で、通常の応答とローカルの引数拒否は確保を増やさない |

時間・音質・有効なプロジェクトの使用メモリの改善は主張しない。

## 決定と境界

- `error.rs` に `ApiError` と `ApiIoOperation` を置き、公開する。
  `Io` は失敗した操作と元の `io::Error`、`Package` は `PackageError`、`SessionJson` は接続先応答の `serde_json::Error` を持つ。
  パスの拒否・別ユーザー・行上限・改行未完了・切断・応答不正・非対応の版は別の変種にする。
- `Error::source` は元の I/O・保存・JSON を返す。表示は従来の文を `Display` で作る。
  API の応答書込みだけの接頭辞も保つ。
- RPC の `PerformError` は引数 JSON・トランザクションの形・`HostError`・応答の直列化を分ける。
  元の `HostError::Element` を保持し、コードの選択は既存の `ElementError::is_parameters`、添字は元の値を使う。
  表示文を作るのは `outcome` の応答生成時だけとする。
- MCP の `ProtocolError` はローカルの固定文と `Box<ApiError>` を分ける。
  リソース要求は原因を保持したまま応答生成へ渡す。ツール要求は最終的な `structuredContent` の生成時だけ文にする。
  接続先が返した正当な error の code・message・data は従来どおりそのまま転送する。
- タイムアウト・同一ユーザーの照合・ソケットの置換条件・入力の検証の順番は変えない。
  編集の再送・巻戻しは追加せず、確定後の送信失敗は結果不明のまま扱う。
  保存形式・RPC のコード・JSON の形・C ABI・音声コールバックは変えない。

## 回帰と基盤

- `tests/typed_errors.rs` は公開入口で I/O の操作・errno・`source`、パッケージの原因、表示とパスの原本保持を照合する。
- `serve_tests.rs` は通常ファイル・使用中ソケットの拒否と、応答の書込み期限の原因を型で照合する。
  既存の後続接続と編集の単一実行の検査は保持する。
- `rpc_tests.rs` は製品ワーカーを通して引数・モデル・参照・素材読込みの原因と添字を照合し、拒否後のモデル保持を確かめる。
  保存の不確定の包み方と従来の wire 応答も照合する。
- 統合の最終検査では、直前の ADR 0303 の `track_changesets_keep_hierarchy_and_child_resolution_checks` が、循環で到達不能となった ID の決め打ちで失敗した。
  再生の入力は HashMap から集めるため、循環の先頭と、等しいキーの二者の順は固定されない。
  診断の順を固定するために製品の並べ替えを足す案は採らず、回帰は原因の変種・該当する ID の集合・ID ごとの名前の保持を照合する。
  不正な入力の拒否と、原因の詳細を検査する範囲は保つ。
- MCP の単体テストは `mcp_tests.rs` へ分離する。
  JSON・応答外枠・改行・切断・版・未送信の行上限を分け、リソースの原因とツールの結果不明の案内を照合する。
- `check_typed_errors.py` に crate 全体を登録し、Python 回帰で既存・新規・入れ子のモジュールへの再導入を拒否する。
  設計書・コードの所在・デバッグ手順・監査の統合表も更新する。
- 外部入力・実行入口・検査の種類は増えず、既存の通常検査と fuzz が同じ入口を通る。
  Makefile と `scripts/verification.json` の選択条件の追加は不要である。
  利用者の操作と制約は変わらず、`status.md` とユーザー確認の追加も不要である。

## 残る範囲

M1 全体の完了とはしない。次は `echode-cli` の文だけの `CliError` である。
アクションの引数エラーの表示文の解析も残る。
検査結果と実機・計測の見送り理由は `target/verification/report.md` を正本にする。
