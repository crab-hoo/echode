# API・MCP・文書の入口の制約

[現在の範囲と制約](../status.md) の領域別の正本。
実機等の確認待ちは [ユーザー確認の一覧](../user-checks.md) の UC-番号で示す。

## API・MCP・文書の入口

- **API の応答の書込み**: 応答の書込みが 30 秒進まない接続は閉じる。送信失敗の前に成立した編集は再実行しない（[ADR 0073](../decisions/0073-socket-path.md)）。
- **MCP の応答**: 版確認後はサイズ上限・読取り期限がなく、無応答なら待ち続ける。
  不正応答は結果不明として確認を促し、確定した編集を再送・巻戻ししない（[ADR 0365](../decisions/0365-mcp-complete-session-replies.md)）。
- **文書の寿命と古い入力（SPG-02）**: 閉じる間はヘルパーの終了を待たず、文書なしの画面を `busy` のまま表示する（[ADR 0203](../decisions/0203-document-session.md)）。
  実画面での文書切替・VoiceOver・聴感はユーザー確認待ち（UC-29）。
- **文書の手番の調停（SPG-05）**: CLI と MCP はこのサービスを使わず、`DocumentHost` へ直接つなぐ（[ADR 0204](../decisions/0204-document-service.md)）。
- **Swift の型付き編集入口（SPG-03）**: 互換の raw JSON の入口が残る（[ADR 0202](../decisions/0202-generated-swift-edit-actions.md)）。
- **製品のログ**: 異常終了で待ち行列の残りは失われ、ヘルパーは終了時の stderr の末尾だけ。
  自動保存停止は `history.usage`、使用量ログは終了時。実アプリは UC-66（[ADR 0322](../decisions/0322-product-log.md)）。
