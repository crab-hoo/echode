# ADR 0333: 出力の導入時の差分で増えるヘルパーも準備スレッドで起動し、再生はその計画を待つ

日付: 2026-11-13
状態: 採用。実機の操作感はユーザー確認待ち（UC-69）
関連: [ADR 0136](0136-output-opening-off-worker.md)、[ADR 0142](0142-background-plugin-graph-preparation.md)、[ADR 0147](0147-output-prewarm-and-background-media.md)、[ADR 0330](0330-playing-edit-media-and-tempo-retime.md)、[ADR 0276](0276-priority-order-and-delegated-decisions.md)

## 問題と再現

開始時の HEAD は `7a31da8`。P2-02 の応答性の残り（[進め方](../phase2.md)）のうち「初回の出力導入時の差分で増えたヘルパーの起動」を扱った。

出力を開く間（[ADR 0136](0136-output-opening-off-worker.md)）の編集は、`Session::install` が導入時に通常の `update` として送る。
`install` は、この差分を送った後で `prepare_in_background` を有効にしていた。
そのため差分のグラフは同期の経路（`publish_pending` の `HostGraph::reconcile`）を通り、増えたヘルパーの起動と初期化を文書ワーカーの手番で待った。
製品アプリはプロジェクトを開いた時に出力を準備する（[ADR 0147](0147-output-prewarm-and-background-media.md)）ので、開いた直後に FX を足すとこの経路に入る。
その間は後続の編集・Undo・停止・デバイスの要求が待つ。初期化の遅い第三者プラグインほど長い。

機器なしの出力で、準備中に 2 本目のトラックを足し、導入を直接呼んで残る状態を見た。
修正前の順序へ戻すと、導入の直後に置換の準備が残らず（同期で済んでいる）、新しい回帰が
`installing prepared the edit's graph on the worker` で失敗した（終了コード 101、`target/focused-checks/install-graph/red.log`）。
グラフを準備スレッドへ回しても、再生の開始の保留を素材だけに限ると、置き換えられる計画が鳴って
`the plan the edit replaced sounded` で失敗した（`target/focused-checks/install-graph/red-hold.log`）。

## 比べた案

1. **導入時は同期のまま（従来）**: 最初のブロックが全変更を持つ。代わりにヘルパーの初期化の間、ワーカーの手番が戻らない。
   [処理中も作業を続ける](../interaction-design.md#処理中も作業を続ける)に反し、入力の取りこぼしに近い待ちを生む。
2. **差分を準備スレッドへ回し、再生はすぐ始める**: 開いた出力への編集（[ADR 0142](0142-background-plugin-graph-preparation.md)）と同じ。
   ただし利用者が再生を押した時点の編集（足した FX やトラック）を欠いた音が、新しい計画の採用まで鳴る。
3. **差分を準備スレッドへ回し、再生の開始はその計画の公開まで待つ（採用）**: 従来と同じ音（最初に鳴るブロックから全変更を持つ）を保ち、ワーカーの手番だけを戻す。
   待ちは再生の開始の遅れとして現れ、従来の同期の待ちより長くならない。素材の読込みを待つ規則（[ADR 0330](0330-playing-edit-media-and-tempo-retime.md)）と同じ形である。

ADR 0276 の順で、案 1 は堅牢性（入力の待ち）、案 2 は見た目・機能（過渡的に違う音）を損なう。案 3 は両方を満たす。

## 決定

- `Session::install` は、出力を組み立てた直後、差分を送る前に `prepare_in_background` を有効にする。
  差分で増えるヘルパーの起動・状態の読込み・共有領域の準備は、開いた出力への編集と同じく `echode-graph-prepare` で行う。
  素材の読込みは従来どおり `echode-media-prepare` である。
- 準備中に編集があった導入は、その時点で要求済みの最新の計画（`PlaybackControl::requested_plan`）を記録する（`OutputStream::hold_start_for_requested_plan`）。
  再生はその計画が公開されるまで始めない。判定は `holding_start`（素材の読込み中、または記録した計画が未公開）で、
  導入時の再生・`resume`・`service` での再開の 3 か所が同じ述語を使う。
- 記録した計画より後の編集は待たない。開いた出力への編集として、公開後に採用される（ADR 0142 の過渡的な旧版の採用と同じ）。
- 待つ間のデバイス要求は、従来の置換と同じく `DeviceAccess::Opening` として投入順に待つ。
- 準備の失敗は従来どおり非同期の準備エラーとして返り、出力を閉じる。記録した計画は出力と一緒に捨てる。
- 出力の初回の準備（準備スレッドの `stream::Prepared::new`）とオフライン書出しの経路は変えない。

## 検査

- 必須契約 `output-install-graph`（`session::tests::contract_an_install_prepares_the_graph_of_an_edit_made_meanwhile_off_the_worker_under_rtsan`）は、
  再生中に準備した出力の導入で、準備中の編集のグラフが準備スレッドに残ること、`service` なしでは何も鳴らないこと、
  公開後に編集後の計画（2 本のトラック）から鳴ることを製品のコールバックで確かめる。
  ヘルパー実行ファイルのないサニタイザー用ビルドでも、検査用の経路でグラフの準備を選ぶ。
  負の対照は導入時に `prepare_in_background` を有効にしない。
- 既存の `output-install-held-play` と `output-prewarm-play` の負の対照は、新しい述語 `holding_start` の行を指す。
- 実ヘルパーの初期化を保留した制御側の順序は、既存の `background-graph` が同じ準備スレッドの経路で確かめる。
  Session の導入から実ヘルパーまでを通す機器なしの検査はない。両者をつないだ実機の経路は UC-69 で確かめる。
- DSP と RT 区間は変えない。ブロック長の比較は既存の製品回帰を維持する。

## 影響と残り

- 開いた直後の FX の追加で、ワーカーの手番がヘルパーの初期化を待たない。再生を押していれば、開始がその分遅れる。
- 計画生成（`PlaybackControl::prepare_plan` のイベント領域の作成）は文書ワーカーに残る。
- 置換準備専用のデバイス列の表示と、文書終了時の初期化待ちの中断は未接続のまま（ADR 0142）。
