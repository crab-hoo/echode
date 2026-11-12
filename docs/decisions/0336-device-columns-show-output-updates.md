# ADR 0336: 開いた出力の更新中は、未取得のデバイス列に「音声出力を更新中」と出す

日付: 2026-11-13
状態: 採用。実画面と VoiceOver はユーザー確認待ち（UC-71）
関連: [ADR 0141](0141-device-output-preparation-status.md)、[ADR 0142](0142-background-plugin-graph-preparation.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)

## 問題

開始時の HEAD は `ecb18c8`。P2-02 の応答性の残り（[進め方](../phase2.md)）のうち「置換準備専用のデバイス列の表示」を扱った。

開いた出力へ FX を足すと、新しいヘルパーは準備スレッドで起動し、その間のデバイス要求は待つ（ADR 0142）。
初回の出力準備は `AudioState.opening` で「音声出力を準備中」と出すが（ADR 0141）、置換の準備は音声状態に現れなかった。
そのため列は「パラメーターを読み込み中」と出し、プラグインの応答が遅いのか、ヘルパーが起動中なのかを区別できなかった。
`pending_plan` は計画の未反映であり、ヘルパーの起動を伴わない編集でも立つので、列の表示には使えない。

## 比べた案

1. **`pending_plan` で出す**: ノートの編集のたびに立ち、一覧の待ちとは関係しない。採らない。
2. **デバイスごとに起動中かを返す**: 個別の進捗を作ることになり、ADR 0141 の「共有出力の状態から特定のデバイスを推測しない」に反する。
3. **共有出力の更新中を 1 つの値で返す（採用）**: Session がデバイス要求を `DeviceAccess::Opening` で待たせる条件（`OutputStream::preparing_graph`）と同じ判定を返す。
   表示と待ちが別の規則にならない（ADR 0276 の共通の不変条件）。

## 決定

- Rust の `AudioState.updating` を足した。開いた出力が置換（編集が足したヘルパー・素材）を準備している間だけ真で、`opening` の間は偽である。
  保存形式・C ABI・プラグイン通信は変えない。
- Swift の `AudioStatus.updating` は、無い時を偽として復号する（古い代役の応答）。
  `DocumentSession.outputUpdating` は `outputOpening` と同じく別に観測し、位置のポーリングで列を作り直さない。
- 一覧が未取得・取得中の列は、出力の準備中なら「音声出力を準備中」、更新中なら「音声出力を更新中」、どちらでもなければ従来の読込み表示を出す。
  オフライン・一覧の失敗・空の一覧は従来どおり優先する。取得済みの一覧は隠さない。列の開閉は状態を保つ。状態文のアクセシビリティ名はデバイス名を含む。

## 検査

- 必須契約 `device-preparation-ui` に、出力の更新中の段を足した。実デバイス列が「音声出力を更新中」を出し、取得済みの列は使え、更新が終わると読込み表示へ戻る。
  2 つ目の負の対照は更新中の文言をなくす（`Expectation failed: column.message.stringValue == text("Updating audio output")` で失敗した）。
- 単体回帰で、準備中が更新中より優先すること、失敗・オフラインが更新中より優先すること、開閉で保つことを確かめる。
  観測の回帰で、位置のポーリングが `outputUpdating` の読み手を無効にしないことを確かめる。
- Session の機器なし回帰（`retained_output_replacements_wait_for_new_devices_without_reopening`）で、置換の準備中に `updating` が真、収束後に偽になることを確かめる。
- `make spike-main-thread`（画面・機器なし、128 トラック）は合格し、位置の応答 p50 0.20 ms・状態の応答 p50 4.2 ms で、[ADR 0289](0289-split-audio-replies-and-one-label-pass.md) の値から変わらなかった。

## 残る範囲

- 実画面の見え方と VoiceOver は UC-71 で確かめる。
- 計画生成は文書ワーカーに残る（[ADR 0335](0335-retimed-plans-keep-note-numbers.md)）。
