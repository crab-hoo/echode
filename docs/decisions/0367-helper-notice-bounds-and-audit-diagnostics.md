# ADR 0367: ヘルパー通知の集約・上限制限と診断・状態同期の補強

- 日付: 2026-11-15
- 状態: 現行
- 領域: プラグイン・隔離
- 関連: [DIA-10・DIA-12](../reviews/2026-11-13-detailed-implementation-audit.md#dia-10-ヘルパーの通知の取り出し)、[ADR 0318](0318-split-control-messages.md)、[ADR 0348](0348-background-state-capture-retries.md)、[ADR 0115](0115-phase2-mcp.md)

## 問題と根拠

詳細実装監査の保守性・堅牢性指摘として、次の 5 境界に蓄積・畳み込み・不整合があった。

1. `ControlState::poll_notice`（`echode-sandbox/src/session.rs`）は `Dirty`・`PluginRestarted`・未使用の `RequestProcess` をすべて `inbox` へ積み、`poll_state_save` は 1 回のポーリングで先頭 1 件だけを取り出して `Dirty` 以外を捨てていた。`drain` も連続送信に対して制限なく回り続け、`try_recv_tagged` の「1 回 1 枠まで」を外していた（DIA-10）。
2. `resolve_cached`・`refresh_catalog_within`・`restore_cached`（`echode-plugin-host`）は `load_scan_cache` の失敗をすべて空の走査結果へ畳み、破損や権限エラーでもログを残さなかった（DIA-12）。
3. `Session::perform_requested`（`echode-audio-io/src/session.rs`）の `Pause`・`Stop` は `stream.pause()?` / `stream.rewind()?` の失敗時に `playing = false` にしたまま `sync_activity()` を通らず抜け、`NSProcessInfo` の活動トークンが残っていた（DIA-12）。
4. `McpSession::legacy_ids`（`echode-api/src/mcp.rs`）は受信した文字列化 ID を全件 `BTreeSet<String>` へ保持し続け、長い接続で上限なく増えていた（DIA-12）。
5. `Socket::recv_tagged`（`echode-sandbox/src/control.rs`）は両分岐ともループ先頭へ戻るだけの条件 `if !poll_read(..)? && self.incoming.len() < 4 { continue; }` を持っていた（DIA-12）。

## 比較と決定

### ヘルパー通知の分離と上限制限（DIA-10）

1. `poll_state_save` 内で `poll_notice()` を複数回回して `Dirty` 以外を捨てる案: テスト用の `PluginRestarted` 通知と競合し、`inbox` と `drain` の無制限な成長も残る。
2. 未使用の `RequestProcess` をヘルパーから送らず `accept` でも落とし、`Dirty` を `dirty_notice: bool` へ、`PluginRestarted` を `restart_notices`（最大 64）へ分離し、`inbox`（最大 64）と 1 回の `drain`（最大 64 件）に上限制限を設ける。wire 列挙子 `ControlMessage::RequestProcess` 自体は既存ファザ互換のため残す。

2 を採る。`poll_state_save_at` は `drain` 後に `dirty_notice` を直接引き取るため、再開通知を捨てたり `20 ms * k` 遅れたりせず即時に 1 回へまとまり、連続送信中でも `drain` と `wait_reply` の期限が機能する。

### 走査キャッシュ・活動同期・MCP ID・ソケット待ち（DIA-12）

- **走査キャッシュ**: 起動直後や未スキャン時の再生・カタログ復旧を止めないため返り値は `Vec` のまま、`NotFound` とそれ以外の読取り失敗を共通補助 `load_existing_scan_cache` で分け、破損・権限エラー等の失敗時だけ `echode_plugin_host::scan` へキャッシュのパスと原因を警告記録する。
- **停止・一時停止の活動同期**: `Session::perform_requested` が本体を `run_command` へ分け、成否を問わず復帰直前に `sync_activity()` を必ず実行する。
- **MCP 旧 ID 集合**: 型表現と既定契約 `mcp-legacy-lifecycle`（`self.legacy_ids.contains(&id.to_string())`）を保ったまま、挿入順 FIFO で最新 `4096` 件を保持する `LegacyIds` に置き換える。
- **ソケット待ち**: 無意味な `if` を外し `let _ = poll_read(self.fd.as_raw_fd(), remaining)?;` とする。

## 回帰と基盤

- `echode-sandbox/src/session_tests.rs`: 既存の活動分離テストを統合後の仕様へ合わせ、`audit_dia10_*` で未使用通知の破棄・再開通知前の即時合算・`inbox`/`restart_notices`/`drain` の有界性を検査。`control_wait_tests.rs` に部分ヘッダの時間切れ回帰を追加。
- `echode-plugin-host/src/scan.rs`: `resolve_cached_logs_corrupt_or_unreadable_cache_but_not_absent` を追加。
- `echode-audio-io/src/session_tests.rs`: `audit_dia12_failed_pause_and_stop_still_clear_playing_activity` で実出力の失敗経路から `activity` 解放を検査。
- `echode-api/src/mcp_tests.rs`: `audit_dia12_legacy_ids_stay_bounded_across_many_requests` で FIFO 上限と再使用拒否を検査。

設計書・監査・デバッグ手順を更新する。実機確認や新しい fuzz 入口の追加は要らない。
