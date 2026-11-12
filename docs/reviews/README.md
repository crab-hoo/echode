# 監査の索引

監査と、その対応の ADR を領域ごとに並べる。[資料の入口](../README.md) から案内する。

作業する領域に関係する監査と ADR だけを読む。全項目を毎回通読しない。
監査は発見時点の証拠を残すため、過去の「失敗」「未実施」を書き換えず、現在の合否は `target/verification/report.md` で確認する。
証拠の保管と過去のローカル記録の扱いは [証拠の保全](../evidence/README.md) を参照する。

## 1. 全体横断・Phase 1 最終照合

- [2026-11-08 監査の未完了項目の統合](2026-11-08-open-items-register.md): 10/8・SPG・XCA・外部監査・FCA ほかの未完了を束ねた。
  重複と古い状態を整理し、残りを堅牢性・保守性・効率・機能に付随・ユーザーの担当の 38 行に分けた。
  順番と FCA-05 の区切りは [ADR 0278](../decisions/0278-audit-backlog-order.md) で決めた。最優先は FCA-02 の採用の固定費用（R1）。
  後で見つけた R8（64 MiB を超える状態でヘルパーが止まる）は [ADR 0318](../decisions/0318-split-control-messages.md) で直した。
  R9（送信に期限がなく、背景の保存と復元の交差で両端が止まる）は [ADR 0319](../decisions/0319-control-send-deadline.md) で直した。
- [2026-11-08 構造の評価の精査](2026-11-08-structure-assessment-review.md): モデルの検査の分担と placement.rs の構造の判定。
  漏れ得るのは `Draft::finish` の消費の検査だけで、ID 探索と検証基盤の規模は既知（FCA-05、2026-11-06 の精査）。
- [2026-11-06 課題一覧の精査](2026-11-06-assessment-review.md): 検証基盤と製品の課題一覧の判定。gix-pack は解決済み、CI の待ちの主因は同時実行数。
  長尺音声の配置窓の PCM 複製（EXT-01）と、拒否した取込みの素材が残ること（EXT-02）を `audit_ext*` で再現した。
  EXT-01 は [ADR 0273](../decisions/0273-shared-source-clip-windows.md)、EXT-02 は [ADR 0272](../decisions/0272-staged-import-media.md) で対応した。
- [2026-11-03 コード全体の監査](2026-11-03-full-code-audit.md): FCA-01〜20。実装の妥当性と規模をプロの水準で問い直した。
  ループの折り返しと再生中の編集がノート数に比例する FCA-01・02 は [ADR 0239](../decisions/0239-live-note-cursor-work.md) で対応した。
  イベント容量の FCA-03 は [ADR 0240](../decisions/0240-block-local-event-capacity.md) で対応した。
  配置の PCM の複製（FCA-10）は [ADR 0241](../decisions/0241-unscaled-placement-pcm.md) で対応した。
  FCA-04 は [ADR 0274](../decisions/0274-reused-track-plans.md) で、変わらないトラックの計画を使い回すようにした。
  FCA-05 の先行分は [ADR 0242](../decisions/0242-model-content-lookup-and-references.md) と監査の追記を参照する。残りは [ADR 0278](../decisions/0278-audit-backlog-order.md) で区切った。
  WAV（FCA-16）は [ADR 0243](../decisions/0243-float-wav-metadata.md) の float ヘッダと、
  [ADR 0247](../decisions/0247-pcm24-wav-export.md) の PCM24 と [ADR 0248](../decisions/0248-pcm16-wav-export.md) の PCM16 を接続した。
  大容量の RF64 は [ADR 0249](../decisions/0249-rf64-wav-export.md)、TPDF ディザは [ADR 0251](../decisions/0251-tpdf-export-dither.md) で接続した。聴感は UC-52 に残す。
  任意のサンプルピークノーマライズは [ADR 0258](../decisions/0258-peak-export-normalization.md) で接続した。LUFS・トゥルーピークは含めない。
  フォルダのミュート・ソロ（FCA-06）は [ADR 0244](../decisions/0244-folder-mute-and-solo-gates.md) で子孫へ反映した。音量・パンとバス化は残る。
  取りこぼしの減衰・復帰（FCA-08）は [ADR 0245](../decisions/0245-plugin-deadline-fades.md) で接続した。実機の聴感は UC-50 に残す。
  監査時の計測は [規模の計測](../evidence/2026-11-03-scale-measurements.md) に置く。
  時間換算の重複とテンポ境界ごとの再探索（FCA-13）は [ADR 0275](../decisions/0275-shared-integer-tempo-index.md) で共通の不変索引へ集めた。
  遅延の取得・通知・表示（FCA-07）の先行分は [ADR 0246](../decisions/0246-plugin-latency-reporting.md)。PDC と残るホスト拡張は未接続。
  ジャーナルの末尾確認（FCA-14）は [ADR 0252](../decisions/0252-bounded-journal-tail-repair.md) で全件読込みをなくした。まとめ書きは行わず、編集ごとの同期を保つ（[ADR 0276](../decisions/0276-priority-order-and-delegated-decisions.md)）。
  未使用の計画受渡しと音声側へ届かない命令（FCA-11）は [ADR 0253](../decisions/0253-product-only-playback-commands.md) で削除した。
  手書き FFI（FCA-12）は [ADR 0265](../decisions/0265-symbol-scoped-foreign-declarations.md) で既存バインディングへ移し、許可をシンボル単位にした。
  同梱音源のホスト依存（FCA-15）は [ADR 0266](../decisions/0266-host-independent-sine-pad-dsp.md) で分離した。
  再生 ID 表（FCA-19）の先行分は [ADR 0267](../decisions/0267-current-playback-id-mappings.md)。Undo の段の使用量の診断は [ADR 0286](../decisions/0286-history-usage-diagnostics.md)、写す単位を run にしたのは [ADR 0287](../decisions/0287-shared-note-runs.md)。総量の予算は再開の条件付きで入れていない。
