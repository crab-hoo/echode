# ADR 0389: CLAP の音声構成の一覧は処理器を止めずに主スレッドで読む

- 日付: 2026-11-16
- 状態: 現行
- 領域: プラグイン・隔離
- 対象: P2-03・F2・FCA-07 の `clap.audio-ports-config` と `audio-ports-config-info` の一覧読込み
- 関連: [ADR 0384](0384-clap-audio-port-config-selection.md)、[ADR 0385](0385-clap-configurable-audio-ports-and-config-info.md)、[ADR 0386](0386-device-audio-configuration-controls.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)

## 背景と再現

開始時の HEAD は `2b0d007`。
ADR 0384 は、CLAP のヘッダが一覧の走査も無効化中に限ると読み、一覧の要求でも処理器を止めて再有効化していた。
上流の [audio-ports-config.h](https://github.com/free-audio/clap/blob/main/include/clap/ext/audio-ports-config.h) の注記は次のとおりで、この読み方は誤りだった。

- `clap_plugin_audio_ports_config.count`・`get`: `[main-thread]`
- `clap_plugin_audio_ports_config.select`: `[main-thread & plugin-deactivated]`
- `clap_plugin_audio_ports_config_info.current_config`・`get`: `[main-thread]`
- `clap_host_audio_ports_config.rescan`: `[main-thread]`
- [configurable-audio-ports.h](https://github.com/free-audio/clap/blob/main/include/clap/ext/configurable-audio-ports.h) の `can_apply_configuration`・`apply_configuration`: `[main-thread & !active]`

デバイス列の「音声構成を読み込む」を押すだけで、再生中の音が途切れていた。
音切れは ADR 0276 で堅牢性に数え、機能の追加より先に直す。

検査用プラグインの一覧系の callback から無効化の要求を外し、主スレッドの確認だけを残してから、2 つの回帰を先に足した。
修正前は次のとおり失敗した（期待終了コード 101 と固有のアサーションを照合。証拠は `target/focused-checks/audio-config-list-red/`）。

- `isolation/audio_ports.rs::reading_audio_configs_keeps_the_plugin_active_under_rtsan`: 3 回の一覧で有効化の回数が 1 から 4 へ増え、状態が `[7, 4, 8, 0]` になった。
- `hosted_playback/audio_ports.rs::reading_audio_configs_while_playing_keeps_every_block_under_rtsan`: 製品のデバイス制御から一覧を読む間、別スレッドの再生が最初のブロックから無音になった。

## 比べた案と決定

- **無効化を続け、ボタンの説明だけを直す**: 不採用。仕様が求めない音切れを利用者に払わせる。
- **一覧を起動時に読んで保持する**: 不採用。ADR 0384 と同じく、保持・改訂・`rescan` の同期の規則が増える。
  中断がなくなっても、明示した要求ごとに最新を読む方が単純で、古い一覧を出さない。
- **一覧の要求を通常の主スレッド要求にする**: 採用。
  `MainRequest::requires_inactive` から `AudioConfigs` を外す。制御側は再起動の世代を進めず、主ループが処理器を動かしたまま要求を取る。
  `select`・`can_apply`・`apply` は従来どおり無効化中だけに実行し、再有効化でポートを読み直す。

再起動中に届いた一覧の要求は、既存の `restart_on_main` が無効化中に処理してもよい。どちらの状態でも `[main-thread]` を満たす。
一覧の読込みはポートを変えないため、`PortsChanged` も計画の交換も起きない。音声側の処理・確保・ロックは変えない。
性能の改善は目的でなく、時間の比較計測はしない。

## 画面と明示読込み

ADR 0386 は「列の表示で一覧を読むと音声を中断する」ことを、明示ボタンを選んだ理由にしていた。
中断はなくなったが、明示読込みは保つ。
表示や Undo のたびにプラグインの主スレッドのコードと制御の往復を走らせると、背景の状態取得と同じ受付を取り合い busy の拒否が増える。
一覧は publication ごとに破棄するため、暗黙に読んでも次の編集でまた読み直しになる。
ボタンの説明は「選択と変更で中断し、一覧の読込みでは中断しない」に改めた。

## 回帰と基盤

- 必須契約 `plugin-audio-config-live-list` に製品経路の回帰を登録し、反例は `requires_inactive` へ `AudioConfigs` を戻す変更とした。
- `main_requests_tests.rs` は一覧の要求が有効なままの `take(false)` で取れることを確かめる。
- 既存の選択・任意変更・文書の回帰は、一覧を有効なまま読んでも同じ値・同じ履歴になることを確かめる。
- 検査用プラグインは一覧系で主スレッドだけを確かめ、選択・任意変更では無効化中も確かめる。
- 新しい検査コマンド・ビルド構成・fuzz 対象は増やさない。制御ソケットの形式は変わらない。

## 残る範囲

第三者 CLAP が有効なままの一覧に正しく答えるか、読込み中に音が途切れないかは UC-97 でユーザーが確かめる。
選択・任意変更・変更可否の照会は CLAP の規則により引き続き音声を中断する。
独自のポート種別・詳細、曲からの表情生成、センド・サイドチェイン等は残り、P2-03・F2・FCA-07 全体を完了にはしない。
