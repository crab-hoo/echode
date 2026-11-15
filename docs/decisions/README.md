# ADR 索引

<!-- scripts/documentation.py --write-adr-index で自動生成。直接編集せず、各 ADR の `- 状態:` / `- 領域:` を更新する。 -->

全 383 件の設計判断を領域別・状態別に並べる。日々の作業で現在有効な制約を確かめる時は「現行・一部置換」を見る。

## 集計

| 領域 | 合計 | 現行 | 一部置換 | 置換済み | 草案 |
|---|---:|---:|---:|---:|---:|
| [全体方針・計画](#全体方針計画) | 12 | 10 | 2 | 0 | 0 |
| [モデル・履歴・保存・API](#モデル履歴保存api) | 82 | 73 | 8 | 0 | 1 |
| [音声エンジン・書き出し](#音声エンジン書き出し) | 103 | 74 | 27 | 2 | 0 |
| [プラグイン・隔離](#プラグイン隔離) | 70 | 57 | 13 | 0 | 0 |
| [UI・描画・入力](#ui描画入力) | 62 | 54 | 8 | 0 | 0 |
| [ビルド・検査・開発基盤](#ビルド検査開発基盤) | 54 | 38 | 16 | 0 | 0 |
| 合計 | 383 | 306 | 74 | 2 | 1 |

## 全体方針・計画

### 現行・一部置換

| ADR | 状態 | 表題 |
|---|---|---|
| [0001](0001-phase0-start.md) | 一部置換（[0004](0004-license-gpl.md)） | 計画の承認と Phase 0 の開始 |
| [0002](0002-s1-confirmed.md) | 現行 | S1 の確定と S2 への進行 |
| [0003](0003-audio-thread-does-not-wait.md) | 現行 | オーディオスレッドはワーカーを待たない |
| [0004](0004-license-gpl.md) | 現行 | ライセンスを GPL-3.0-or-later に変える |
| [0005](0005-s2-confirmed-s4-next.md) | 現行 | S2 の確定と履歴の検証 |
| [0006](0006-s4-confirmed-s5-next.md) | 現行 | S4 の確定と入力の検証 |
| [0007](0007-s5-confirmed-phase1.md) | 現行 | S5 の確定とズームの選び方 |
| [0014](0014-existing-implementations-as-options.md) | 現行 | 既存の実装を選択肢として取り込む |
| [0117](0117-workflow-interactions.md) | 現行 | 制作ワークフローに沿って P2 の操作と状態を分ける |
| [0276](0276-priority-order-and-delegated-decisions.md) | 一部置換（[0277](0277-reaper-style-folder-bus.md)・[0281](0281-calm-transparent-failure.md)） | 優先順位を定め、判断をエージェントへ委ねる |
| [0278](0278-audit-backlog-order.md) | 現行 | 監査の残りの順番と、FCA-05 の区切り |
| [0281](0281-calm-transparent-failure.md) | 現行 | 障害は静かに安全側へ倒し、起きたことと行ったことを隠さない |

## モデル・履歴・保存・API

### 現行・一部置換

#### ADR 0008〜0201

| ADR | 状態 | 表題 |
|---|---|---|
| [0008](0008-actions-undo-journal.md) | 現行 | アクションの Undo とジャーナル |
| [0009](0009-package-autosave.md) | 一部置換（[0033](0033-journal-log.md)） | プロジェクトの保存とジャーナルの基点 |
| [0017](0017-folder-tracks.md) | 一部置換（[0024](0024-track-order.md)） | フォルダは親の ID で表す |
| [0020](0020-json-rpc.md) | 一部置換（[0030](0030-action-catalog.md)・[0034](0034-api-transaction.md)） | JSON-RPC は本人だけのソケットでアクションを受ける |
| [0021](0021-changeset.md) | 現行 | 操作の結果は ChangeSet として残す |
| [0022](0022-clip-content-and-placement.md) | 一部置換（[0117](0117-workflow-interactions.md)・[0170](0170-content-relative-auto-bars.md)・[0196](0196-lossless-package-floats.md)） | クリップは中身と配置に分ける |
| [0023](0023-note-order.md) | 現行 | ノートは開始、音高、ID の順に持つ |
| [0024](0024-track-order.md) | 一部置換（[0127](0127-positioned-track-insertion.md)） | トラックの並びは兄弟ごとの分数インデックスで持つ |
| [0026](0026-parameters.md) | 現行 | パラメータを置き、フェーダーもその 1 つにする |
| [0027](0027-note-expression.md) | 現行 | ノートのベロシティは 16 ビットで、表情は別の配列に置く |
| [0028](0028-package-schema.md) | 現行 | パッケージは版を順に上げ、出力の形はスナップショットで固定する |
| [0029](0029-invariants.md) | 現行 | 不変条件は 1 つの関数で検査する |
| [0030](0030-action-catalog.md) | 現行 | アクションの ID は登録簿の 1 つだけにする |
| [0031](0031-content-addressed-store.md) | 現行 | プラグインの状態は内容のハッシュで保管する |
| [0032](0032-document-host.md) | 現行 | ドキュメントは 1 つのスレッドが持つ |
| [0033](0033-journal-log.md) | 現行 | ジャーナルは追記だけのログにする |
| [0034](0034-api-transaction.md) | 現行 | API で複数の操作を 1 回の Undo にする |
| [0046](0046-history-thin.md) | 一部置換（[0270](0270-gix-0-90-and-github-advisories.md)・[0368](0368-opt-in-history-thinning-feature.md)） | 自動保存は残すコミットだけをつなぎ、パックにまとめる |
| [0054](0054-history-restore.md) | 現行 | 自動保存した版を新しい版として戻す |
| [0057](0057-crash-recovery.md) | 現行 | 開いた時に自動保存の基点を作り、書き出しは置いた音声を混ぜる |
| [0059](0059-clip-fx.md) | 現行 | クリップごとに独立した FX チェインを持つ |
| [0062](0062-package-single-writer.md) | 現行 | パッケージの書き手は一つにし、切れたジャーナル末尾は確定した改行まで戻す |
| [0071](0071-clip-overlap-index.md) | 一部置換（[0117](0117-workflow-interactions.md)） | コールバックは重なる配置だけを選ぶ |
| [0073](0073-socket-path.md) | 現行 | ソケットのパスは種類を見てから片付ける |
| [0074](0074-package-entry-io.md) | 現行 | パッケージ内のファイル操作を、保持した親ディレクトリから行う |
| [0083](0083-history-commit.md) | 現行 | 失敗した編集は履歴を複製せずに戻す |
| [0084](0084-note-diff.md) | 現行 | ノートの差分は変わった中身だけを見る |
| [0086](0086-checked-time.md) | 現行 | 時刻の境界は検査してから拒否する |
| [0115](0115-phase2-mcp.md) | 一部置換（[0365](0365-mcp-complete-session-replies.md)） | Phase 2 を開始し、既存セッションへ最小 MCP を接続する |
| [0116](0116-folder-drum-rack.md) | 現行 | ドラムラックはフォルダトラックで表す |
| [0124](0124-selected-track-fx-batches.md) | 現行 | 選択トラックへの FX 一括追加を単一の編集として保存する |
| [0127](0127-positioned-track-insertion.md) | 現行 | 選択位置への空トラック挿入と末尾追加を分ける |
| [0129](0129-track-duplication.md) | 現行 | トラックの通常複製を部分木単位の独立コピーにする |
| [0170](0170-content-relative-auto-bars.md) | 現行 | 自動クリップの小節は中身の原点と一緒に動く |
| [0180](0180-journal-replay-invariants.md) | 現行 | 復旧時のジャーナルは各行の不変条件を検査してから受け入れる |
| [0182](0182-noop-edits.md) | 現行 | 無変更の編集を成功として受け、履歴を増やさない |
| [0183](0183-recovery-consent.md) | 現行 | 古い保存版への復旧を通知し、同意まで編集を止める |
| [0196](0196-lossless-package-floats.md) | 現行 | パッケージの浮動小数点を精度を落とさず保存する |
| [0200](0200-shared-model-units-and-limits.md) | 現行 | モデルの時間単位と値の範囲を Swift と共有する |
| [0201](0201-integer-loop-drafts.md) | 現行 | ループの UI 下書きを整数で共有する |

#### ADR 0202〜0376

| ADR | 状態 | 表題 |
|---|---|---|
| [0202](0202-generated-swift-edit-actions.md) | 現行 | アクション登録簿から Swift の編集型を生成する |
| [0203](0203-document-session.md) | 現行 | 文書の寿命の状態を DocumentSession へまとめ、文書の照合を 1 か所にする |
| [0204](0204-document-service.md) | 現行 | 文書の手番の調停を FFI から文書サービスの crate へ移す |
| [0214](0214-parameter-changeset-fields.md) | 現行 | パラメーターの変更セットを項目表から生成する |
| [0215](0215-device-changeset-fields.md) | 現行 | デバイスの変更セットを項目表から生成する |
| [0216](0216-note-changeset-fields.md) | 現行 | ノートの変更セットを項目表から生成する |
| [0217](0217-track-changeset-fields.md) | 現行 | トラックの変更セットを項目表から生成する |
| [0218](0218-content-changeset-fields.md) | 現行 | 中身の変更セットを項目表から生成する |
| [0219](0219-placement-changeset-fields.md) | 現行 | 配置の変更セットを項目表から生成する |
| [0220](0220-tempo-changeset-fields.md) | 現行 | テンポ点の変更セットを項目表から生成する |
| [0221](0221-project-changeset-fields.md) | 現行 | プロジェクトの変更セットを項目表から生成する |
| [0222](0222-parameter-package-fields.md) | 現行 | パラメーターの項目表をモデルと保存へ共有する |
| [0223](0223-device-package-fields.md) | 現行 | デバイスの項目表をモデルと保存へ共有する |
| [0224](0224-note-package-fields.md) | 現行 | ノートの項目表をモデルと保存へ共有する |
| [0225](0225-track-package-fields.md) | 現行 | トラックの項目表をモデルと保存へ共有する |
| [0226](0226-model-field-registry-completion.md) | 現行 | モデル項目の表を保存・ジャーナル・スナップショットまで共有する |
| [0230](0230-save-durability.md) | 現行 | 保存の書出しを同期してからジャーナルを短縮し、欠けた再生は同意へ送る |
| [0234](0234-typed-journal-durability.md) | 現行 | ジャーナルの不確定を文字列でなく型で判定する |
| [0242](0242-model-content-lookup-and-references.md) | 現行 | 内容の二分探索と参照の一括照合 |
| [0252](0252-bounded-journal-tail-repair.md) | 現行 | ジャーナルの追記前確認を末尾から行う |
| [0286](0286-history-usage-diagnostics.md) | 現行 | Undo の段が持つメモリを測って診断へ出し、予算は写す単位と比べてから決める |
| [0287](0287-shared-note-runs.md) | 現行 | 中身のノートを共有の run に分け、Undo の段が編集した分だけを持つ |
| [0292](0292-typed-document-errors.md) | 現行 | 文書の手番のエラーを列挙型にする |
| [0296](0296-typed-storage-errors.md) | 現行 | 履歴 crate の保管庫・復号のエラーを先に型にする |
| [0297](0297-typed-autosave-and-git-errors.md) | 現行 | 自動保存の待ち手へ元のエラーを共有して返す |
| [0298](0298-typed-package-validation-errors.md) | 現行 | スキーマとパッケージの拒否を原因の型で返す |
| [0299](0299-typed-document-journal-and-settings-errors.md) | 現行 | 文書・ジャーナル・設定の失敗を型で保持する |
| [0300](0300-typed-thinning-errors.md) | 現行 | 間引き・パック・バンドルの失敗を型で保持する |
| [0303](0303-typed-model-invariant-errors.md) | 現行 | モデルの並びと不変条件の原因を型で保つ |
| [0304](0304-typed-api-errors.md) | 現行 | API・MCP の原因を応答の直前まで型で保つ |
| [0305](0305-typed-cli-errors.md) | 現行 | CLI の原因を表示まで型で保持する |
| [0306](0306-typed-action-parameters.md) | 現行 | アクションの引数の原因を表示文から分類しない |
| [0307](0307-typed-document-command-decoding.md) | 現行 | 文書の単一編集も型付きで直接 JSON から読む |
| [0350](0350-unreadable-history-is-not-empty.md) | 現行 | 読めない履歴を空の履歴として開かない |
| [0353](0353-shared-placement-runs.md) | 現行 | トラックの配置もノートと同じ共有の run に分ける |
| [0354](0354-document-acquisition-diagnostics.md) | 現行 | 文書の作成・オープンの失敗を同期の借用 callback で渡す |
| [0360](0360-background-recording-retry-policy.md) | 現行 | 背景の読取りと記録の再試行を共通化する |
| [0363](0363-invalidate-failed-autosave-cache.md) | 現行 | 失敗した自動保存のキャッシュを次へ渡さない |
| [0365](0365-mcp-complete-session-replies.md) | 現行 | MCP は検証済みセッションの完全な応答を待つ |
| [0376](0376-typed-changeset-ownership.md) | 現行 | 変更セットの所有関係を種類の表で宣言し、種類の追加をコンパイルで止める |

#### ADR 0377〜0377

| ADR | 状態 | 表題 |
|---|---|---|
| [0377](0377-overlap-independent-auto-placement.md) | 現行 | 自動配置の探索と参照更新を重ならない窓から切り離す |

### 置換済み・草案

| ADR | 状態 | 表題 |
|---|---|---|
| [0169](0169-clip-growth-follows-notes-and-stops-at-walls.md) | 草案（[0170](0170-content-relative-auto-bars.md)） | クリップはノートを追って伸び、同じトラックの隣のクリップで止まる |

## 音声エンジン・書き出し

### 現行・一部置換

#### ADR 0010〜0135

| ADR | 状態 | 表題 |
|---|---|---|
| [0010](0010-playback-samples.md) | 現行 | 再生位置はサンプルで持つ |
| [0011](0011-preview-cache.md) | 現行 | プレビュー用キャッシュを Phase 3 に入れる |
| [0012](0012-fader-after-cache.md) | 一部置換（[0039](0039-bus.md)） | フェーダーはキャッシュの後ろ |
| [0015](0015-output-device-follow.md) | 現行 | 出力デバイスは制御スレッドが追従する |
| [0018](0018-test-synth.md) | 現行 | テスト用シンセは wavesynth を使う |
| [0019](0019-offline-wav.md) | 一部置換（[0123](0123-explicit-sine-pad-plugin.md)・[0243](0243-float-wav-metadata.md)） | オフラインの書き出しはテスト用シンセで WAV にする |
| [0025](0025-clip-window-cuts-notes.md) | 現行 | 配置の窓の外のノートは鳴らさない |
| [0035](0035-playback-plan.md) | 現行 | 再生計画は制御スレッドが作り、オーディオスレッドはカーソルだけ進める |
| [0038](0038-device.md) | 一部置換（[0123](0123-explicit-sine-pad-plugin.md)） | 内蔵デバイスは処理、イベント、状態、遅れ、余韻を持つ |
| [0039](0039-bus.md) | 現行 | バスはチャンネル数を持たず、ステレオのパンはバランスにする |
| [0040](0040-libm.md) | 現行 | 音を決める計算は libm で行い、足す順を固定する |
| [0047](0047-product-audio.md) | 一部置換（[0097](0097-cancellable-retained-transport.md)） | 製品の音声出力とトランスポートを接続する |
| [0055](0055-loop-metronome.md) | 一部置換（[0117](0117-workflow-interactions.md)） | ループとメトロノームは再生の設定にする |
| [0056](0056-audio-import.md) | 現行 | 音声素材は正準 PCM として保管し、固定クリップにする |
| [0063](0063-audio-thread-plan-swap.md) | 現行 | 音声スレッドは用意した箱とイベント列を使い回す |
| [0064](0064-one-callback-deadline.md) | 現行 | 製品のコールバックは締め切りを一つだけ使う |
| [0067](0067-track-audio-into-effects.md) | 現行 | 配置した音声はトラックのエフェクトの前でシンセと合流する |
| [0068](0068-clip-gain.md) | 現行 | 配置のゲインは準備した音声に掛ける |
| [0069](0069-plugin-export.md) | 現行 | CLAP を含む書き出しは再生と同じチェインを使う |
| [0070](0070-export-recovered-snapshot.md) | 一部置換（[0089](0089-export-identity.md)） | 書き出しは文書ホストの復旧済み版を使う |
| [0077](0077-event-reserve.md) | 現行 | イベントバッファは長さとの差で広げる |
| [0079](0079-failed-export.md) | 現行 | 失敗したプラグインの書き出しは完成にしない |
| [0081](0081-event-sort.md) | 現行 | ブロックのイベントは作業領域で整列する |
| [0082](0082-callback-deadline.md) | 一部置換（[0283](0283-output-time-callback-deadline.md)） | コールバックの期限は入口で決める |
| [0087](0087-callback-shutdown.md) | 現行 | 完了コールバックからの停止は自分を待たない |
| [0089](0089-export-identity.md) | 一部置換（[0090](0090-reaudit-playback.md)） | 書き出しの成功は版と連番を完成後に示す |
| [0090](0090-reaudit-playback.md) | 一部置換（[0283](0283-output-time-callback-deadline.md)） | 再監査の再生・状態・期限 |
| [0095](0095-product-audio-buses-and-continuity.md) | 現行 | 製品の音声バスと連続性を接続する |
| [0097](0097-cancellable-retained-transport.md) | 現行 | 再生準備の取消と出力の再利用 |
| [0101](0101-live-output-discontinuities.md) | 一部置換（[0102](0102-exclusive-audio-graph.md)・[0103](0103-live-stop-waveform.md)） | 定期保存確認とライブ出力の不連続を分ける |
| [0102](0102-exclusive-audio-graph.md) | 現行 | 再生中の音声グラフを制御側へ貸さない |
| [0103](0103-live-stop-waveform.md) | 一部置換（[0133](0133-stopped-plugin-processing-and-edit-input.md)） | 停止では最後の値ではなく鳴っている波形を減衰する |
| [0104](0104-stop-output-diagnostics.md) | 現行 | 停止クリックの再報告を実出力の記録で切り分ける |
| [0105](0105-nonblocking-stop-cleanup.md) | 一部置換（[0133](0133-stopped-plugin-processing-and-edit-input.md)） | 停止後の破棄する処理を音声の期限で待たない |
| [0106](0106-natural-plugin-onsets.md) | 現行 | 自然なプラグインの発音を直接出力と比較する |
| [0108](0108-phase1-loop-readiness.md) | 現行 | P1のループ先頭を準備済みPCMで保持し、トラックの差し替え口を接続する |
| [0109](0109-placement-bounds-and-plugin-capture.md) | 現行 | 配置の原点、プラグイン状態の取り込み、クリップゲインの上限、停止中のキャッシュ範囲 |
| [0110](0110-apple-silicon-draw-and-peaks.md) | 一部置換（[0112](0112-m4-minimum.md)） | Apple Silicon 向けの描画周期、波形バッファ、ピーク読出、稼働表明 |
| [0133](0133-stopped-plugin-processing-and-edit-input.md) | 現行 | ライブ停止中もプラグインを処理し、通常編集で画面全体を無効にしない |
| [0135](0135-sample-audition.md) | 一部置換（[0143](0143-browser-click-and-audition-timing.md)・[0151](0151-streaming-sample-audition.md)・[0152](0152-browser-audition-controls.md)） | サンプルの単発試聴を文書ワーカーから独立した出力で鳴らす |

#### ADR 0136〜0280

| ADR | 状態 | 表題 |
|---|---|---|
| [0136](0136-output-opening-off-worker.md) | 現行 | 出力とヘルパーの準備を文書ワーカーの外で行う |
| [0147](0147-output-prewarm-and-background-media.md) | 現行 | プロジェクトを開いた時に出力を準備し、停止中の編集の素材を別スレッドで読む |
| [0151](0151-streaming-sample-audition.md) | 一部置換（[0163](0163-sample-overview.md)） | サンプルを少しずつ復号して試聴し、同じ読込みから波形を表示する |
| [0155](0155-remembered-playback-start.md) | 一部置換（[0181](0181-piano-reference-workflow-and-keys.md)） | 記憶した再生開始位置と、停止・一時停止を分ける |
| [0161](0161-placed-audio-without-fades.md) | 現行 | 置いた音声に自動のフェードを付けない |
| [0164](0164-note-end-chase.md) | 現行 | ノートの終端ちょうどから再生する時、そのノートを追いかけて鳴らさない |
| [0205](0205-playback-options.md) | 現行 | 再生の生成を 1 つの入口と `PlaybackOptions` へまとめる |
| [0231](0231-offline-export-and-scan-cache.md) | 現行 | オフライン書き出しを実時間の期限から外し、走査キャッシュを置換で書く |
| [0236](0236-callback-work-per-track.md) | 一部置換（[0383](0383-pdc-stage-notes.md)） | コールバックでトラックごとに全体を走査しない |
| [0238](0238-audio-callback-panics.md) | 一部置換（[0372](0372-release-line-tables-and-dsym-archive.md)） | オーディオコールバックの panic を捕まえ、理由を残す |
| [0239](0239-live-note-cursor-work.md) | 一部置換（[0279](0279-unchanged-track-voice-handover.md)） | 境界の処理を鳴っているノートと索引に限定する |
| [0240](0240-block-local-event-capacity.md) | 現行 | イベント容量をブロック内の密度から求める |
| [0241](0241-unscaled-placement-pcm.md) | 一部置換（[0369](0369-checked-sample-planes-at-the-engine-entrance.md)） | 配置の PCM をゲインとモノの昇格から分ける |
| [0243](0243-float-wav-metadata.md) | 現行 | float WAV の形式拡張とフレーム数 |
| [0244](0244-folder-mute-and-solo-gates.md) | 現行 | フォルダのミュートとソロを子孫のフェーダーへ反映する |
| [0247](0247-pcm24-wav-export.md) | 現行 | CLI の 24 bit PCM WAV 書出し |
| [0248](0248-pcm16-wav-export.md) | 現行 | CLI の 16 bit PCM WAV 書出し |
| [0249](0249-rf64-wav-export.md) | 現行 | 大容量 WAV の RF64 自動切替 |
| [0250](0250-explicit-export-tail.md) | 現行 | CLI 書出しの余韻を明示する |
| [0251](0251-tpdf-export-dither.md) | 現行 | CLI の整数 WAV へ TPDF ディザを接続する |
| [0253](0253-product-only-playback-commands.md) | 現行 | 製品だけの計画受渡しと音声側の命令型 |
| [0254](0254-gui-snapshot-export.md) | 現行 | GUI の書出し設定・専用スナップショット・別プロセス実行 |
| [0255](0255-export-progress.md) | 現行 | CLI と GUI の書出し進捗 |
| [0256](0256-export-cancellation.md) | 現行 | 書出しの協調的な途中取消 |
| [0257](0257-portable-export-settings.md) | 現行 | 書出し設定をプロジェクトと一緒に持ち運ぶ |
| [0258](0258-peak-export-normalization.md) | 現行 | 書出しへ任意のサンプルピークノーマライズを接続する |
| [0259](0259-export-destination-boundary.md) | 現行 | CLI の書出し先を元パッケージの外へ限定する |
| [0260](0260-pinned-gui-export-destination.md) | 現行 | GUI の書出し先をスナップショット準備前に固定する |
| [0261](0261-reveal-export-output.md) | 現行 | 完了した書出しのファイルを Finder で表示する |
| [0262](0262-export-numeric-drafts.md) | 現行 | 書出しの数値欄は文字列の下書きを検証してから確定する |
| [0263](0263-common-export-defaults.md) | 現行 | 書出しの共通既定設定をこの Mac に保持する |
| [0264](0264-named-export-presets.md) | 現行 | 名前付きの書出しプリセットを既定値から分離する |
| [0267](0267-current-playback-id-mappings.md) | 一部置換（[0274](0274-reused-track-plans.md)） | 再生 ID の対応表を現在の計画に限定する |
| [0272](0272-staged-import-media.md) | 一部置換（[0362](0362-published-media-and-helper-lifetimes.md)） | 取込みの素材を受理した編集と一緒に公開する |
| [0273](0273-shared-source-clip-windows.md) | 現行 | 長いトリムは共有の PCM の窓を読む |
| [0274](0274-reused-track-plans.md) | 一部置換（[0279](0279-unchanged-track-voice-handover.md)） | 変わらないトラックの再生計画を使い回す |
| [0275](0275-shared-integer-tempo-index.md) | 現行 | モデルと再生で共有する整数時間とテンポ索引 |
| [0277](0277-reaper-style-folder-bus.md) | 現行 | フォルダは Reaper と同じく、子の音を親トラックへ通す |
| [0279](0279-unchanged-track-voice-handover.md) | 一部置換（[0280](0280-placed-cursor-and-voice-matching.md)） | 計画の採用で、変わらないトラックの声をそのまま引き継ぐ |
| [0280](0280-placed-cursor-and-voice-matching.md) | 現行 | 全トラックを作り直す編集の採用を、事前の位置合わせと声の照合で短くする |

#### ADR 0282〜0383

| ADR | 状態 | 表題 |
|---|---|---|
| [0282](0282-plugin-nan-and-master-auto-mute.md) | 一部置換（[0359](0359-finite-overflow-is-a-fault.md)） | プラグインの NaN でトラックを、暴走した出力でマスターを自動ミュートする |
| [0283](0283-output-time-callback-deadline.md) | 現行 | 出力のブロックを HAL の出力時刻と機器の安全余白から終える |
| [0291](0291-typed-audio-io-errors.md) | 現行 | 出力・試聴のエラーを列挙型にする |
| [0294](0294-typed-engine-errors.md) | 現行 | エンジンのエラーを列挙型にし、エラーを文に変える `map_err` を拒否する |
| [0330](0330-playing-edit-media-and-tempo-retime.md) | 現行 | 再生中の編集の素材も別スレッドで読み、テンポの変わった計画は採用時に拍を保つ |
| [0331](0331-off-worker-audio-import.md) | 現行 | 外部の音声の取込みを文書ワーカーの外で段階に置き、後の編集は取込みの後に順に当てる |
| [0333](0333-install-diff-graph-off-worker.md) | 現行 | 出力の導入時の差分で増えるヘルパーも準備スレッドで起動し、再生はその計画を待つ |
| [0335](0335-retimed-plans-keep-note-numbers.md) | 現行 | テンポ・レートだけが変わったトラックは、ノートの番号を前回の計画から位置で引き継ぐ |
| [0337](0337-mono-instrument-on-stereo-source.md) | 現行 | モノの最初の音源は、ステレオの素材と同じトラックで等電力のステレオにする |
| [0338](0338-folder-bus.md) | 現行 | 子の音を親トラックのバスへ通す（フォルダバスの接続） |
| [0339](0339-plugin-delay-compensation.md) | 一部置換（[0340](0340-latency-graph.md)・[0373](0373-pdc-playhead-position.md)） | プラグインの遅延を合流点の遅延線で揃える（PDC） |
| [0340](0340-latency-graph.md) | 現行 | PDC を遅延グラフで計算し、公開の前に不変条件で検証する |
| [0344](0344-sample-audition-output-guard.md) | 現行 | サンプル試聴の最終出力を共通のガードで止める |
| [0345](0345-track-mix-fault-attribution.md) | 現行 | ホストの音声合流の異常をプラグインの障害と分ける |
| [0358](0358-product-musical-transport.md) | 一部置換（[0378](0378-pdc-stage-transport.md)） | 曲の時刻をブロックごとに作り、全プラグイン段へ渡す |
| [0359](0359-finite-overflow-is-a-fault.md) | 現行 | f32 を超える有限の積と和を 0 にせず障害にする |
| [0366](0366-hal-fault-isolation-and-overload-notices.md) | 現行 | HAL の機器単位の失敗を分離し、過負荷は集約して記録する |
| [0369](0369-checked-sample-planes-at-the-engine-entrance.md) | 現行 | 素材の有限性とゲインの積を、エンジンの入口の型で確かめる |
| [0373](0373-pdc-playhead-position.md) | 現行 | PDC の再生ヘッドを処理の時計から分ける |
| [0378](0378-pdc-stage-transport.md) | 現行 | 各プラグイン段の曲時刻を入力音声へ揃える |
| [0383](0383-pdc-stage-notes.md) | 現行 | ノートを各段の入力音声の時刻で生成する |

### 置換済み・草案

| ADR | 状態 | 表題 |
|---|---|---|
| [0013](0013-windowed-sinc.md) | 置換済み（[0014](0014-existing-implementations-as-options.md)） | サンプルレート変換は窓付き sinc |
| [0036](0036-audio-handoff.md) | 置換済み（[0253](0253-product-only-playback-commands.md)） | 実行計画はキューで渡し、解放は制御スレッドが行う |

## プラグイン・隔離

### 現行・一部置換

#### ADR 0016〜0312

| ADR | 状態 | 表題 |
|---|---|---|
| [0016](0016-plugin-scan-is-outside.md) | 一部置換（[0285](0285-scan-timeouts-are-not-verdicts.md)） | プラグインの検知は本体の外で行う |
| [0037](0037-clap-events.md) | 現行 | ブロックのイベントは CLAP の形にし、番号は再生のあいだ固定する |
| [0041](0041-helper-threads.md) | 一部置換（[0343](0343-relay-realtime-main-notifications.md)） | ヘルパーはメイン、処理、制御の 3 スレッドに分ける |
| [0042](0042-plugin-ipc.md) | 現行 | プラグインの往復は共有メモリ、制御はソケットに分ける |
| [0043](0043-plugin-submit-collect.md) | 現行 | プラグインは依頼してから、まとめて受け取る |
| [0048](0048-clap-device.md) | 現行 | CLAP デバイスはミキサーの前に置き、スキャンはヘルパーで行う |
| [0049](0049-hosted-playback.md) | 現行 | 再生は CLAP をヘルパーへ通し、内蔵シンセの前に置く |
| [0060](0060-plugin-folder-watch.md) | 一部置換（[0285](0285-scan-timeouts-are-not-verdicts.md)） | プラグインフォルダの変化を、開いているブラウザへ届ける |
| [0065](0065-plugin-event-capacity.md) | 一部置換（[0102](0102-exclusive-audio-graph.md)） | ノートが増えたら、生きているヘルパーのイベント容量を広げる |
| [0066](0066-plugin-note-off.md) | 現行 | プラグインへ届いたノートは、消す前にノートオフを送る |
| [0075](0075-declared-audio-ports.md) | 現行 | CLAP の audio-ports 宣言に合わせて処理する |
| [0076](0076-plugin-state-bytes.md) | 現行 | CLAP の state をメインスレッドで保存し、ハッシュだけをプロジェクトに置く |
| [0078](0078-keep-plugin-helpers.md) | 現行 | 変わっていない CLAP デバイスのヘルパーを残す |
| [0080](0080-helper-realtime.md) | 現行 | ヘルパーの音声処理では確保しない |
| [0088](0088-scan-pipes.md) | 現行 | スキャンの出力は子の実行中に読む |
| [0099](0099-transport-plugin-state.md) | 現行 | 停止と位置移動でプラグインの旧音を消す |
| [0111](0111-macos-helper-notices.md) | 現行 | ヘルパーの致命信号、Workgroup の待合、未実装の macOS 境界 |
| [0120](0120-device-bypass.md) | 現行 | CLAP のホストバイパスを編集・保存・音声へ接続する |
| [0123](0123-explicit-sine-pad-plugin.md) | 現行 | Sine Pad を明示的な隔離 CLAP 音源にする |
| [0125](0125-device-offline.md) | 現行 | CLAP のオフライン状態を保存と隔離グラフへ接続する |
| [0130](0130-clap-parameter-editing.md) | 一部置換（[0131](0131-device-view-and-responsive-parameters.md)） | CLAP のパラメーター編集をプラグイン状態の記録として接続する |
| [0132](0132-helper-plugin-windows.md) | 一部置換（[0134](0134-plugin-window-lifetime-and-shortcuts.md)） | プラグイン独自のウィンドウをヘルパーが持ち、プラグイン側の変更を履歴へ中継する |
| [0134](0134-plugin-window-lifetime-and-shortcuts.md) | 現行 | プラグイン GUI の寿命を表示状態から分離し、ループとキー入力で保持する |
| [0142](0142-background-plugin-graph-preparation.md) | 現行 | 開いた出力の置換ヘルパーを文書ワーカー外で準備する |
| [0199](0199-owned-helper-control-requests.md) | 現行 | ヘルパーの制御要求を所有し、応答を要求番号で照合する |
| [0227](0227-control-only-plugin-operations.md) | 現行 | プラグイン操作の実装を制御専用ハンドルへ集める |
| [0229](0229-plugin-window-live-edits.md) | 現行 | プラグインの窓での未記録の変更を、無関係な文書の変更から守る |
| [0237](0237-helper-descriptor-limit.md) | 現行 | 最初のヘルパーの前に記述子の上限を上げる |
| [0245](0245-plugin-deadline-fades.md) | 現行 | プラグインの取りこぼしと復帰を標本数でつなぐ |
| [0246](0246-plugin-latency-reporting.md) | 現行 | CLAP の遅延を隔離ヘルパーから表示まで運ぶ |
| [0266](0266-host-independent-sine-pad-dsp.md) | 現行 | Sine Pad の DSP をホストから分離する |
| [0285](0285-scan-timeouts-are-not-verdicts.md) | 現行 | 走査の時間切れを壊れたという判定にせず、遅いバンドルを後で長く待つ |
| [0288](0288-received-descriptors-are-all-owned.md) | 現行 | 制御ソケットで受けた記述子を全部持ち、2 個以上のメッセージを拒否する |
| [0293](0293-typed-helper-boundary-errors.md) | 現行 | ヘルパーとの境界のエラーと拒否を列挙型にする |
| [0295](0295-typed-plugin-host-errors.md) | 現行 | ヘルパーのエラーとプラグイン自身の失敗を列挙型にする |
| [0308](0308-typed-helper-launch-failures.md) | 現行 | ヘルパーの起動拒否を種類と診断文に分けて送る |
| [0309](0309-typed-helper-request-failures.md) | 現行 | ヘルパーの無効な ID と内部の応答不一致を型で送る |
| [0310](0310-typed-plugin-load-boundary.md) | 現行 | 起動のエラーを実行中のエラーから型で分ける |
| [0311](0311-typed-shared-memory-errors.md) | 現行 | 共有メモリの失敗で操作と errno を保持する |
| [0312](0312-typed-control-text-errors.md) | 現行 | 制御メッセージの不正な UTF-8 の原因を保持する |

#### ADR 0313〜0382

| ADR | 状態 | 表題 |
|---|---|---|
| [0313](0313-typed-launch-refusal-decoding.md) | 現行 | 起動拒否の不正なペイロードの原因を保持する |
| [0314](0314-typed-refusal-structure-errors.md) | 現行 | 拒否ペイロードの構造の原因を保持する |
| [0315](0315-typed-control-metadata-errors.md) | 現行 | 制御メタデータの構造の原因を保持する |
| [0316](0316-typed-parameter-payload-errors.md) | 現行 | パラメーター・窓・通知の内側の構造の原因を保持する |
| [0317](0317-typed-control-transport-errors.md) | 一部置換（[0318](0318-split-control-messages.md)） | 記述子の規則の違反と送信の拒否の原因を保持する |
| [0318](0318-split-control-messages.md) | 現行 | 1 枠を超える制御メッセージを分けて送り、溜まった分を読む |
| [0319](0319-control-send-deadline.md) | 現行 | 送信は待つ間に相手の分を読み、本体の要求は送信にも期限を持つ |
| [0329](0329-plugin-states-by-name.md) | 現行 | 記録済みのプラグイン状態を名前で比べ、読み込むヘルパーだけが読む |
| [0334](0334-cancel-helper-starts-on-close.md) | 現行 | 出力を閉じる・機器を替える時は、起動中のヘルパーを待たずに取り消す |
| [0341](0341-recover-notes-after-helper-overrun.md) | 現行 | ヘルパーへ渡せなかったノートは現在の発音へ戻す |
| [0342](0342-restart-completion-gates-processing.md) | 現行 | 再起動の完了世代までプラグインの処理を再開しない |
| [0343](0343-relay-realtime-main-notifications.md) | 現行 | 処理スレッドのメイン通知を制御側から起こす |
| [0346](0346-failed-device-commits-release-saves.md) | 現行 | デバイスの確定の失敗でも背景保存の保留を終える |
| [0347](0347-clap-thread-check.md) | 現行 | CLAP のスレッド判定を生成時の ID と既存の TLS から返す |
| [0348](0348-background-state-capture-retries.md) | 現行 | 背景の状態取得は変更の印を保ち、間隔を空けて取り直す |
| [0349](0349-clap-log-relay.md) | 現行 | CLAP のログは固定量の診断スロットから制御側へ渡す |
| [0351](0351-clap-main-thread-timers.md) | 現行 | CLAP のタイマーをインスタンスの主スレッドで管理する |
| [0352](0352-live-clap-tails-and-export-fence.md) | 現行 | CLAP の余韻長を通知で更新し、書出しの確定前に読み直す |
| [0355](0355-clap-audio-port-name-rescans.md) | 一部置換（[0374](0374-dimension-preserving-audio-port-rescans.md)） | CLAP の音声ポートの再走査は名前の変更だけを公開する |
| [0356](0356-clap-note-port-capabilities-and-lifecycle.md) | 一部置換（[0375](0375-midi-1-notes-for-midi-only-clap-ports.md)） | CLAP のノート形式の対応を公開し、終了処理を処理器と一緒に渡す |
| [0357](0357-reentrant-parameter-description-rescans.md) | 現行 | パラメーターの再走査通知を読取り中にも受ける |
| [0362](0362-published-media-and-helper-lifetimes.md) | 現行 | 公開済み素材を保持し、失敗したヘルパーの寿命を閉じる |
| [0364](0364-remove-unused-region-restart-reset.md) | 現行 | 未使用の共有領域リセット入口を削除する |
| [0367](0367-helper-notice-bounds-and-audit-diagnostics.md) | 現行 | ヘルパー通知の集約・上限制限と診断・状態同期の補強 |
| [0374](0374-dimension-preserving-audio-port-rescans.md) | 一部置換（[0382](0382-audio-port-changes-exchange-mapping-and-routing.md)） | 音声ポートの寸法を変えない通知を許可し、再有効化前に構成を検証する |
| [0375](0375-midi-1-notes-for-midi-only-clap-ports.md) | 一部置換（[0379](0379-midi2-notes-for-clap-ports.md)） | CLAP 形式を受けないノート入力ポートへ曲のノートを MIDI 1.0 で送る |
| [0379](0379-midi2-notes-for-clap-ports.md) | 一部置換（[0380](0380-mpe-member-channels-for-clap-ports.md)） | MIDI 2.0 だけを受ける CLAP ポートへ曲のノートを UMP で送る |
| [0380](0380-mpe-member-channels-for-clap-ports.md) | 一部置換（[0381](0381-mpe-note-expression.md)） | MPE だけを受ける CLAP ポートへ下位ゾーンを設定し、ノートをメンバーチャンネルで送る |
| [0381](0381-mpe-note-expression.md) | 現行 | MPE の表情は単独所有中のメンバーへ送り、ノートオン前に初期値を送る |
| [0382](0382-audio-port-changes-exchange-mapping-and-routing.md) | 現行 | 音声ポートの変更を、共有領域と親のルーティングの交換として接続する |

## UI・描画・入力

### 現行・一部置換

#### ADR 0045〜0172

| ADR | 状態 | 表題 |
|---|---|---|
| [0045](0045-visible-elements.md) | 現行 | 可視範囲は ID と役割を返す |
| [0050](0050-arrange-viewport.md) | 一部置換（[0051](0051-arrange-presentation.md)） | トラックヘッダとタイムラインの表示領域を共有する |
| [0051](0051-arrange-presentation.md) | 現行 | トラックヘッダとタイムラインを一つの Metal フレームにする |
| [0052](0052-piano-keyboard.md) | 一部置換（[0117](0117-workflow-interactions.md)） | ピアノロールの鍵盤とディグリー |
| [0053](0053-workspace-ui-foundation.md) | 一部置換（[0117](0117-workflow-interactions.md)・[0174](0174-shared-piano-viewport.md)） | Phase 1 のワークスペースの状態と操作を分離する |
| [0058](0058-waveform-peaks.md) | 現行 | 波形のピークは CPU で作り、見えている列だけを描く |
| [0085](0085-arrange-index.md) | 現行 | 可視索引は ID で引く |
| [0091](0091-playhead-wake.md) | 現行 | 停止中のプレイヘッドを描く |
| [0098](0098-pane-render-boundaries.md) | 現行 | ペイン変更中も表示面を境界内へ収める |
| [0107](0107-mix-controls.md) | 現行 | トラックと音声クリップの画面操作を製品経路へ接続する |
| [0118](0118-track-fx-chain-editing.md) | 一部置換（[0131](0131-device-view-and-responsive-parameters.md)） | トラック FX のチェイン操作を既存の編集経路へ接続する |
| [0122](0122-sample-library-search-and-placement.md) | 現行 | サンプル素材の登録検索と入力時点の配置先を保持する |
| [0128](0128-folder-disclosure-and-row-projection.md) | 現行 | フォルダ開閉を表示用スナップショットへ分離する |
| [0131](0131-device-view-and-responsive-parameters.md) | 一部置換（[0146](0146-independent-editor-stack.md)） | 横並びのデバイスビューと、プラグインの応答を待たない一覧 |
| [0137](0137-device-disclosure.md) | 現行 | デバイス列の開閉と、可変幅での FX ドロップ |
| [0138](0138-plugin-window-status.md) | 現行 | 独自ウィンドウの寿命をデバイスの名前の横へ表示する |
| [0139](0139-track-header-fx-names.md) | 現行 | トラックヘッダの挿入済み FX と追加入口を分ける |
| [0140](0140-device-parameter-list-retry.md) | 現行 | パラメーター一覧の失敗をデバイス列から再試行する |
| [0141](0141-device-output-preparation-status.md) | 現行 | デバイス列で出力の準備と一覧の応答待ちを区別する |
| [0143](0143-browser-click-and-audition-timing.md) | 一部置換（[0151](0151-streaming-sample-audition.md)） | ブラウザの一覧を押した時点で選択し、試聴の各段の時刻を残す |
| [0144](0144-unscaled-canvas-resizing.md) | 一部置換（[0149](0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)・[0150](0150-stale-drawable-frame.md)） | ペインのリサイズで前のフレームを伸縮せず、指定ヘッダ幅を保つ |
| [0145](0145-sample-browser-file-drops.md) | 現行 | 制作向けファイルの一覧と音声の位置指定ドロップ |
| [0146](0146-independent-editor-stack.md) | 現行 | 下部のエディタを個別に開閉し、固定高のデバイスを下へ積む |
| [0149](0149-flipped-canvas-anchor-and-fixed-transport-chrome.md) | 現行 | 反転した表示面の固定位置を AppKit に合わせ、状態行・再生ボタン・縦位置・アクセシビリティ要素の寿命を固定する |
| [0150](0150-stale-drawable-frame.md) | 現行 | 入力と大きさの違う drawable にも現在の入力を描く |
| [0152](0152-browser-audition-controls.md) | 現行 | ブラウザ表示中の試聴出力を保持し、音量と手動の開始位置を接続する |
| [0153](0153-value-control-clicks-and-typed-entry.md) | 現行 | 値の操作子のクリック規則と数値入力を共通にする |
| [0154](0154-clip-move-and-duplicate.md) | 一部置換（[0170](0170-content-relative-auto-bars.md)） | クリップの移動と複製移動を置き換えで接続する |
| [0156](0156-ruler-loop-range-and-entry.md) | 現行 | ルーラーのループ範囲指定と、終端以降からの通常進行 |
| [0157](0157-arrange-ruler.md) | 現行 | アレンジのルーラーと共有トランスポート |
| [0158](0158-ruler-loop-edge-handles.md) | 現行 | 両ルーラーの既存ループ端をドラッグする |
| [0159](0159-playhead-follow.md) | 現行 | 両エディタを再生ヘッドへ追従させ、手動の時間スクロールで解除する |
| [0160](0160-ordered-transport-requests.md) | 現行 | トランスポートの要求を順番待ちにし、操作子とサンプル一覧の Space を止めない |
| [0162](0162-plugin-candidate-table.md) | 現行 | FX 候補の一覧を AppKit の表にし、押した時点で選択して行のどこでも追加する |
| [0163](0163-sample-overview.md) | 現行 | ブラウザの波形を試聴から分け、選択した素材の全体を表示して残す |
| [0165](0165-inspector-fx-add-and-arrange-note-pitch.md) | 現行 | インスペクターの FX 追加ボタンと、アレンジの MIDI クリップの音高表示を要件へ加える |
| [0167](0167-piano-note-drag-snapping.md) | 現行 | ピアノロールの単一ノートのドラッグで Shift による吸着解除を接続する |
| [0168](0168-note-edit-clip-growth.md) | 現行 | ノートの時刻編集で対象の固定クリップも必要な分だけ広げる |
| [0171](0171-piano-blank-drag-insertion.md) | 現行 | ピアノロールの空白ドラッグでノートを追加する |
| [0172](0172-piano-click-and-input-length.md) | 現行 | ピアノロールの単クリック入力と入力長の共有 |

#### ADR 0173〜0336

| ADR | 状態 | 表題 |
|---|---|---|
| [0173](0173-piano-note-velocity.md) | 現行 | 単一ノートの Option ドラッグと共通入力ベロシティ |
| [0174](0174-shared-piano-viewport.md) | 現行 | ピアノロールの表示範囲をトラック間で共有する |
| [0175](0175-piano-velocity-lane.md) | 現行 | ピアノロール下部の単一ノート用ベロシティレーン |
| [0176](0176-piano-note-selection-and-deletion.md) | 現行 | ピアノロールの複数選択と一括削除 |
| [0177](0177-piano-batch-note-velocity.md) | 現行 | 選択したノートのベロシティを同じ量だけ調整する |
| [0178](0178-piano-batch-note-movement.md) | 現行 | 選択したノートを同じ時刻差・音程差で移動する |
| [0179](0179-piano-batch-note-resizing.md) | 現行 | 選択したノートの左右端を同じ量だけ伸縮する |
| [0181](0181-piano-reference-workflow-and-keys.md) | 現行 | ピアノの入力先を分離し、参照ノートとキーボード編集を接続する |
| [0184](0184-piano-note-duplicate-drag.md) | 現行 | Command ドラッグでノートを複製移動する |
| [0185](0185-piano-auto-and-fixed-grid.md) | 現行 | ピアノロールの自動・固定グリッド |
| [0186](0186-piano-triplet-and-dotted-grids.md) | 現行 | ピアノロールの三連符・付点グリッド |
| [0187](0187-piano-independent-time-guides.md) | 現行 | 吸着幅と独立したピアノロールの時間目印 |
| [0188](0188-arrange-adjacent-clip-duplication.md) | 現行 | アレンジの単一クリップを直後へ複製する |
| [0189](0189-selected-track-mute-and-solo.md) | 現行 | 選択トラックのミュート・ソロを一括で切り替える |
| [0191](0191-selected-track-numeric-mix.md) | 現行 | 選択トラックの音量・パンを数値入力で相対調整する |
| [0192](0192-selected-track-accessibility-steps.md) | 現行 | ヘッダの音量・パンの読み上げ増減を選択全体へ適用する |
| [0193](0193-header-mix-gesture-document-scope.md) | 現行 | ヘッダの値操作を押下した文書とコールバックへ束縛する |
| [0194](0194-clip-mix-gesture-document-scope.md) | 現行 | クリップの値操作を押下した文書と編集先へ束縛する |
| [0206](0206-piano-interaction-state.md) | 現行 | ピアノロールの操作状態を 1 つの列挙型へまとめる |
| [0228](0228-selected-track-live-mix.md) | 現行 | 選択トラックのドラッグと既定値リセットを一括試聴する |
| [0289](0289-split-audio-replies-and-one-label-pass.md) | 現行 | 出力の応答を位置・試聴・状態に分けて観測し、アレンジの文字は 1 フレーム 1 回で用意する |
| [0336](0336-device-columns-show-output-updates.md) | 現行 | 開いた出力の更新中は、未取得のデバイス列に「音声出力を更新中」と出す |

## ビルド・検査・開発基盤

### 現行・一部置換

#### ADR 0044〜0320

| ADR | 状態 | 表題 |
|---|---|---|
| [0044](0044-swift-bridge.md) | 一部置換（[0269](0269-github-public-ci-scope.md)） | Rust と Swift は、生成した C ABI と薄い Swift の層でつなぐ |
| [0061](0061-distribution-signing.md) | 現行 | 配布バンドルを生成した Xcode プロジェクトと同じ自己署名で作る |
| [0072](0072-hardened-runtime.md) | 現行 | 本体の署名は Hardened Runtime を明示する |
| [0092](0092-playhead-spike.md) | 一部置換（[0093](0093-product-contract-gate.md)） | プレイヘッドの実ウインドウ回帰は明示した計測へ置く |
| [0093](0093-product-contract-gate.md) | 一部置換（[0324](0324-no-agent-hooks.md)・[0328](0328-one-cargo-selection-for-the-task-tier.md)） | 利用経路の回帰検査と、その検査が不具合を拒否することを必須にする |
| [0094](0094-agent-development-foundation.md) | 現行 | エージェントの作業資料と診断を整理する |
| [0096](0096-measurement-completion.md) | 現行 | 音声計測の進行と完走を合格条件にする |
| [0100](0100-scoped-verification.md) | 現行 | 検査の依存入力と長時間計測を分ける |
| [0112](0112-m4-minimum.md) | 現行 | 対象チップを M4 ファミリー以降にする |
| [0113](0113-product-macos-workgroup-and-caches.md) | 一部置換（[0114](0114-m4-build-baseline.md)） | 製品の Workgroup 接続、M4 ビルド条件、PCM と描画の再利用 |
| [0114](0114-m4-build-baseline.md) | 一部置換（[0269](0269-github-public-ci-scope.md)） | 自分たちでビルドするコードの M4 基準を統一する |
| [0119](0119-agent-token-efficiency.md) | 現行 | AI エージェント作業時のトークン効率と文書・テスト・検証出力の整理 |
| [0121](0121-startup-recovery-and-errno-validation.md) | 現行 | 起動復旧の終了待ちと errno 検査ランタイムを修正する |
| [0126](0126-agent-context-and-source-reading.md) | 現行 | エージェントの一括開始診断と根拠付き原文読込 |
| [0148](0148-verification-pipeline-speed.md) | 一部置換（[0197](0197-verification-target-scopes.md)・[0198](0198-verification-launch-and-cache-cost.md)・[0271](0271-task-tier-duplicate-runs.md)・[0328](0328-one-cargo-selection-for-the-task-tier.md)） | 検査の範囲と合否を変えずに `make verify` を速くする |
| [0166](0166-verification-failure-detection.md) | 現行 | 検査の失敗を隠さず、実行不能な環境と安価な失敗を先に検出する |
| [0190](0190-native-scan-deadline-fixtures.md) | 現行 | 走査期限の回帰をネイティブ検査ヘルパーで再現する |
| [0195](0195-verification-cost-observability.md) | 一部置換（[0198](0198-verification-launch-and-cache-cost.md)） | 検査コストを継続記録し、反例のビルドだけを共用する |
| [0197](0197-verification-target-scopes.md) | 一部置換（[0198](0198-verification-launch-and-cache-cost.md)・[0212](0212-verification-tiers.md)・[0324](0324-no-agent-hooks.md)） | 検査の依存ターゲットと証拠の段階を区別する |
| [0198](0198-verification-launch-and-cache-cost.md) | 現行 | 契約の起動コストと診断・ビルドキャッシュの境界を分ける |
| [0207](0207-continuation-handoff-tsan.md) | 現行 | 継続の先行再開の前後関係を TSan へ注釈で示す |
| [0208](0208-verification-shared-scopes.md) | 現行 | 検査の識別計算で同じ範囲の読込みを共有する |
| [0209](0209-real-hal-guard.md) | 現行 | UI テストのバンドルから実 HAL をなくし、触れたテストを名指しで止める |
| [0210](0210-sanitizer-test-width.md) | 現行 | サニタイザの Swift 全体実行の同時実行数を絞り、競合の検出は全テスト同時の実行で保つ |
| [0211](0211-opt-in-history-spike.md) | 現行 | Phase 0 の履歴モデルを非既定の feature へ分離する |
| [0212](0212-verification-tiers.md) | 一部置換（[0324](0324-no-agent-hooks.md)・[0327](0327-instrumented-builds-only-on-github.md)） | 作業ごとの検査と節目の全体検査を分ける |
| [0213](0213-opt-in-engine-spike.md) | 現行 | Phase 0 のエンジン計測を非既定の feature へ分離する |
| [0232](0232-verification-escapes-and-expected-red.md) | 一部置換（[0327](0327-instrumented-builds-only-on-github.md)） | 検査の選択と意図した red の照合の取りこぼしを塞ぐ |
| [0233](0233-verification-deferral-reasons-and-traps.md) | 一部置換（[0325](0325-swift-test-stall-watchdog.md)） | 見送りの理由を検査ごとにし、検査の罠を減らす |
| [0235](0235-reading-budget-and-split-guides.md) | 一部置換（[0370](0370-domain-status-guides.md)） | 読む量に上限を置き、状態とデバッグの文書を分ける |
| [0265](0265-symbol-scoped-foreign-declarations.md) | 現行 | 外部宣言をシンボル単位で制限する |
| [0268](0268-bounded-export-diagnostics.md) | 現行 | 書出しの診断は先頭と末尾を一定量だけ表示する |
| [0269](0269-github-public-ci-scope.md) | 一部置換（[0324](0324-no-agent-hooks.md)・[0327](0327-instrumented-builds-only-on-github.md)） | GitHub で公開し、CI の範囲を決める |
| [0270](0270-gix-0-90-and-github-advisories.md) | 現行 | gix を 0.90 へ上げ、GitHub の勧告は RustSec と別に確かめる |
| [0271](0271-task-tier-duplicate-runs.md) | 一部置換（[0327](0327-instrumented-builds-only-on-github.md)・[0328](0328-one-cargo-selection-for-the-task-tier.md)） | 作業の段階で同じテストを何度も実行しない |
| [0284](0284-audio-path-panic-lints.md) | 現行 | 音声経路の crate で添字・unwrap・expect・panic を拒否する |
| [0290](0290-fuzz-targets-for-external-input.md) | 現行 | 外部入力に fuzz の対象を置き、カバレッジを測る |
| [0301](0301-typed-history-spike-errors.md) | 現行 | 履歴スパイクでも原因の型を保ち、初期化失敗後に終了できるようにする |
| [0302](0302-typed-ffi-catalog-errors.md) | 現行 | FFI の目録も C ABI の直前まで原因の型を保つ |
| [0320](0320-typed-package-entry-errors-and-workspace-check.md) | 現行 | パッケージの項目の拒否を型にし、文字列のエラーの検査をワークスペース全体へ広げる |

#### ADR 0321〜0372

| ADR | 状態 | 表題 |
|---|---|---|
| [0321](0321-unsafe-safety-comments-and-ffi-arguments.md) | 現行 | `unsafe` の全ブロックに根拠を書き、C ABI のポインター引数を一つの規則で検査する |
| [0322](0322-product-log.md) | 現行 | 製品のログを `log` の窓口と一つのロガーに置き、オーディオスレッドの出来事はリングで制御スレッドへ渡す |
| [0323](0323-swift-python-shell-format-and-lint.md) | 現行 | Swift・Python・シェルの整形と lint |
| [0324](0324-no-agent-hooks.md) | 現行 | エージェントのフックを置かない |
| [0325](0325-swift-test-stall-watchdog.md) | 現行 | Swift のテストの期限を、イベント列の外側の監視で掛ける |
| [0326](0326-foreign-declarations-checked-against-the-sdk.md) | 現行 | 残る外部宣言を 1 か所に集め、SDK のヘッダと照合する |
| [0327](0327-instrumented-builds-only-on-github.md) | 一部置換（[0361](0361-unshipped-configurations-only-on-github.md)） | 計装したビルドと反例は GitHub だけで実行する |
| [0328](0328-one-cargo-selection-for-the-task-tier.md) | 一部置換（[0361](0361-unshipped-configurations-only-on-github.md)） | 作業の段階の Cargo の呼出しを既定のメンバーの 1 つの選択にそろえる |
| [0332](0332-check-swift-test-width.md) | 現行 | 通常の check-swift も同時実行数 16 で合否を決める |
| [0361](0361-unshipped-configurations-only-on-github.md) | 現行 | 出荷しない構成は GitHub だけでビルドする |
| [0368](0368-opt-in-history-thinning-feature.md) | 現行 | 履歴の間引きとバンドルを非既定の feature へ閉じる |
| [0370](0370-domain-status-guides.md) | 現行 | 制約を領域別に分け、開始時は索引だけを読む |
| [0371](0371-adr-status-and-domain-index.md) | 現行 | 全 ADR に先頭の状態・領域行を持たせ、領域別の索引を自動生成する |
| [0372](0372-release-line-tables-and-dsym-archive.md) | 現行 | release ビルドで行番号表と版別 dSYM 保管を有効化し、panic.ndjson をオフライン解決可能にする |