- [2026-11-03 コード全体の監査の実行指示](2026-11-03-fix-instructions.md): 上の監査の作業単位 W1〜W5、再現 4 件と `make spike-scale`、境界と完了条件。
- [2026-11-02 コード品質とエージェント環境の横断監査](2026-11-02-cross-cutting-audit.md): XCA-01〜22。
  保存の同期の順序、ソースと CI の継続性、エージェントごとの防護、資料の量。
  XCA-01〜03 は [ADR 0230](../decisions/0230-save-durability.md) で修正した。XCA-04 は 2026-11-02 にプッシュし、2026-11-05 に GitHub（非公開）へ移した。XCA-05 は 2026-11-05 に CI を静的検査へ限った（[ADR 0269](../decisions/0269-github-public-ci-scope.md)）。
  XCA-08・09 は [ADR 0233](../decisions/0233-verification-deferral-reasons-and-traps.md) で対応した。XCA-09 の Swift の期限は [ADR 0325](../decisions/0325-swift-test-stall-watchdog.md) で掛けた。
  XCA-11 は共通の運用の知識を各手順へ移し、XCA-12 は確認待ちを [ユーザー確認の一覧](../user-checks.md) へ集めた。
  XCA-16 は保存の不確定の判定を型にし（[ADR 0234](../decisions/0234-typed-journal-durability.md)）、`echode-audio-io`・`echode-document`・`echode-sandbox`・`echode-engine`・`echode-plugin-host` の境界のエラーを列挙型にした（[ADR 0291](../decisions/0291-typed-audio-io-errors.md)、[ADR 0292](../decisions/0292-typed-document-errors.md)、[ADR 0293](../decisions/0293-typed-helper-boundary-errors.md)、[ADR 0294](../decisions/0294-typed-engine-errors.md)、[ADR 0295](../decisions/0295-typed-plugin-host-errors.md)）。
  `echode-history` は保管庫・復号・ピーク（[ADR 0296](../decisions/0296-typed-storage-errors.md)）と、製品の Git・自動保存を型にした（[ADR 0297](../decisions/0297-typed-autosave-and-git-errors.md)）。
  スキーマ・パッケージ検証（[ADR 0298](../decisions/0298-typed-package-validation-errors.md)）、文書・ジャーナル・設定（[ADR 0299](../decisions/0299-typed-document-journal-and-settings-errors.md)）も型にした。
  間引き・パック・バンドルは [ADR 0300](../decisions/0300-typed-thinning-errors.md)。
  非既定の履歴スパイクも型にし、初期化失敗後の終了の待ちを修正した（[ADR 0301](../decisions/0301-typed-history-spike-errors.md)）。
  `echode-ffi` の目録も、ヘルパー未検出のパスと元の目録・JSON の原因を保つ（[ADR 0302](../decisions/0302-typed-ffi-catalog-errors.md)）。
  モデルの並びと不変条件も、組立て・パッケージ・復旧を通って規則・ID・時刻を保つ（[ADR 0303](../decisions/0303-typed-model-invariant-errors.md)）。
  API・MCP の通信と RPC の文書の原因も、応答の生成直前まで型で保持する（[ADR 0304](../decisions/0304-typed-api-errors.md)）。
  CLI も入力・I/O の操作とパス、元の保存・API・再生の原因を表示まで保持する（[ADR 0305](../decisions/0305-typed-cli-errors.md)）。
  アクションの引数は入力値付きの拒否を返し、Serde の文面の分類をなくした（[ADR 0306](../decisions/0306-typed-action-parameters.md)）。
  文書ワーカーの単一編集も型付きのテキスト入口を使い、構文・形と意味の原因を分けて保持する（[ADR 0307](../decisions/0307-typed-document-command-decoding.md)）。
  起動の変換は `LoadError` に狭め、実行中の失敗を文へ戻す防御分岐と未分類の拒否を除いた（[ADR 0310](../decisions/0310-typed-plugin-load-boundary.md)）。
  共有メモリの作成も操作と元の errno を保持し、文を I/O エラーへ包む再導入を検査する（[ADR 0311](../decisions/0311-typed-shared-memory-errors.md)）。
  制御文字列も窓名・パラメーターと元の UTF-8 の原因を保持する（[ADR 0312](../decisions/0312-typed-control-text-errors.md)）。
  不正な起動拒否も不足したフィールド・長さ・コードと診断文の UTF-8 の原因を保持する（[ADR 0313](../decisions/0313-typed-launch-refusal-decoding.md)）。
  拒否以外の制御メタデータもフィールド・長さ・種類とポートの方向を保持する（[ADR 0315](../decisions/0315-typed-control-metadata-errors.md)）。
  パラメーター・通知・窓の報告の内側もフィールド・項目の番号・長さと受けた値を保持する（[ADR 0316](../decisions/0316-typed-parameter-payload-errors.md)）。
  記述子の規則の違反と送信の拒否も届いた数・枠の種類と長さを保持し、`Remap` の送信の拒否をなくした（[ADR 0317](../decisions/0317-typed-control-transport-errors.md)）。
  XCA-13 はコールバックのトラックごとの全体の走査をなくした（[ADR 0236](../decisions/0236-callback-work-per-track.md)）。
  XCA-14 はコールバックの panic を捕まえて理由を記録へ残し（[ADR 0238](../decisions/0238-audio-callback-panics.md)）、音声経路の crate で添字などを拒否した（[ADR 0284](../decisions/0284-audio-path-panic-lints.md)）。
  証拠は [保存の耐久性の追跡](../evidence/2026-11-02-durability-trace.md) に置く。
