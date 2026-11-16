# ADR 0391: センドとサイドチェインをモデル・保存・API・PDC の計画へ接続する

- 日付: 2026-11-16
- 状態: 現行
- 領域: モデル・履歴・保存・API
- 関連: [ADR 0129](0129-track-duplication.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)、
  [ADR 0376](0376-typed-changeset-ownership.md)、[ADR 0390](0390-routes-in-the-latency-graph.md)、
  [操作要件](../interaction-design.md#センドとレシーブ)、[P2-03](../phase2.md#p2-03-ルーティングフォルダバスpdc)

## 問題

開始時の HEAD は `ddd829e`。
ADR 0390 でセンドとサイドチェインを遅延グラフの `Routes` として解く土台はできたが、
ドキュメントモデル・パッケージ保存・ジャーナルと Undo・アクション・JSON-RPC / MCP・Swift 編集型には経路の表現がなく、
再生エンジンの `compensate_plan` もフォルダの親子だけを `Routes` に渡していた。

P2-03 を安全に進めるため、コールバックの音声加算や画面を足す前に、
経路の所有関係・循環禁止の不変条件・保存形式・編集と問合せ API・PDC への投影を 1 つの整合した層として接続する。

## 比べた案と決定

### 1. 所有関係と表現

1. **プロジェクト直下に全経路の単一配列を置く**:
   トラックやデバイスの削除・複製・差分保存のたびに別表を横断して掃除する必要があり、所有の漏れが起きやすい。
2. **送り元トラックが `SendRoute` を、受け側 CLAP デバイスが `SidechainRoute` を所有する（採用）**:
   - `Track::sends: Vec<SendRoute>`（`ObjectKind::Send`、`Ownership::TRACK_SENDS`、`Property::SendOrder`）
   - `Device::sidechains: Vec<SidechainRoute>`（`ObjectKind::Sidechain`、`Ownership::DEVICE_SIDECHAINS`、`Property::SidechainOrder`。ミキサーは空のみ許可）
   - 取り出し位置は `RouteTap::PostFxPreFader`（`"post_fx_pre_fader"`）、既定ゲインは `1.0`（0 dB、有効範囲は `-0.0` を除く有限値 `0.0..=GAIN_MAX`、入力の `-0.0` は `0.0` に正規化）。
   - 1 つの接続 ID を送り元・受け側の両方から参照でき、デバイス削除時は所有するサイドチェインも逆順に削除変更を出して単一 Undo で復元できる。

### 2. 循環と重複の検証をモデルの不変条件へ集約する

ADR 0276 に従い、循環や欠落の判定を API やエンジンごとに別実装しない。
`Project::check_invariants`（および経路・親子・デバイス複製操作）で、信号が流れる有向辺

- フォルダバス: `child -> parent`
- センド: `source -> target`
- サイドチェイン: `source -> device の所属 track`

の和集合に対する閉路を非再帰 DFS で検出し、`ModelError::Cycle`（組立て時は `InvariantError::RouteCycle`）で拒否する。
並列経路（DAG）は許可し、同一 `(source, target)` の重複センドと同一 `(device, source)` の重複サイドチェインは拒否する。

### 3. トラック複製とデバイスコピーの意味

- `device.copy`: サイドチェインの `source` と `gain` を引き継ぎ、新しいサイドチェイン ID をコピー先デバイス ID と元 ID から決定論的に導出し、循環検査を通す。
- `track.duplicate`: 複製対象の部分木（自身と子孫）の内部を指す経路は対応する複製先トラックへ張り替え、部分木の外を指す経路は元の相手を保ったまま新しい ID を導出する。

### 4. パッケージスキーマ 7 と旧ジャーナル互換

- パッケージの `SCHEMA` を `6` から `7` へ進め、`tracks/<id>.json` のトラックに `sends`、各デバイスに `sidechains` を保存する。
  スキーマ 6 以前のパッケージは段階変換 `add_route_fields` で空配列を補う。浮動小数点は既存の `PackageFormatter` で無損失表記を保つ。
- 変更セットの `TrackState`・`DeviceState` では `SendOrder`・`SidechainOrder` を省略時に空配列とする（`field_optional_ids`）ことで、スキーマ 6 以前のジャーナルもそのまま再生できる。

### 5. アクション・API・PDC への投影

- アクション登録簿と Swift 生成型（`EditAction.swift`）へ `send.add`・`send.set_gain`・`send.remove`・`sidechain.add`・`sidechain.set_gain`・`sidechain.remove` を追加する。
- JSON-RPC に `project.routing`、MCP に読み取り専用ツール `echode.project.routing` とリソース `echode://project/routing` を追加する。
- `echode-engine` の `Prepared::new`（`track_routing`）でスナップショットのセンドとオンライン CLAP デバイスのサイドチェインを計画順の位置へ解決して保持し、
  `compensate_plan` が `Routes { tracks, sends, sidechains }` へ投影する。
  キャッシュ再生中のトラックや未ホスト・オフラインのデバイス宛てのサイドチェインは、`TrackFlow::devices` に現れないため安全に省略される。
  プラグインなしの初期 `playback()` でも `compensate_plan` を通し、余韻長の変化も `compensation_changes` に反映する。

## 残る範囲

- コールバックでのセンド音声の加算、CLAP 補助入力ポートへのサイドチェイン音声の供給、実行順序の依存解決
- ミュート・ソロとセンド／リターン開放の連動、ドラム用の MIDI 配布、GUI のルーティング操作
