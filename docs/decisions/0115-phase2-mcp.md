# ADR 0115: Phase 2 を開始し、既存セッションへ最小 MCP を接続する

- 日付: 2026-10-17
- 状態: 採用
- 範囲: Phase 2 の最初の実装単位。制作・ミックス機能全体の完成ではない

## 判断

ユーザーが Phase 2 への移行を指定した。P1 の[最終照合](../reviews/2026-10-17-phase1-acceptance.md)を保持し、PLAN の全範囲を削らず[進め方](../phase2.md)を追加する。後続の機能を同じ製品経路から検査できるよう、計画にある最小 MCP を最初に接続する。M4 / macOS 27 のビルド条件と、測定で汎用設定が速い場合の例外を引き継ぐ。

`echode mcp SOCKET` は stdio のアダプターであり、同じビルドの `echode serve PACKAGE SOCKET` へ接続する。DocumentHost をもう一つ作らず、API のアクション・一括編集・Undo / Redo・概要を利用する。書き手と成功応答前のジャーナル、失敗時の巻戻しは [ADR 0032](0032-document-host.md) / [ADR 0034](0034-api-transaction.md) のままである。独立した空の crate や新しいライブラリの版は足さない。CLI の検査に既存の serde_json を使う。

## 公開する範囲

- 登録簿の各アクションを `echode.<action-id>` というツールへ生成する。入力スキーマは同じ登録簿から取り、`type` はツール名から決定する。呼出側による `type` の上書きは拒否する。
- `echode.actions.transaction` は既存の ID 参照を含む操作列を1回の Undo にする。`echode.edit.undo` / `echode.edit.redo` と読み取り専用の `echode.project.summary` も提供する。
- リソースは `echode://project/summary` と `echode://project/tracks`。現在の接続先だけを読み、任意のファイルパスは開かない。編集用ツールと読取り用を annotation で区別する。
- MCP 2026-07-28 は要求ごとの版と capabilities、`server/discover`、結果の種類と private / TTL 0 のキャッシュ情報を扱う。2025-11-25 は initialize → initialized として別に扱い、要求 ID の再使用を拒否する。両者を接続状態から勝手に読み替えない。

標準出力は MCP の1行1メッセージだけに使い、診断は CLI の標準エラーへ出す。EOF はアダプターだけを終了し、接続先のセッションを終了しない。外部クライアントへの登録、GUI アプリのソケット共有、変更購読、再生・機器・レンダリングの操作、HTTP、長い非同期タスクは今回の範囲外である。ユーザーの設定や起動中アプリを変更しない。

## 入力・接続・失敗の境界

通知や不正な要求 ID では編集しない。ツールの入力形とアクションは既存の型・モデルで検査し、既知のツールの実行失敗は `isError` と元のエラー・要素番号を返す。未知のツール・不正なプロトコルは JSON-RPC のエラーにする。プロジェクトの文字列は JSON のデータとして返す。

接続先は Unix ソケットだけで、`libc::getpeereid` により同じ uid を要求する。各呼出しで API 版1を確認してから処理を送り、応答の ID・形も検査する。呼出しごとに閉じるため、MCP のアイドル中に既存の1接続制サーバーを占有しない。接続先への読書きは既存の30秒、1メッセージは既存の1 MiBで区切る。これはプロジェクトやトラック数の固定上限ではない。今回の概要はページングしないため、1行に入らない大きい概要はエラーになる。

書込み後の切断やタイムアウトでは、編集が記録済みか不明な場合がある。自動で再送せず、概要を調べてから判断する案内を返す。処理中の取消によって、確定済みの編集を勝手に巻き戻す仕組みは加えない。同時実行は直列で、未対応の機能を capabilities に広告しない。