- [2026-11-02 外部監査 2 件の精査](2026-11-02-external-audit-review.md): 製品・エージェント環境・検査時間の外部指摘の判定。
  プラグインの窓での変更の消失（DOC-A1・A2）と、保管物のない状態による読めない先端（HIS-A1）を再現した。
  DOC-A1・A2 は [ADR 0229](../decisions/0229-plugin-window-live-edits.md)、HIS-A1 は [ADR 0230](../decisions/0230-save-durability.md)、
  CLI-A1・PLG-A1 (b)(c) は [ADR 0231](../decisions/0231-offline-export-and-scan-cache.md)、
  ENV-01・05・06・07 は [ADR 0232](../decisions/0232-verification-escapes-and-expected-red.md) で修正した。ENV-01 の選択は [ADR 0327](../decisions/0327-instrumented-builds-only-on-github.md) で対象ごとなくした。
  SBX-A1 (2) は [ADR 0288](../decisions/0288-received-descriptors-are-all-owned.md) で修正した。ほかの状態は [統合表](2026-11-08-open-items-register.md) にある。
- [2026-11-02 監査対応の実行指示](2026-11-02-fix-instructions.md): 上の 2 監査の作業単位 W1〜W4、再現テスト、境界と完了条件。

- [2026-10-30 構造の横断監査](2026-10-30-structure-audit.md): SPG-01〜11。
  絡み合いを起こしやすい構造と根本の修正方針・順番。
  SPG-01 は [ADR 0199](../decisions/0199-owned-helper-control-requests.md)、SPG-02 は [ADR 0203](../decisions/0203-document-session.md)、
  SPG-05 は [ADR 0204](../decisions/0204-document-service.md)、SPG-07 は [ADR 0205](../decisions/0205-playback-options.md)、
  SPG-08 は [ADR 0206](../decisions/0206-piano-interaction-state.md) で対応し、当時の再現は保持する。
  SPG-10 は履歴とエンジンの Phase 0 コードを非既定 feature へ分離した（[ADR 0211](../decisions/0211-opt-in-history-spike.md)、[ADR 0213](../decisions/0213-opt-in-engine-spike.md)）。
  SPG-06 の残りの素通しは制御専用ハンドルへ集め、型付きの操作を保った（[ADR 0227](../decisions/0227-control-only-plugin-operations.md)）。
  SPG-09 はモデル・変更セット・保存・ジャーナルの項目登録と、FFI 内のスカラー転記の共有まで完了した。
  既存の形式・ABI・子の検証を保つ。経緯と境界は [ADR 0226](../decisions/0226-model-field-registry-completion.md) を参照する。

