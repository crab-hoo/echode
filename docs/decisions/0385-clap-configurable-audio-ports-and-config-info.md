# ADR 0385: CLAP の audio-ports-config-info と configurable-audio-ports を無効化中に接続する

- 日付: 2026-11-16
- 状態: 一部置換（[ADR 0388](0388-typed-clap-audio-port-details.md)）。標準のポート詳細は型付きで送る
- 領域: プラグイン・隔離
- 対象: P2-03・F2・FCA-07 の `clap.audio-ports-config-info` と `clap.configurable-audio-ports`
- 関連: [ADR 0384](0384-clap-audio-port-config-selection.md)、[ADR 0382](0382-audio-port-changes-exchange-mapping-and-routing.md)、[ADR 0199](0199-owned-helper-control-requests.md)

## 背景と再現

着手時の HEAD は `d95f8c4`。
[ADR 0384](0384-clap-audio-port-config-selection.md) で `clap.audio-ports-config` の構成一覧と ID 選択を制御 API へ接続したが、現在の構成 ID と各構成の個別ポート詳細を返す `clap.audio-ports-config-info`、およびホストから任意のポート構成変更を問い合せ・適用する `clap.configurable-audio-ports` が未接続だった。

検査用プラグインの `audio-ports` fixture に `CLAP_EXT_AUDIO_PORTS_CONFIG_INFO` と `CLAP_EXT_CONFIGURABLE_AUDIO_PORTS` を実装し、ホスト側が未対応（`current_id: None`・`ports: None`・`NoConfigurableAudioPorts`）の状態で実ヘルパーの隔離テスト `audio_ports::configurable_audio_ports_and_config_info_exchange_ports_under_rtsan` を実行した。
期待どおり終了コード 101、`current_id` の `left: None` / `right: Some(100)` で失敗した（`target/verification/scratch/red/1.log`）。

## 比べた案と決定

- **構成一覧の読取りで `audio-ports-config-info` を必須にする**: 不採用。`clap.audio-ports-config` だけを実装するプラグインの一覧取得まで壊す。
  `CLAP_EXT_AUDIO_PORTS_CONFIG_INFO`（`/1`）および互換 `/draft-0` の両方が無い時は `current_id: None`・`ports: None` を保ち、表がある時だけ `current_config` と `get` の欠落、未知の `current_config` ID、個別ポートと主ポート数の矛盾を型付きの `PluginFailure` で拒否する。
- **`configurable-audio-ports` の問い合せ（`can_apply`）も無効化中へ送る**: 採用。
  CLAP の [configurable-audio-ports.h](https://github.com/free-audio/clap/blob/main/include/clap/ext/configurable-audio-ports.h) は `can_apply_configuration`・`apply_configuration` の両方を無効化中の主スレッドへ限定する。既存の `MainRequests`（`requires_inactive()`）へ統合し、音声スレッドが処理器を返してから実行して再有効化する。
  `can_apply` が `false` を返した時は失敗ではなく `Ok(false)` を返し、`apply` が `false` を返した時だけ `ConfigurableAudioPortsRefused` を返す。`select` と同じく拒否後も共通の `reactivate` でポートを読み直して復帰する。
- **`port_type` と `port_details` の組み立て**:
  `AudioPortRequest { input, index, channels }` から `clap_audio_port_configuration_request` を作る際、1 チャンネルには `CLAP_PORT_MONO`、2 チャンネルには `CLAP_PORT_STEREO` を設定し、無効化・削除（`0`）および 3 チャンネル以上と `port_details` は `null` にする。サラウンド種別や任意の `port_details` を憶測しない。
- **ポート詳細の読取りの共通化**:
  `threads.rs::read_port_list` と `audio_configs.rs::read_config_info` はどちらも CLAP の `clap_audio_port_info` を走査して `channel_count == 0` と記述失敗を検査するため、`audio_ports.rs::read_port_channels` へ共通化した。事前確保の上限は 64 までに抑えつつ実数には固定上限を設けず、異常なポート数申告による即時の過大確保を避ける。
- **compat 識別子の扱い**:
  `extension::table_with_compat` で正規 ID を先に引き、無ければ `_COMPAT`（`audio-ports-config-info/draft-0` と `configurable-audio-ports.draft1`）を引く。

## 所有権・IPC・検証境界

- 内部 IPC の制御メッセージへ `CanApplyAudioPorts`・`AudioPortsApplicable`・`ApplyAudioPorts`・`AudioPortsApplied`（型番号 `44..=47`）を追加し、`AudioPortConfigs { current_id, configs }` と `AudioPortConfig.ports: Option<PortLayout>`、`PluginFailure` コード `31..=36` を加えた。
  受信側の配列予約はフレーム残量で抑え、欠損フラグや不正 UTF-8・余分な末尾バイトを拒否する。
- `PluginControl` に `can_apply_audio_port_configuration` と `apply_audio_port_configuration` を追加し、`apply` 成功時は設定変更を dirty に記録し、応答より先に `PortsChanged` を送る。
- 共有領域とルーティングの切替は [ADR 0382](0382-audio-port-changes-exchange-mapping-and-routing.md) の計画交換をそのまま使い、音声スレッドにロック・確保・I/O・システムコール・時計を足さない。

## 回帰と残課題

- 単位回帰（`echode-sandbox`・`echode-plugin-host::audio_configs_tests` の v1 / draft 互換と異常系、`echode-fuzz` の制御メッセージ境界）、実ヘルパーの隔離回帰（`isolation/audio_ports.rs`）、製品エンジンの RTSan 回帰（`hosted_playback/audio_ports.rs`）を通した。
- 必須契約 `plugin-configurable-audio-ports`・`plugin-configurable-port-routing` と反例を登録した。
- 構成メニュー UI・選択状態のプロジェクト保存・特殊な `port_details` 指定・曲からの表情生成等は残り、P2-03・F2・FCA-07 全体は完了としない。
- 第三者 CLAP の `/1` と互換 draft の解釈・選択変更中の聴感は自動 fixture で判定できないため、UC-98 に残す。
