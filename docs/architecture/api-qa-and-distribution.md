# API・MCP・品質保証・ビルド配布アーキテクチャ（§15〜§17）

- 全体索引: [../ARCHITECTURE.md](../ARCHITECTURE.md)
- 検査手順: [../verification.md](../verification.md)
- デバッグ手順: [../debugging.md](../debugging.md)
- 現在の実装範囲: [../status.md](../status.md)

## 15. 外部制御・API・AI

- **JSON-RPC**: ユーザー本人だけが読み書きできる Unix ドメインソケットで 1 行 1 件の JSON-RPC 2.0 を提供する。
  `actions.list`、`actions.perform`、`actions.transaction`、`edit.undo`、`edit.redo`、`history.usage`、`project.summary`、`protocol.version` を受ける
  （[ADR 0020](../decisions/0020-json-rpc.md)、[ADR 0034](../decisions/0034-api-transaction.md)）。
  `history.usage` と MCP の `echode.history.usage` は、Undo・Redo の段数と段だけが持つメモリの推定を返す（[ADR 0286](../decisions/0286-history-usage-diagnostics.md)）
  要求の版・メソッド・ID・params の外枠を編集前に検証し、不正なら ID の省略時も `-32600` を返す。
  正しい通知だけ応答を省略し、不正な型の ID は null として返す
  （[ADR 0020 の後続修正](../decisions/0020-json-rpc.md#要求の外枠の検証2026-10-29)）。
  前後の空白除去は JSON の SP・HT・CR・LF だけとし、その他の空白は `-32700` で編集前に拒否する。
  文字列内の Unicode と正しい空行・通知は保つ
  （[ADR 0020 の空白検証](../decisions/0020-json-rpc.md#json-の空白だけを除去する2026-10-29)）。
  無変更も成功として Action／配列を返すが、履歴は増やさない。配列の受理順と参照位置を保つ
  （[ADR 0182](../decisions/0182-noop-edits.md)）。
  接続は直列に処理し、読込み・応答書込みの無進行をそれぞれ 30 秒で打ち切る。
  送信失敗はその接続だけを閉じ、既に成立した編集を再実行しない（[ADR 0073](../decisions/0073-socket-path.md)）。
  改行前に EOF となった要求は、その接続だけをエラーで閉じて編集へ渡さない。
  完了済みの前方の要求は保持し、CRLF・分割受信・改行込みの行上限は変えない
  （[ADR 0073 の改行境界](../decisions/0073-socket-path.md#改行前の切断の拒否2026-10-29)）。
- **CLI とヘッドレス**: `echode render`（ステレオ float / PCM16 / PCM24 WAV 書き出し、[ADR 0248](../decisions/0248-pcm16-wav-export.md)）、
  `echode apply`（アクション列の実行）、`echode serve`（ヘッドレスサーバー）、`echode mcp`（MCP サーバー）
- **MCP**: アクション登録簿から MCP ツールを自動生成し、プロジェクト概要やトラック一覧をリソースとして公開する。
  Phase 2 の初版（[ADR 0115](../decisions/0115-phase2-mcp.md)）は `echode mcp SOCKET` の stdio と同一ビルドの `echode serve` 接続を実装し、MCP `2026-07-28` と `2025-11-25` の要求を分ける。
  書き手は既存の `DocumentHost` のままで、MCP がパッケージを開き直さない
  stdio と接続先 API の応答は、どちらも改行の完了を要求する。
  版応答の部分行では編集を送らず、編集応答の部分行は結果不明として案内し、再送しない。
  完了済みの前方の編集と、改行込み 1 MiB の境界は保持する
  （[ADR 0115 の回帰補強](../decisions/0115-phase2-mcp.md#改行境界の回帰補強2026-10-29)）。
  接続先の error はオブジェクト・整数 code・文字列 message を検証し、不正なら結果不明として案内する。
  確定済みの編集は巻き戻さず再送せず、正当な error の data と result の値は保持する
  （[ADR 0115 の応答検証](../decisions/0115-phase2-mcp.md#接続先のエラー応答の形の検証2026-10-29)）。
- **自然言語コマンド**: コマンドパレットから Foundation Models のオンデバイスモデルで関連アクション検索・文脈取得・構造化計画を作成し、利用者確認後に 1 つの Undo 単位として実行する（Apple Intelligence 非対応環境では通常パレットとして動作）

## 16. 品質保証

現在の接続範囲は [status.md](../status.md)、実行と証拠の扱いは [検査手順](../verification.md) と [デバッグ手順](../debugging.md) を参照。

| 種類 | 内容 |
|---|---|
| 単体テスト | モデルの操作、テンポマップ、時間の変換、スマートクリップの規則、グラフの組み立て、PDC の計算 |
| プロパティベーステスト | ランダムなアクション列を流し、Undo / Redo の対称性、保存と読み込みの往復、マージの性質を検査する（`proptest`） |
| ゴールデンテスト | テスト用プロジェクトを CLI でレンダリングし、基準音声と比べる |
| 隔離のテスト | わざと壊れるテスト用プラグインでクラッシュ、ハング、遅延、大量イベントを再現する |
| 互換性のテスト | 無料プラグインでスキャン、処理、状態の保存と復元、GUI の表示を確かめる |
| UI のテスト | オフスクリーン描画の比較と主要操作の自動テスト |
| 性能のテスト | エンジンのベンチマーク（トラック数 × デバイス数）と描画のフレーム時間 |
| ファジング・耐久テスト | プロジェクト読み込み、API 入力、MIDI 解析のファジングと長時間再生編集 |

変更に応じた検査の入口は `make verify` であり、対応規則は `scripts/verification.json` に一元化する。
開始時の情報取得は `make agent-context` にまとめる。
検査の正本を共有する要約と、ハッシュ付きの原文読込は [エージェントの手順](../agent-workflow.md) を参照する。
実行予定の環境事前確認、依存関係付きの優先順、失敗検出遅延の記録は同じ実行器に置く（[ADR 0166](../decisions/0166-verification-failure-detection.md)）。
呼出しごとの実行時間と再利用、契約・反例の件数と段階別の時間を診断用に残す。
テスト製品と同一反例のビルドだけを共用し、契約ごとの隔離・期限・判定を保つ（[ADR 0195](../decisions/0195-verification-cost-observability.md)）。
契約と橋渡しの入力は Cargo のライブラリ・別ターゲットを区別し、通常テストと反例の証拠を独立に再利用する。
製品のリンク先変更は両段階を失効させ、呼出し元のフックにも直列予算を確保する（[ADR 0197](../decisions/0197-verification-target-scopes.md)）。
作業ごとの `make verify` と、サニタイザ・全反例を含む節目の `make verify-full` に段階を分ける（[ADR 0212](../decisions/0212-verification-tiers.md)）。
診断・表示と製品の証拠入力は分離し、契約の起動コストと同一反例・TSan のビルドを共有する。
実テストの独立プロセス・期限・全反例の判定は維持する（[ADR 0198](../decisions/0198-verification-launch-and-cache-cost.md)）。
個別コマンドの記録補助は終了コードを集約するだけで、別の選択・キャッシュ・合格経路を持たない。
リアルタイム処理の規則はデバッグビルドのメモリ確保検出と `make rtsan` で機械的に守らせる。

## 17. ビルドと配布

- **ツールチェーンと最低 OS**: Xcode 27（Swift 6.4、macOS 27 SDK）と Rust stable。ターゲットは `aarch64-apple-darwin`（最低 OS `27.0`）、対象 CPU は M4 ファミリー以降（`apple-m4`）。
  Swift は `-target-cpu apple-m4` と `-Xcc -mcpu=apple-m4` を対で指定する
  （[ADR 0112](../decisions/0112-m4-minimum.md)、[ADR 0114](../decisions/0114-m4-build-baseline.md)）
- **XCFramework と XcodeGen**: Rust を静的ライブラリとして XCFramework にまとめ、`project.yml` から XcodeGen でプロジェクトを生成する（`.xcodeproj` は手編集しない）
- **署名と配布**: 開発でも配布でも自己署名証明書 `Echode Local` で署名し、マイク等の TCC 許可を維持する（証明書未作成時の `make app` のみアドホック署名）。
  配布バンドルは `Echode.app/Contents/MacOS/Echode`、`Contents/Helpers/EchodePluginHost.app`（`LSUIElement`、このヘルパーのみ library validation 無効化）、`Contents/MacOS/echode-cli`、`Contents/Frameworks/EchodeFFI.xcframework` で構成する
  （[ADR 0061](../decisions/0061-distribution-signing.md)）
- **GPL-3.0-or-later**: リリースごとに同一版のソースを公開し、同梱ライブラリのライセンス表記をアプリ内から読めるようにする。
  任意のプラグインを読み込むため App Sandbox は使わない