- [2026-10-27 敵対的監査](2026-10-27-adversarial-audit.md): ADV-01〜04 対応済み。
  壊れたジャーナルは拒否し、空の Undo は無変更の成功とする。
  古いコミットへ後退する復旧は、編集前の通知と明示同意を要求する（[ADR 0183](../decisions/0183-recovery-consent.md)）。
  証拠は [追跡する要約と保管方針](../evidence/README.md) を定め、新しい `target/` リンクを検査で拒否する。
  既に消えた過去の完全ログ・画像の欠落は残る。

- [2026-10-25 現行コード監査](2026-10-25-code-audit.md): CUR-01〜04。
  不正 PCM の panic、素材終端・テンポ変化での波形誤表示、API 応答の書込み待ちを再現し、後日対応を追記した。

- [2026-10-08 初期監査](2026-10-08-audit.md): HIS / MOD / ENG / PLG / API / UI / DEV の一覧と未着手項目（`audit_*` テスト）。
- [2026-10-13 境界監査](2026-10-13-audit.md)、[Phase 1 専門監査](2026-10-13-phase1-pro-audit.md): AUD-01〜13、PRO-01〜17。
- [修正指示](2026-10-14-phase1-fix-instructions.md)、[修正後監査](2026-10-14-phase1-fix-reaudit.md): 修正条件と再現結果。
- [2026-10-17 境界再監査](2026-10-17-boundary-reaudit.md)、[P1 最終照合](2026-10-17-phase1-acceptance.md): Phase 1 完了判定と境界回帰（[ADR 0108](../decisions/0108-phase1-loop-readiness.md)、[ADR 0109](../decisions/0109-placement-bounds-and-plugin-capture.md)）。

## 2. オーディオエンジン・リアルタイム・再生

- ステレオ・ループ・フェーダー補間: [ADR 0095](../decisions/0095-product-audio-buses-and-continuity.md)、[ADR 0108](../decisions/0108-phase1-loop-readiness.md)。
- 再生準備の取消・排他グラフ・停止減衰・非ブロック後片付け: [ADR 0097](../decisions/0097-cancellable-retained-transport.md)、[ADR 0101](../decisions/0101-live-output-discontinuities.md)、[ADR 0102](../decisions/0102-exclusive-audio-graph.md)、[ADR 0103](../decisions/0103-live-stop-waveform.md)、[ADR 0104](../decisions/0104-stop-output-diagnostics.md)、[ADR 0105](../decisions/0105-nonblocking-stop-cleanup.md)。
- 停止中のプラグイン処理・自然な余韻・編集時の全体暗転の解消: [ADR 0133](../decisions/0133-stopped-plugin-processing-and-edit-input.md)。
- 実機計測の完走条件: [ADR 0096](../decisions/0096-measurement-completion.md)。
- 再生の生成の 1 つの入口と、開始位置・時計・プラグイン・ワークグループの組合せ: [ADR 0205](../decisions/0205-playback-options.md)。
- 文書ワーカーから独立したサンプルの単発試聴・専用 IOProc: [ADR 0135](../decisions/0135-sample-audition.md)。
  有限の先読み・連続 SRC と同じ読込みからの部分波形: [ADR 0151](../decisions/0151-streaming-sample-audition.md)。
  表示中の出力保持・音量・自動試聴の切替と手動の開始位置: [ADR 0152](../decisions/0152-browser-audition-controls.md)。
  一覧を押した時点の選択、利用者の操作だけで変わる試聴、要求ごとの各段の時刻: [ADR 0143](../decisions/0143-browser-click-and-audition-timing.md)。