プロトコルの一次資料は [2026-07-28 の versioning](https://modelcontextprotocol.io/specification/2026-07-28/basic/versioning)、[discovery](https://modelcontextprotocol.io/specification/2026-07-28/server/discover)、[stdio](https://modelcontextprotocol.io/specification/2026-07-28/basic/transports/stdio)、[2025-11-25 の lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle)と[tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)。全仕様や全 MCP クライアントの互換性を保証した意味ではない。

## 検査と基盤の保守

実 CLI の stdio → 実 Unix ソケット → DocumentHost で、トラックとノートの一括編集、単一 Undo、Redo、途中失敗の巻戻し、別クライアントからの同じ版の読取り、終了後の復旧を確認する。未実装の CLI ではこのテストが応答前に失敗した記録を `target/mcp-before.log` に保持する。修正後は同じ入力を通す。

旧・新のプロトコル、通知・ID・メタデータ・引数の拒否、接続先停止、異なる API 版への編集禁止、破損応答、メッセージ上限、EOF の回帰も追加する。`mcp-edit` を既存の Makefile の製品契約へ登録し、編集を読取りへ置き換えたコピーで失敗することを要求する。選択の回帰と、README・status・PLAN・設計・開発・検査・デバッグ手順も更新する。診断の収集は既存の契約ログに載るため新しい収集先は不要である。

検査結果の正本は `target/verification/report.md`。MCP クライアントへの設定、実アプリ・音声機器・聴感・実機性能は今回の自動回帰とは区別する。

最初の全検査は基盤90テスト、通常回帰、製品契約28件と負の対照28件、RTSan、入力、全 feature、ASan が通り、TSan の Swift 製品検査で停止した。新しいスレッドの `set_errno` と終了済みのスレッドのエラー読取りが同じアドレスとして報告された。[ADR 0114](0114-m4-build-baseline.md) の標準ライブラリだけの独立再現と同型であり、HAL の警告とは異なる。今回のログと失敗したレポートを `target/phase2-mcp/tsan-errno-before.log`・`tsan-verification-before.log`・`verification-before-errno.json` に保存し、抑制・条件変更をせず正式な入口を再実行する。内部原因の解決を確認した意味ではない。

同じ正式な入口の再実行は TSan・Miri を含む自動検査9項目が合格した。MCP の実 CLI 回帰4件と通信境界4件、製品契約28件と負の対照28件、基盤90テストを含む。release の `target/release/echode` もビルドし、旧・新プロトコルの初期応答、23個の生成ツール、EOF の終了を確認した。`target/phase2-mcp/release-build.log` と `release-stdio.json` に保持する。Mach-O は arm64 単独・最低 OS 27.0 だった。外部 MCP クライアントへの登録やユーザーのアプリ操作は行っていない。書出し処理を変えていない `spike-render` と、新しい依存版を加えていない `advisories` は理由付きで見送った。TSan の警告は再実行の合格で解決へ読み替えない。

## 改行境界の回帰補強（2026-10-29）

`mcp.rs::read_line` は導入時から、stdio 入力と接続先 API の応答の両方で改行前の EOF を拒否している。
今回は製品挙動を変更せず、単体検査にとどまっていた境界を公開入口と実ソケットの回帰へ補強した。
JSON 自体が完結していても、CR だけの終端や末尾空白は LF の代わりにならない。

- `mcp_framing.rs` は旧・新プロトコルの部分行で単一／一括編集・Undo / Redo を送らないことを確認する。
  ジャーナルと既存の Redo、完了済みの前方の編集、再オープン後の状態を検査する。
- 接続先の版応答が部分行なら編集を送らず、編集応答の部分行なら成功とせず、再送もしない。
  実ソケットの半切断で再現し、結果不明の案内を確認する。編集済みかどうかの自動判断は追加しない。
- LF / CRLF を含めてちょうど 1 MiB の入力、その 1 byte 超過、空の EOF と未完了行を通常回帰で区別する。
  小さい読込み単位で、JSON トークンと CRLF が分割されても前方の要求を保つことも確認する。

必須契約 `mcp-stdio-framing`・`mcp-backend-framing` を既存の検査入口へ登録する。
各契約はコピー上で改行検証だけを外す負の対照を持ち、コンパイル失敗でなく境界のアサーションによる失敗を要求する。
外部 MCP クライアントへの登録・機器・GUI・聴感は変更も検査もせず、既存の未確認範囲を合格へ読み替えない。

## 接続先のエラー応答の形の検証（2026-10-29）

`058b027` を基準に、実ソケットで編集を記録した後の応答を `error:null` へ置き換えると、
MCP が `structuredContent.error:null` の通常の実行エラーとして返すことを再現した。
結果不明と再送前の確認の案内がなく、コード・メッセージの欠落や不正な型も同様に通っていた。

- `exchange` は、応答の版・ID・result/error の排他に加え、error がオブジェクトで、
  `code` が整数、`message` が文字列であることを検証する。
  不正なら `invalid session response` とし、ツールは既存の結果不明・再送前の確認の案内を返す。
- 正当な実行エラーの code・message・任意の data は保持する。
  正当な result の値にはエラー用の形を強制しない。MCP の旧・新プロトコルとも同じ検証を通る。
- `tests/mcp_responses.rs` は公開 `serve_mcp` と実 Unix ソケットで、実際に記録済みの編集、
  再送・再接続の禁止、単一 Undo / Redo、後続の要求と再オープンを検査する。
  自動保存でジャーナルが短縮され得るため、生の行数の増分ではなくモデル・履歴・再オープンで確定を照合する。
- 必須契約 `mcp-backend-errors` は error の検証だけを外す負の対照を持つ。
  修正前の回帰は終了コード 101 と `malformed backend errors must report an uncertain edit` で失敗した。
  作業時の記録は `target/focused-checks/mcp-response-red/` に残す。

制御側の応答検証だけを変更し、音声・保存形式・外部 MCP クライアントの設定は変更しない。
既に記録した編集を巻き戻したり、結果不明の応答を理由に自動で再送したりしない。