- 出力とヘルパーの準備を文書ワーカーの外で行う・機器なしの検査用出力: [ADR 0136](../decisions/0136-output-opening-off-worker.md)。
- 開いた出力の置換ヘルパー・共有領域の準備と後続編集の集約: [ADR 0142](../decisions/0142-background-plugin-graph-preparation.md)。
- プロジェクトを開いた時の出力準備・停止中の編集の素材の別スレッド読込み・読込み中の再生の保留: [ADR 0147](../decisions/0147-output-prewarm-and-background-media.md)。
- 再生中の編集の素材の別スレッド読込み・テンポの変わった計画の採用時の拍の保持・tick の位置移動: [ADR 0330](../decisions/0330-playing-edit-media-and-tempo-retime.md)。
- 外部の音声の取込みの別スレッドの段階・後の編集の投入順の保持・段階で作るピーク: [ADR 0331](../decisions/0331-off-worker-audio-import.md)。
- 出力の導入時の差分で増えるヘルパーの準備スレッドでの起動・その計画を待つ再生の開始: [ADR 0333](../decisions/0333-install-diff-graph-off-worker.md)。
- 閉じる・機器交換の時の起動中のヘルパーの取消: [ADR 0334](../decisions/0334-cancel-helper-starts-on-close.md)。
- 記憶した再生開始位置・余韻を保つ停止での復帰・一時停止・`transport.json`・ピアノロールのルーラー: [ADR 0155](../decisions/0155-remembered-playback-start.md)。
- ピアノロールのルーラードラッグの範囲指定・クリックとの分離と、ループ前からの進入・終端以降の通常進行:
  [ADR 0156](../decisions/0156-ruler-loop-range-and-entry.md)。
- アレンジのルーラー・両エディタの開始位置と範囲共有・行領域の上端: [ADR 0157](../decisions/0157-arrange-ruler.md)。
- 両ルーラーの既存端の調整・つかむ位置と反対端の保持・外部変更による取消: [ADR 0158](../decisions/0158-ruler-loop-edge-handles.md)。
- 両エディタの再生ヘッドへのページ送り・手動の時間スクロールでの解除・保存しない追従の値: [ADR 0159](../decisions/0159-playhead-follow.md)。

## 3. プラグイン隔離・サンドボックス・Workgroup

- CLAP バンドル・停止時リセット・自然な発音: [ADR 0093](../decisions/0093-product-contract-gate.md)、[ADR 0099](../decisions/0099-transport-plugin-state.md)、[ADR 0106](../decisions/0106-natural-plugin-onsets.md)。
- macOS ヘルパー通知・Workgroup 引き継ぎ: [2026-10-17 macOS 境界監査](2026-10-17-macos-helper-audit.md)、[ADR 0111](../decisions/0111-macos-helper-notices.md)、[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)。
- フォルダ型ドラムラックと FX チェイン: [ADR 0116](../decisions/0116-folder-drum-rack.md)、[ADR 0118](../decisions/0118-track-fx-chain-editing.md)。
- 選択トラックへの FX 一括追加: [ADR 0124](../decisions/0124-selected-track-fx-batches.md)。
- 同梱 Sine Pad の明示的な CLAP 化・音源なしの無音: [ADR 0123](../decisions/0123-explicit-sine-pad-plugin.md)。
- CLAP のホストバイパス・保存形式 4: [ADR 0120](../decisions/0120-device-bypass.md)。
- CLAP のオフライン化・保存形式 5: [ADR 0125](../decisions/0125-device-offline.md)。
- CLAP のパラメーター編集・停止中のヘルパー起動・状態の Undo: [ADR 0130](../decisions/0130-clap-parameter-editing.md)。
- 横並びのデバイスビュー・画面内の行だけの一覧・検索とピン留め・保存形式 6・ワーカー外のデバイス要求: [ADR 0131](../decisions/0131-device-view-and-responsive-parameters.md)。
- GUI の一時非表示・ループ変更での寿命、GUI フォーカス中のキー中継、末尾の FX 追加:
  [ADR 0134](../decisions/0134-plugin-window-lifetime-and-shortcuts.md)。
- ヘルパー所有のプラグインウィンドウ・フォーカス・プラグイン側の変更の中継と単一 Undo: [ADR 0132](../decisions/0132-helper-plugin-windows.md)。

## 4. UI・Metal 描画・入力・Swift 橋渡し

- 下部の個別開閉・固定高のデバイス・アレンジ非表示と旧設定の移行: [ADR 0146](../decisions/0146-independent-editor-stack.md)。
- 操作の要望の照合（リサイズ時の伸縮、ブラウザ・クリップの D&D、既定値と値入力のクリック、試聴、下部の積み方）: [2026-10-21 操作の要望の照合](2026-10-21-ui-feedback-audit.md)。
- 2026-10-25 の操作の報告（開始位置の変更時の暗転、一覧の Space、ブラウザの波形、置いた音声のフェード、FX 候補、ノートの終端からの再生）と追加の要望:
  [2026-10-25 操作の報告の照合](2026-10-25-user-feedback.md)。
  対応は [ADR 0160](../decisions/0160-ordered-transport-requests.md)〜[ADR 0164](../decisions/0164-note-end-chase.md)、
  インスペクターの FX 追加とアレンジの音高表示の要件は [ADR 0165](../decisions/0165-inspector-fx-add-and-arrange-note-pitch.md)。
- 制作向けファイルの一覧と音声の位置指定ドロップ・トラック追加を伴う取込みの単一履歴: [ADR 0145](../decisions/0145-sample-browser-file-drops.md)。
- デバイス列での初回出力準備と一覧の応答待ちの区別: [ADR 0141](../decisions/0141-device-output-preparation-status.md)。
- パラメーター一覧の失敗からの列内の再試行・連打の集約: [ADR 0140](../decisions/0140-device-parameter-list-retry.md)。
- 独自ウィンドウの寿命通知と、デバイス列・インスペクターの窓アイコン: [ADR 0138](../decisions/0138-plugin-window-status.md)。
- トラックヘッダの名前と FX 追加入口・残りのチェインのメニュー: [ADR 0139](../decisions/0139-track-header-fx-names.md)。
- フォルダ開閉と表示行の保持: [ADR 0128](../decisions/0128-folder-disclosure-and-row-projection.md)。

- デバイス列の開閉・文書内の表示状態・可変幅の FX ドロップ: [ADR 0137](../decisions/0137-device-disclosure.md)。
- Computer Use 監査と再調査: [Computer Use 監査](2026-10-14-computer-use.md)、[取得エラー再調査](2026-10-14-computer-use-reinvestigation.md)、[同一デスクトップ再監査](2026-10-14-computer-use-same-desktop.md)。
- ペイン境界・ミックス UI・高リフレッシュ描画・可視列キャッシュ: [ADR 0098](../decisions/0098-pane-render-boundaries.md)、[ADR 0107](../decisions/0107-mix-controls.md)、[2026-10-17 Apple Silicon 監査](2026-10-17-apple-silicon-audit.md)、[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)、[ADR 0113](../decisions/0113-product-macos-workgroup-and-caches.md)。
- 選択トラックのミュート・ソロ、音量・パンの数値入力とヘッダの読み上げ増減:
  [ADR 0189](../decisions/0189-selected-track-mute-and-solo.md)、[ADR 0191](../decisions/0191-selected-track-numeric-mix.md)、
  [ADR 0192](../decisions/0192-selected-track-accessibility-steps.md)。
  数値入力・読み上げ増減に続き、ドラッグ・既定値リセットも共通差分・全対象の試聴・単一 Undo へ接続した
  （[ADR 0228](../decisions/0228-selected-track-live-mix.md)）。
- ヘッダの値操作の文書切替境界: [ADR 0193](../decisions/0193-header-mix-gesture-document-scope.md)。
  押下・リセット待ち・数値欄を文書交換で取り消し、旧操作へ新文書のコールバックを貸さない。
- 文書の寿命の状態のまとめ・丸ごとの差替え・省略できない文書 ID と 1 か所の照合: [ADR 0203](../decisions/0203-document-session.md)。
- クリップのゲイン・フェードの文書境界と押下時の編集先保持:
  [ADR 0194](../decisions/0194-clip-mix-gesture-document-scope.md)。旧押下・数値欄と投入済みの送信を文書交換で拒否する。
- 値の操作子のクリック規則・入力欄・`text_to_value` の文字入力: [ADR 0153](../decisions/0153-value-control-clicks-and-typed-entry.md)。
- ピアノロールの単一ノートのドラッグでの Shift 吸着解除・最短 1 tick と解放時の確定:
  [ADR 0167](../decisions/0167-piano-note-drag-snapping.md)。
- ノートの音価・位置の編集で固定クリップも全長まで広げ、確定表示と履歴へ反映する:
  [ADR 0168](../decisions/0168-note-edit-clip-growth.md)。
- ピアノロールの空白ドラッグによる追加・解放時の単一確定・取消と新規の最短長:
  [ADR 0171](../decisions/0171-piano-blank-drag-insertion.md)。
  単クリックの入力・共有する入力長・Undo / Redo と再オープン時の初期化: [ADR 0172](../decisions/0172-piano-click-and-input-length.md)。
- ピアノロールの右ドラッグ枠選択・Command の追加と解除・右メニューと一括削除:
  [ADR 0176](../decisions/0176-piano-note-selection-and-deletion.md)。
- 単一ノートの Option＋上下ドラッグ・幾何を保つベロシティ専用アクションと全トラック共通の入力値:
  [ADR 0173](../decisions/0173-piano-note-velocity.md)。
  下部の棒と全音高の時刻内表示・共通仮表示・単一確定・読み上げの操作: [ADR 0175](../decisions/0175-piano-velocity-lane.md)。
- 自動配置の中身の小節基準と、小節未満の移動・複製での自動状態の保持:
  [ADR 0170](../decisions/0170-content-relative-auto-bars.md)。
  ノートの所属替え・固定窓の小節拡張・隣のクリップを壁にする残りの見直し案（草案・未採用）:
  [ADR 0169](../decisions/0169-clip-growth-follows-notes-and-stops-at-walls.md)。
- 選択済みノートと下部の棒のベロシティ一括調整・共通上下限・共有内容の重複排除と単一 Undo:
  [ADR 0177](../decisions/0177-piano-batch-note-velocity.md)。
- 選択済みノート本体の一括移動・共通時刻差と音程差・画面外を含む上下限制限・共有内容の単一編集と取消:
  [ADR 0178](../decisions/0178-piano-batch-note-movement.md)。
- ノートの Command 複製移動・クリックとの分離・元と強弱の保持・共有内容の重複排除と複製側の選択:
  [ADR 0184](../decisions/0184-piano-note-duplicate-drag.md)。
- 選択済みノートの左右端の一括伸縮・共通の最短長と時刻制限・反対端と入力記憶の保持:
  [ADR 0179](../decisions/0179-piano-batch-note-resizing.md)。
- ピアノロールの排他的な操作状態・押下による前の操作の取消・取消の範囲: [ADR 0206](../decisions/0206-piano-interaction-state.md)。
- ピアノロールのトラック切替・再生成で共有する時間・音域・倍率と、別文書での初期化:
  [ADR 0174](../decisions/0174-shared-piano-viewport.md)。
- ピアノロールの自動／固定グリッドと共通吸着幅・表示線・旧押下の取消: [ADR 0185](../decisions/0185-piano-auto-and-fixed-grid.md)。
  固定音価の三連符・付点と種類別の表示名: [ADR 0186](../decisions/0186-piano-triplet-and-dotted-grids.md)。
  吸着幅と独立した拍・4 拍の目印と、縮小時の保持・描画順・重複排除: [ADR 0187](../decisions/0187-piano-independent-time-guides.md)。
- ペインのリサイズ中の伸縮とヘッダ幅の保持: [ADR 0144](../decisions/0144-unscaled-canvas-resizing.md)。
- 反転した表示面の固定位置と状態行・再生ボタンの寸法: [ADR 0149](../decisions/0149-flipped-canvas-anchor-and-fixed-transport-chrome.md)。
- 入力と大きさの違う drawable への描画と、広がった余白の背景: [ADR 0150](../decisions/0150-stale-drawable-frame.md)。
- 別作業ツリーのサイズ不一致 drawable 修正の評価: [2026-10-24 omp-dual の実装監査](2026-10-24-omp-dual-audit.md)。
- 再生開始・ピアノロールの表示・ペインのリサイズ時のがくつき（上下の固定位置の誤り、状態行の高さ、作り直しの負荷）:
  [2026-10-22 GUI のがくつきの監査](2026-10-22-gui-jank-audit.md)。

## 5. モデル・Git 履歴・CLI / MCP

- 配置境界・ピーク部分読出・状態取り込み: [ADR 0109](../decisions/0109-placement-bounds-and-plugin-capture.md)、[ADR 0110](../decisions/0110-apple-silicon-draw-and-peaks.md)。
- パッケージの小さいゲイン・パンの保存精度と、クリップの直交ドラッグの恒等性:
  [ADR 0196](../decisions/0196-lossless-package-floats.md)。既存の正確な表記は維持し、9 桁丸めで失われるビットだけを保全する。
- Phase 2 ヘッドレス MCP: [ADR 0115](../decisions/0115-phase2-mcp.md)。
- 選択位置へのトラック挿入と末尾追加: [ADR 0127](../decisions/0127-positioned-track-insertion.md)。
- トラックの通常複製と導出 ID: [ADR 0129](../decisions/0129-track-duplication.md)。
- クリップの移動・複製移動と置き換えでの着地（削除・切詰め・分割）: [ADR 0154](../decisions/0154-clip-move-and-duplicate.md)。
- アレンジ自身の ⌘D・現在の窓の直後への独立した複製・単一 Undo と保存:
  [ADR 0188](../decisions/0188-arrange-adjacent-clip-duplication.md)。

- 文書の手番の調停（ワーカー・デバイスレーン・保存の取込み）・完了の 1 回の保証・`Presenter`: [ADR 0204](../decisions/0204-document-service.md)。

## 6. ビルド・対象プラットフォーム・検査基盤

- macOS 27 / M4 基準と最適化監査: [2026-10-17 macOS 27 最適化監査](2026-10-17-macos27-optimization-audit.md)、[ADR 0112](../decisions/0112-m4-minimum.md)、[ADR 0114](../decisions/0114-m4-build-baseline.md)。
- 検査基盤とトークン効率化: [ADR 0093](../decisions/0093-product-contract-gate.md)、[ADR 0094](../decisions/0094-agent-development-foundation.md)、[ADR 0100](../decisions/0100-scoped-verification.md)、[ADR 0119](../decisions/0119-agent-token-efficiency.md)。
- モデルの時間単位・値の範囲の共有と Swift の拍の変換: [ADR 0200](../decisions/0200-shared-model-units-and-limits.md)。
  ループの整数下書きと未変更の入力欄の端点保持: [ADR 0201](../decisions/0201-integer-loop-drafts.md)。
- 編集登録簿からの Swift 型生成・鮮度検査と、型付き単一／一括の製品橋渡し:
  [ADR 0202](../decisions/0202-generated-swift-edit-actions.md)。controller の編集をすべて移行し、一括編集内の結果参照も型で表す。raw JSON の互換入口だけが残る。
- 一括開始診断と根拠付きの節・行の読込: [ADR 0126](../decisions/0126-agent-context-and-source-reading.md)。
- 検査の実行時間（反例のコピーの保持・契約ごとの再利用・定義単位の選択）: [ADR 0148](../decisions/0148-verification-pipeline-speed.md)。
- 検査時間の継続記録・テスト製品のビルド・同一反例のビルド共用: [ADR 0195](../decisions/0195-verification-cost-observability.md)。
- 契約のプラグイン依存と Cargo ターゲットの範囲、通常・反例の独立再利用、ヘッダの不変時刻とフック全体の予算:
  [ADR 0197](../decisions/0197-verification-target-scopes.md)。Swift のリンク先変更と UI テスト全体の失効は引き続き区別する。
- 契約の Swift Testing の直接起動、Rust の同一反例の共有、診断・フックと製品証拠の分離、TSan の同一ビルド先:
  [ADR 0198](../decisions/0198-verification-launch-and-cache-cost.md)。広い製品依存と計装ごとの検査は維持する。
- 識別計算での同じ範囲の共有と `check-verification` の短縮、前後の測定: [ADR 0208](../decisions/0208-verification-shared-scopes.md)。
- UI テストのバンドルから実 HAL をなくすリンク時の代役と、触れたテストを名指しする `check-real-hal`: [ADR 0209](../decisions/0209-real-hal-guard.md)。
- サニタイザの Swift 全体実行の同時実行数と、全テスト同時で競合を探す実行、前後の測定: [ADR 0210](../decisions/0210-sanitizer-test-width.md)。
- 通常の check-swift の同時実行数をサニタイザの合否の実行と揃える: [ADR 0332](../decisions/0332-check-swift-test-width.md)。
- 作業ごとの検査と節目の全体検査の分離、反例とサニタイザの選択条件: [ADR 0212](../decisions/0212-verification-tiers.md)。
- 終了コードの保証・実行予定だけの環境事前確認・依存順序と失敗検出遅延: [ADR 0166](../decisions/0166-verification-failure-detection.md)。
- 起動復旧の終了待ち・Darwin errno の検査ランタイム修正: [ADR 0121](../decisions/0121-startup-recovery-and-errno-validation.md)。
- 継続の先行再開の TSan 誤検出と検査ビルドだけの注釈: [ADR 0207](../decisions/0207-continuation-handoff-tsan.md)。
